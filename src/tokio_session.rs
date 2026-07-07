//! Tokio-backed terminal session: a driver over the sans-io core.
//!
//! This module owns qwertty's first async runtime boundary. It is **not** an async wrapper around
//! the synchronous [`TerminalSession`] methods: it composes the sans-io core layers and drives them
//! with Tokio readiness (design 04).
//!
//! - [`TerminalSession`] owns the device, the mode ledger, the restore handle, and the
//!   `enter`/`leave` lifecycle. This driver reuses it wholesale for ownership and teardown.
//! - [`SemanticDecoder`] turns the raw bytes each readiness read yields into typed [`Event`] values
//!   (design 02).
//! - `Correlator` matches those events against registered query `Expectation`s, completing a query
//!   or passing an event through in arrival order (design 03).
//!
//! The driver holds a small queue of decoded-but-undelivered [`Event`]s and the id of the one live
//! query expectation. Time is injected only through `tokio::time` deadlines this driver owns; the
//! core never sees a clock. Every `async fn` is cancel-safe: all state lives on the struct, so a
//! dropped future abandons only its own wait and never loses a buffered event or a decoder byte
//! (design 04 / design 03 §proof plan).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use tokio::io::unix::AsyncFd;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::time::{Instant, timeout_at};

use crate::caps::{
    Capabilities, Finding, identity_from_env, infer_hyperlinks, infer_truecolor, std_env_source,
};
use crate::commands::terminal::MouseMode;
use crate::correlate::{Correlator, Expectation, ExpectationId, Feed, Reply, Resolution};
use crate::report::{
    CursorPositionReport, DecPrivateModeReport, OscColorKind, TerminalStatusReport,
};
use crate::{
    Command, CommandBuffer, Event, InputBytes, KittyKeyboardFlags, KittyKeyboardGrant, ResizeEvent,
    SemanticDecoder, Terminal, TerminalDevice, TerminalSession, TerminalSize, commands, terminal,
};

/// The DEC private modes the capability probe bundle queries, and the [`Capabilities`] field each
/// answer sets. Kept as one table so the write side, the register side, and the collect side stay
/// in agreement (design 03 probe bundle).
const PROBE_MODES: [ProbeMode; 4] = [
    ProbeMode {
        mode: 2026,
        field: CapabilityField::SynchronizedOutput,
    },
    ProbeMode {
        mode: 2027,
        field: CapabilityField::GraphemeClustering,
    },
    ProbeMode {
        mode: 2048,
        field: CapabilityField::InBandResize,
    },
    ProbeMode {
        mode: 2004,
        field: CapabilityField::BracketedPaste,
    },
];

/// One DEC private mode the probe asks about, and which [`Capabilities`] boolean its answer sets.
#[derive(Clone, Copy)]
struct ProbeMode {
    mode: u16,
    field: CapabilityField,
}

/// Which [`Capabilities`] boolean a DECRQM answer populates.
#[derive(Clone, Copy)]
enum CapabilityField {
    SynchronizedOutput,
    GraphemeClustering,
    InBandResize,
    BracketedPaste,
}

const DEV_TTY: &str = "/dev/tty";
const READ_BUFFER_LEN: usize = 1024;

/// A Tokio-backed terminal session driving the sans-io core.
///
/// `TokioTerminalSession` is available when the `tokio` feature is enabled. It owns a live terminal
/// device registered with Tokio readiness, enters raw mode when the session starts, writes output
/// bytes in method-call order, reads input through runtime-backed I/O, decodes input into typed
/// [`Event`] values with a [`SemanticDecoder`], correlates query replies with a `Correlator`, and
/// gives callers an explicit async [`leave`](Self::leave) path for terminal-mode cleanup errors.
///
/// The generic parameter `D` is the underlying [`TerminalDevice`]. It defaults to the live
/// [`Terminal`]; tests and embedding environments can drive the same Tokio session headless over
/// any other device that exposes a pollable descriptor (such as `FakeDevice`) through
/// [`from_device`](Self::from_device). A device that returns `None` from
/// [`TerminalDevice::as_fd`] cannot be registered with Tokio readiness and is rejected at
/// construction with [`terminal::Error::Unsupported`].
///
/// The composed [`TerminalSession`] stays runtime-neutral. This type is not a thin async wrapper
/// around its blocking methods; it is the driver that feeds the core bytes and time.
///
/// # Cancellation
///
/// Every `async fn` on this type is cancel-safe. All state — the decoder, the correlator, the
/// pending-event queue, and the live-query id — lives on the struct, so dropping a future
/// mid-await loses nothing: a later call resumes from the same state. See
/// [`next_event`](Self::next_event) and the query helpers for the specifics.
///
/// # Re-entrancy
///
/// `enter`/`leave` re-entrancy over this Tokio type (cycling raw mode without dropping the fd
/// registration) is deferred to a later slice. [`leave`](Self::leave) here consumes the session for
/// API continuity with the previous implementation; construct a fresh session to re-enter.
///
/// # Example
///
/// ```no_run
/// use qwertty::{ProtocolPosition, TokioTerminalSession, commands};
///
/// # async fn run() -> qwertty::Result<()> {
/// let mut session = TokioTerminalSession::open()?;
///
/// session.command(commands::screen::clear()).await?;
/// session
///     .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))
///     .await?;
/// session.text("Ready\r\n").await?;
/// session.flush().await?;
/// session.leave().await
/// # }
/// ```
#[derive(Debug)]
pub struct TokioTerminalSession<D: TerminalDevice = Terminal> {
    /// The composed sans-io session: device, mode ledger, restore handle, enter/leave.
    session: TerminalSession<D>,
    /// A duplicate of the device descriptor registered with Tokio readiness.
    ///
    /// The dup shares the same open file description as the device the session owns, so readiness
    /// observed on either applies to both. Setting the dup non-blocking (required by [`AsyncFd`])
    /// therefore affects the shared description; [`original_flags`](Self::original_flags) captures
    /// what to put back on teardown.
    readiness: AsyncFd<OwnedFd>,
    /// The device status flags captured before this session set the descriptor non-blocking.
    ///
    /// Restored on every teardown path (leave and drop). This matters most for the
    /// [`open`](Self::open) path, whose descriptor is a duplicate of the inherited standard input:
    /// its open file description is shared with the parent shell, so a leaked non-blocking flag
    /// would corrupt the shell's own reads (FM-L class).
    original_flags: OFlags,
    /// The semantic decoder that turns each read's raw bytes into typed events (design 02).
    decoder: SemanticDecoder,
    /// The sans-io correlator matching query replies to expectations (design 03).
    correlator: Correlator,
    /// Decoded-but-undelivered passthrough events, in arrival order, awaiting `next_event`.
    pending: VecDeque<Event>,
    /// The id of the single in-flight query expectation, if any.
    ///
    /// A query helper stores its expectation id here for the life of the query. It is swept (see
    /// [the cancel-sweep](#the-cancel-sweep)) at the start of the next query so a previously
    /// dropped/cancelled query's expectation is resolved as `Resolution::Cancelled` before a new
    /// one registers.
    active_query: Option<ExpectationId>,
    /// The ids of a capability probe bundle's still-registered expectations, if a probe is (or
    /// was) in flight.
    ///
    /// A probe registers several expectations at once (design 03 probe bundle) and records them
    /// here for the same reason a single query records [`active_query`](Self::active_query): a
    /// dropped probe future leaves its expectations registered, so they are swept as
    /// `Resolution::Cancelled` before the next query registers. Cleared when a probe finishes
    /// normally (its own fence resolves the set).
    active_probe: Vec<ExpectationId>,
}

impl TokioTerminalSession<Terminal> {
    /// Opens the current controlling terminal and starts a Tokio-backed session.
    ///
    /// When standard input is a read-write terminal, this reaches the controlling terminal by
    /// duplicating that inherited descriptor (see `controlling_terminal_via_stdin`); on macOS a
    /// freshly opened controlling-terminal descriptor is rejected by kqueue, while the inherited
    /// one registers fine. Otherwise it opens `/dev/tty`. Either way it captures the current
    /// terminal mode, enters raw mode through the session's ledger, sets the readiness
    /// descriptor non-blocking, and registers it with the current Tokio runtime.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal cannot be opened, configured, or registered with Tokio.
    pub fn open() -> terminal::Result<Self> {
        match controlling_terminal_via_stdin() {
            Some((device, path)) => {
                let terminal = Terminal::from_file(device, path)?;
                Self::from_terminal(terminal)
            }
            None => Self::open_path(resolved_controlling_terminal_path()),
        }
    }

    /// Opens a specific terminal device path and starts a Tokio-backed session.
    ///
    /// This is mainly useful for tests, embedding environments, and advanced callers that have
    /// already resolved the terminal device they want qwertty to own.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the path cannot be opened as a terminal device, raw mode cannot be
    /// entered, non-blocking mode cannot be set, or Tokio cannot register the file descriptor.
    pub fn open_path(path: impl Into<PathBuf>) -> terminal::Result<Self> {
        let terminal = Terminal::open_path(path)?;
        Self::from_terminal(terminal)
    }

    /// Builds a Tokio-backed session from an already-opened terminal.
    fn from_terminal(terminal: Terminal) -> terminal::Result<Self> {
        let session = TerminalSession::from_terminal(terminal)?;
        Self::from_session(session)
    }

    /// Returns a panic-safe restore handle for this session.
    ///
    /// The handle stays valid without borrowing the session, so it can live inside a panic hook
    /// installed once for the whole program. This delegates to the composed
    /// [`TerminalSession::restore_handle`]; see [`RestoreHandle`](crate::RestoreHandle) for the
    /// hook pattern and what the emergency path covers.
    #[must_use]
    pub fn restore_handle(&self) -> crate::RestoreHandle {
        self.session.restore_handle()
    }
}

impl<D: TerminalDevice> TokioTerminalSession<D> {
    /// Starts a Tokio-backed session over any pollable terminal device.
    ///
    /// This is the runtime-neutral-core payoff: a headless device such as `FakeDevice` drives the
    /// real Tokio session, so query correlation, cancellation, and event delivery are testable in
    /// plain unit tests with no pseudoterminal. The device must expose a pollable descriptor
    /// through [`TerminalDevice::as_fd`]; one that returns `None` is rejected with
    /// [`terminal::Error::Unsupported`] because Tokio readiness has nothing to register.
    ///
    /// The session enters raw mode through its ledger, and the readiness descriptor is set
    /// non-blocking exactly as for a live terminal.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::Unsupported`] when the device has no pollable descriptor, or
    /// another error when raw mode cannot be entered, non-blocking mode cannot be set, or Tokio
    /// cannot register the descriptor.
    pub fn from_device(device: D) -> terminal::Result<Self> {
        let session = TerminalSession::from_device(device)?;
        Self::from_session(session)
    }

    /// Wraps an entered [`TerminalSession`] with the readiness registration and sans-io core.
    ///
    /// This duplicates the device descriptor for Tokio readiness (a dup shares the same open file
    /// description, so readiness is shared), captures the original status flags, sets the dup
    /// non-blocking, and registers it with the current runtime.
    fn from_session(session: TerminalSession<D>) -> terminal::Result<Self> {
        let borrowed = session.device().as_fd().ok_or_else(|| {
            terminal::Error::unsupported("Tokio readiness registration", "device without a fd")
        })?;

        let dup: OwnedFd = rustix::io::dup(borrowed)
            .map_err(io::Error::from)
            .map_err(terminal::Error::open_terminal)?;

        let original_flags = fcntl_getfl(&dup)
            .map_err(io::Error::from)
            .map_err(terminal::Error::open_terminal)?;
        fcntl_setfl(&dup, original_flags | OFlags::NONBLOCK)
            .map_err(io::Error::from)
            .map_err(terminal::Error::open_terminal)?;

        let readiness = match AsyncFd::try_new(dup) {
            Ok(readiness) => readiness,
            Err(err) => {
                let (dup, err) = err.into_parts();
                // Put the original flags back on the shared description before giving up.
                _ = fcntl_setfl(&dup, original_flags);
                return Err(terminal::Error::open_terminal(err));
            }
        };

        Ok(Self {
            session,
            readiness,
            original_flags,
            decoder: SemanticDecoder::new(),
            correlator: Correlator::new(),
            pending: VecDeque::new(),
            active_query: None,
            active_probe: Vec::new(),
        })
    }

    /// Returns the current terminal size.
    ///
    /// The result is a snapshot. This method does not subscribe to future resize events. The size
    /// is resolved through the composed session's geometry ladder (device measurement with an
    /// environment fallback).
    ///
    /// # Errors
    ///
    /// Returns an error when neither the device nor the environment yields a usable size.
    pub fn size(&self) -> terminal::Result<TerminalSize> {
        self.session.size()
    }

    /// Writes one terminal command through Tokio readiness.
    ///
    /// Commands, raw bytes, and text are written in the order their session methods are awaited.
    /// The command bytes are not flushed until [`flush`](Self::flush) is called or the
    /// operating system decides to make them visible.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write all encoded bytes.
    pub async fn command(&mut self, command: impl AsRef<Command>) -> terminal::Result<()> {
        let mut bytes = Vec::new();
        command.as_ref().encode(&mut bytes);
        self.bytes(bytes).await
    }

    /// Writes raw bytes through Tokio readiness.
    ///
    /// This method does not inspect, escape, or validate bytes. Use it for renderer output that is
    /// already encoded. Prefer [`text`](Self::text) for ordinary UTF-8 render text.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write all bytes.
    pub async fn bytes(&mut self, bytes: impl AsRef<[u8]>) -> terminal::Result<()> {
        let mut bytes = bytes.as_ref();
        while !bytes.is_empty() {
            let mut guard = self
                .readiness
                .writable()
                .await
                .map_err(terminal::Error::write_terminal)?;

            // Write through the *registered* readiness descriptor, which shares its open file
            // description with the device the session owns (the dup), so bytes written here are the
            // device's bytes. Doing the I/O on the fd Tokio registered is what keeps readiness
            // correct under edge-triggered polling; the closure returns `io::Result` so `try_io`
            // can classify a `WouldBlock` (clearing the guard's readiness) from a real
            // error, exactly as the old direct-`File` loop did.
            match guard.try_io(|inner| fd_write(inner.get_ref(), bytes)) {
                Ok(Ok(0)) => {
                    return Err(terminal::Error::write_terminal(io::Error::new(
                        ErrorKind::WriteZero,
                        "failed to write terminal output",
                    )));
                }
                Ok(Ok(len)) => bytes = &bytes[len..],
                Ok(Err(err)) => return Err(terminal::Error::write_terminal(err)),
                Err(_would_block) => {}
            }
        }

        Ok(())
    }

    /// Writes UTF-8 render text through Tokio readiness.
    ///
    /// This method does not escape control characters. Renderers that accept user-controlled text
    /// should perform their own escaping policy before writing to a terminal stream.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write all text bytes.
    pub async fn text(&mut self, text: impl AsRef<str>) -> terminal::Result<()> {
        self.bytes(text.as_ref()).await
    }

    /// Reads raw terminal input bytes into `buffer` through Tokio readiness.
    ///
    /// This returns one operating-system read as [`InputBytes`]. It does **not** decode UTF-8,
    /// parse escape sequences, match query replies, classify keys, or apply any protocol policy
    /// — it is the raw byte foundation beneath [`next_event`](Self::next_event). A zero-length
    /// buffer returns an empty value without reading from the terminal.
    ///
    /// This bypasses the decoder and correlator: mixing raw `read_input` with `next_event` on the
    /// same session interleaves undecoded bytes with decoded events, so prefer one or the other for
    /// a given input stream. Cancel-safe: a cancelled await performs no read.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot read input.
    pub async fn read_input(&mut self, buffer: &mut [u8]) -> terminal::Result<InputBytes> {
        if buffer.is_empty() {
            return Ok(InputBytes::default());
        }

        loop {
            let mut guard = self
                .readiness
                .readable()
                .await
                .map_err(terminal::Error::read_terminal)?;

            match guard.try_io(|inner| fd_read(inner.get_ref(), buffer)) {
                Ok(Ok(len)) => return Ok(InputBytes::new(buffer[..len].to_vec())),
                Ok(Err(err)) => return Err(terminal::Error::read_terminal(err)),
                Err(_would_block) => {}
            }
        }
    }

    /// Reads and delivers the next terminal input [`Event`].
    ///
    /// Delivery order: a previously buffered passthrough event is returned first; otherwise this
    /// awaits terminal readiness, reads one operating-system read, decodes it into events, feeds
    /// each through the correlator, buffers the passthroughs in order, and returns the first.
    /// With no query registered the correlator passes everything through, so this is an
    /// ordinary decoded event stream.
    ///
    /// # Resize coalescing (design 01 §resize, R-IN-8, FM-G2)
    ///
    /// A resize storm collapses to a single [`Event::Resize`] carrying the **final** geometry,
    /// while every non-resize event keeps its order and identity. Precisely: when the event at
    /// the front of the queue is a resize and a *later* resize is still buffered behind it, the
    /// front resize is superseded and dropped; the surviving resize is the last one in the
    /// burst, delivered in its own position relative to the surrounding input. A queue of `R1
    /// K1 R2 K2 R3` therefore delivers `K1 K2 R3` — every keystroke in order, exactly one
    /// resize reflecting the final geometry.
    ///
    /// This is deliberately the opposite of the mouse and scroll policy, which never coalesces
    /// (FM-V6): a burst of scroll ticks delivers every tick, because per-terminal tick ratios carry
    /// information an application must be able to see. Only resize collapses, and only here in
    /// delivery — the decoder itself emits one event per report.
    ///
    /// # Cancellation
    ///
    /// Cancel-safe. The decoder state, the correlator, and the pending-event queue all live on the
    /// session, so a call cancelled while awaiting readiness leaves every already-decoded event and
    /// every buffered byte available to the next call.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot read input or returns end-of-file before
    /// another event is available.
    pub async fn next_event(&mut self) -> terminal::Result<Event> {
        loop {
            if let Some(event) = self.take_coalesced_event() {
                return Ok(event);
            }

            let events = self.read_events().await?;
            self.buffer_events(events);
        }
    }

    /// Pops the next event from the pending queue, applying resize coalescing.
    ///
    /// Resize events coalesce to the burst's last one (design 01 §resize, FM-G2): a front resize is
    /// dropped whenever a later resize is still buffered behind it, so a resize storm collapses to
    /// one `Resize` with the final geometry without reordering or dropping any non-resize event.
    /// Non-resize events (keys, mouse, scroll, focus, paste, syntax) are returned unchanged and in
    /// order — the never-coalesce policy for mouse and scroll (FM-V6) falls out of this: they are
    /// simply never the event the resize rule drops.
    ///
    /// Returns `None` only when the queue is empty.
    fn take_coalesced_event(&mut self) -> Option<Event> {
        take_coalesced_event(&mut self.pending)
    }

    /// Requests and reads the current terminal cursor position.
    ///
    /// This emits the Device Status Report request `CSI 6 n`, flushes output, and reads decoded
    /// input until a `CSI row ; column R` cursor position report completes the query. Events read
    /// before the report that are not the report remain queued in their original order for later
    /// [`next_event`](Self::next_event) calls.
    ///
    /// `timeout` bounds the whole request/response operation; on elapse the query resolves as a
    /// timeout and [`terminal::Error::QueryTimeout`] is returned. Cancelling the future while it is
    /// waiting leaves the session usable and preserves unrelated decoded events for later calls.
    ///
    /// This is a single-query convenience method. It does not implement a general query registry,
    /// concurrent query routing, capability probing, or terminal feature detection.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use qwertty::TokioTerminalSession;
    ///
    /// # async fn run() -> qwertty::Result<()> {
    /// let mut session = TokioTerminalSession::open()?;
    /// let report = session
    ///     .request_cursor_position(Duration::from_secs(1))
    ///     .await?;
    ///
    /// assert!(report.row() > 0);
    /// assert!(report.column() > 0);
    ///
    /// session.leave().await
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when writing, flushing, or reading terminal I/O fails, or when the timeout
    /// elapses before a cursor position report is received.
    pub async fn request_cursor_position(
        &mut self,
        timeout: Duration,
    ) -> terminal::Result<CursorPositionReport> {
        let reply = self
            .run_query(
                Expectation::CursorPosition,
                commands::cursor::request_position(),
                "cursor position query",
                timeout,
            )
            .await?;
        match reply {
            Reply::CursorPosition(report) => Ok(report),
            other => Err(unexpected_reply(other)),
        }
    }

    /// Requests and reads terminal status.
    ///
    /// This emits the Device Status Report request `CSI 5 n`, flushes output, and reads decoded
    /// input until a `CSI 0 n` ready report or a `CSI 3 n` malfunction report completes the query.
    /// Events read before the report that are not the report remain queued in their original order
    /// for later [`next_event`](Self::next_event) calls.
    ///
    /// `timeout` bounds the whole request/response operation; on elapse the query resolves as a
    /// timeout and [`terminal::Error::QueryTimeout`] is returned. Cancelling the future while it is
    /// waiting leaves the session usable and preserves unrelated decoded events for later calls.
    ///
    /// This is a single-query convenience method. It does not implement a general query registry,
    /// concurrent query routing, capability probing, or terminal feature detection.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use qwertty::TokioTerminalSession;
    /// use qwertty::report::TerminalStatus;
    ///
    /// # async fn run() -> qwertty::Result<()> {
    /// let mut session = TokioTerminalSession::open()?;
    /// let report = session
    ///     .request_terminal_status(Duration::from_secs(1))
    ///     .await?;
    ///
    /// assert_eq!(report.status(), TerminalStatus::Ready);
    ///
    /// session.leave().await
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when writing, flushing, or reading terminal I/O fails, or when the timeout
    /// elapses before a terminal status report is received.
    pub async fn request_terminal_status(
        &mut self,
        timeout: Duration,
    ) -> terminal::Result<TerminalStatusReport> {
        let reply = self
            .run_query(
                Expectation::TerminalStatus,
                commands::terminal::request_status(),
                "terminal status query",
                timeout,
            )
            .await?;
        match reply {
            Reply::TerminalStatus(report) => Ok(report),
            other => Err(unexpected_reply(other)),
        }
    }

    /// Requests kitty keyboard progressive-enhancement flags and verifies what was granted.
    ///
    /// This is the verify-after-push handshake (design 06). It:
    ///
    /// 1. writes `CSI > flags u` to push the caller-chosen `requested` flags (rabbitui P0-4);
    /// 2. queries `CSI ? u` and reads decoded input until the `CSI ? flags u` reply completes,
    ///    exactly like the cursor-position and terminal-status helpers — unrelated events read
    ///    before the reply stay queued for later [`next_event`](Self::next_event) calls;
    /// 3. records the **granted** flags in the session mode ledger (`CSI > granted u` to re-apply,
    ///    `CSI < 1 u` to pop), so teardown pops the reality, not the request; and
    /// 4. returns a [`KittyKeyboardGrant`] carrying the requested set and what the terminal
    ///    granted.
    ///
    /// The granted set may be a subset of the requested set (the mismatch case the caller must
    /// handle). On a terminal that never answers the query — an old terminal, or a mux that
    /// swallowed it — the request **degrades gracefully**: the `timeout` elapses (or the terminal
    /// closes), the grant is recorded as *unknown* ([`KittyKeyboardGrant::is_unknown`]), **no**
    /// keyboard entry is recorded in the ledger, and no enhancement is assumed (FM-C4: unknown is
    /// not unsupported). Only a genuine read error other than EOF surfaces as an `Err`.
    ///
    /// `timeout` bounds the whole request/response operation. Cancelling the future while it is
    /// waiting leaves the session usable and preserves unrelated decoded events for later calls;
    /// note that the push bytes are already on the wire, so a cancelled request may leave flags
    /// pushed that the ledger has not recorded — call this to completion for the recorded-teardown
    /// guarantee.
    ///
    /// # Errors
    ///
    /// Returns an error only when writing, flushing, or a non-EOF read fails. A query timeout or
    /// EOF is reported as an unknown grant, not an error.
    pub async fn request_kitty_keyboard(
        &mut self,
        requested: KittyKeyboardFlags,
        timeout: Duration,
    ) -> terminal::Result<KittyKeyboardGrant> {
        self.command(commands::terminal::push_kitty_keyboard_flags(requested))
            .await?;

        let reply = self
            .run_query(
                Expectation::KittyKeyboardFlags,
                commands::terminal::query_kitty_keyboard_flags(),
                "kitty keyboard flags query",
                timeout,
            )
            .await;

        match reply {
            Ok(Reply::KittyKeyboardFlags(bits)) => {
                let granted = KittyKeyboardFlags::from_bits(bits);
                self.session.record_kitty_keyboard(granted);
                Ok(KittyKeyboardGrant::new(requested, Some(granted)))
            }
            Ok(other) => Err(unexpected_reply(other)),
            // A timeout or EOF means the terminal never answered: unknown, not unsupported. The
            // request degrades gracefully — no ledger entry, no assumed enhancement.
            Err(terminal::Error::QueryTimeout { .. }) => {
                Ok(KittyKeyboardGrant::new(requested, None))
            }
            Err(err) if is_unexpected_eof(&err) => Ok(KittyKeyboardGrant::new(requested, None)),
            Err(err) => Err(err),
        }
    }

    /// Probes the terminal's capabilities with one DA1-fenced query bundle (design 03/06).
    ///
    /// This is the batched capability probe every serious terminal consumer independently builds
    /// (helix, zellij, notcurses, codex): a single write of a bundle of queries plus a trailing DA1
    /// request as a fence, then **one** deadline. It never runs implicitly (FM-C7); a caller
    /// invokes it explicitly and owns the `timeout` budget (design 03: ~150 ms locally is
    /// typical; a longer budget is the caller's choice over ssh/mux, not a longer default —
    /// FM-C6/Q9).
    ///
    /// # What it asks
    ///
    /// In one buffer, written in this order (DA1 **last**, as the fence):
    ///
    /// - XTVERSION (`CSI > q`) → [`identity`](Capabilities::identity) (program, version);
    /// - DECRQM for modes 2026, 2027, 2048, 2004 → the four booleans
    ///   ([`synchronized_output`](Capabilities::synchronized_output),
    ///   [`grapheme_clustering`](Capabilities::grapheme_clustering),
    ///   [`in_band_resize`](Capabilities::in_band_resize),
    ///   [`bracketed_paste`](Capabilities::bracketed_paste));
    /// - kitty keyboard flags (`CSI ? u`) → [`kitty_keyboard`](Capabilities::kitty_keyboard);
    /// - OSC 10 / OSC 11 → [`foreground_color`](Capabilities::foreground_color) /
    ///   [`background_color`](Capabilities::background_color);
    /// - DA1 (`CSI c`), the fence →
    ///   [`primary_device_attributes`](Capabilities::primary_device_attributes).
    ///
    /// [`hyperlinks`](Capabilities::hyperlinks) and [`truecolor`](Capabilities::truecolor) are not
    /// asked for at all — no query exists for either (FM-C12) — and are populated purely from the
    /// environment, always with [`Evidence::Inferred`](crate::Evidence::Inferred) or
    /// [`Evidence::Unknown`](crate::Evidence::Unknown) evidence.
    ///
    /// # The fence (FM-Q7, the drain-before-read rule)
    ///
    /// A terminal answers queries in order, so DA1's reply arriving means every earlier reply that
    /// was coming has already arrived. When the DA1 expectation completes, this resolves every
    /// other still-pending bundle expectation as `Resolution::NoReply` — **but only after the
    /// entire current decode batch has been fed to the correlator**. A DA1 reply and a slower
    /// reply landing in the *same* `read()` must both be matched before the fence acts, or the
    /// slower reply would be lost (notcurses#2434). This method therefore feeds a whole read's
    /// events, and only then checks whether DA1 completed in that batch.
    ///
    /// A fully silent terminal (no DA1 either) costs **one** `timeout` total, after which every
    /// expectation resolves `NoReply` and an all-[`None`](Capabilities::is_all_unknown)
    /// `Capabilities` is returned — never a per-query timeout (the FM-C6 anti-pattern).
    ///
    /// # Unknown is not unsupported (FM-C4)
    ///
    /// Every unanswered field is `None`, meaning *unknown*. DA1 is a fence, not a feature oracle:
    /// its presence proves nothing about features, and its silence means the whole probe is
    /// unknown, not that the terminal lacks everything. A DECRQM "mode not recognized" (value
    /// 0) answer is also `None` for that field. This slice returns the minimal typed result;
    /// M3-S2 adds evidence-provenance, terminal identity, and env inference on top of these
    /// fields.
    ///
    /// # Typeahead survives
    ///
    /// Input that is not a bundle reply — typeahead, keystrokes, unrelated reports — passes through
    /// as ordinary events buffered for later [`next_event`](Self::next_event) delivery, in arrival
    /// order. A probe never eats a user's typeahead (FM-Q1).
    ///
    /// # Cancellation
    ///
    /// Cancel-safe like the other query helpers: the bundle's expectation ids live on the
    /// correlator, and a dropped probe future's leftover expectations are swept as
    /// `Resolution::Cancelled` before the next query registers (the same cancel-sweep the single
    /// query helpers use, generalized to the bundle).
    ///
    /// # Errors
    ///
    /// Returns an error only when writing or flushing the bundle fails, or a non-EOF read error
    /// occurs. A silent terminal (timeout) or a closed terminal (EOF) is **not** an error: both
    /// yield the `Capabilities` gathered so far, with the unanswered fields `None`.
    pub async fn probe_capabilities(
        &mut self,
        timeout: Duration,
    ) -> terminal::Result<Capabilities> {
        // Step 1: sweep a leftover single-query expectation from a dropped/cancelled prior query.
        self.sweep_active_query();
        // Also sweep any leftover bundle from a dropped prior probe (belt-and-suspenders: a probe
        // stores its ids in `active_probe`, resolved on every exit path, so this is normally
        // empty).
        self.sweep_active_probe();

        // Step 2: register the bundle. DA1 is registered like any other; the fence *semantics* is
        // this method's, keyed on the DA1 id. The M3 vocabulary within one bundle never overlaps
        // (distinct modes, distinct colours, distinct frames), so registration never conflicts.
        let bundle = self.register_probe_bundle();

        // Step 3: write the whole bundle in ONE buffer, DA1 last as the fence, then flush.
        let mut buffer = CommandBuffer::new();
        buffer
            .command(commands::terminal::request_xtversion())
            .command(commands::terminal::request_kitty_keyboard_flags())
            .command(commands::osc::request_foreground_color())
            .command(commands::osc::request_background_color());
        for probe in PROBE_MODES {
            buffer.command(commands::terminal::request_dec_private_mode(probe.mode));
        }
        // DA1 last: the fence.
        buffer.command(commands::terminal::request_primary_device_attributes());
        self.bytes(buffer.into_bytes()).await?;
        self.flush().await?;

        // The env-inferred findings and the env-only identity fallback never come from a terminal
        // reply (FM-C12: no query exists for hyperlinks/truecolor), so they are populated once, up
        // front, from the environment alone. If an XTVERSION reply arrives later,
        // `store_bundle_reply` overwrites `identity` with the XTVERSION-informed cross-check; until
        // then this is the best identity available (env only, no probed signal).
        let mut capabilities = Capabilities {
            hyperlinks: infer_hyperlinks(std_env_source),
            truecolor: infer_truecolor(std_env_source),
            identity: identity_from_env(None, std_env_source),
            ..Capabilities::default()
        };

        // Step 4: drain already-buffered events through the correlator before any read (design 03's
        // drain-before-read rule): a reply that arrived interleaved with earlier typeahead, already
        // sitting in `pending`, must be able to complete a bundle query before a new read.
        let buffered: Vec<Event> = self.pending.drain(..).collect();
        if self.feed_batch_into_bundle(&bundle, buffered, &mut capabilities) {
            self.finish_probe(&bundle);
            return Ok(capabilities);
        }

        // Step 5: one deadline loop over the whole probe.
        let deadline = Instant::now() + timeout;
        loop {
            let events = match timeout_at(deadline, self.read_events()).await {
                Ok(Ok(events)) => events,
                Ok(Err(err)) => {
                    // EOF or a read error ends the probe. Both resolve the still-pending bundle as
                    // NoReply and return what was gathered; a non-EOF error is still not fatal to
                    // the caller's capability picture (unknown, not unsupported), but a genuine I/O
                    // error is surfaced.
                    if is_unexpected_eof(&err) {
                        self.finish_probe(&bundle);
                        return Ok(capabilities);
                    }
                    self.resolve_bundle(&bundle, Resolution::Cancelled);
                    self.active_probe.clear();
                    return Err(err);
                }
                Err(_elapsed) => {
                    // The whole-probe deadline elapsed: a silent (or partially silent) terminal.
                    // Resolve the still-pending bundle as NoReply — one timeout total, not one per
                    // query (FM-C6) — and return the capabilities gathered so far.
                    self.finish_probe(&bundle);
                    return Ok(capabilities);
                }
            };

            // Feed the WHOLE batch before acting on any DA1 completion (FM-Q7). If DA1 completed in
            // this batch, the fence fires after the batch is fully matched.
            if self.feed_batch_into_bundle(&bundle, events, &mut capabilities) {
                self.finish_probe(&bundle);
                return Ok(capabilities);
            }
        }
    }

    /// Registers the DA1-fenced probe bundle and records its ids for the fence and cancel-sweep.
    ///
    /// Returns the bundle: the DA1 fence id, and each other expectation id paired with the
    /// [`Capabilities`] slot its reply populates. Every id is also recorded in `active_probe` so a
    /// dropped probe future's expectations are swept before the next query (cancel-safety).
    fn register_probe_bundle(&mut self) -> ProbeBundle {
        // Register in a fixed order; DA1 is registered *last* so it is the fence (its id keys the
        // whole fence semantics). Every field is set at construction so the struct never sits in a
        // half-initialized state.
        let xtversion = Some(self.register_probe(Expectation::XtVersion));
        let kitty = Some(self.register_probe(Expectation::KittyKeyboardFlags));
        let foreground = Some(self.register_probe(Expectation::OscColor {
            which: OscColorKind::Foreground,
        }));
        let background = Some(self.register_probe(Expectation::OscColor {
            which: OscColorKind::Background,
        }));
        let modes = PROBE_MODES
            .iter()
            .map(|probe| {
                let id = self.register_probe(Expectation::DecPrivateMode { mode: probe.mode });
                (id, probe.field)
            })
            .collect();
        let fence = Some(self.register_probe(Expectation::PrimaryDeviceAttributes));

        ProbeBundle {
            fence,
            xtversion,
            kitty,
            foreground,
            background,
            modes,
        }
    }

    /// Registers one bundle expectation, recording its id in `active_probe` for the cancel-sweep.
    fn register_probe(&mut self, expectation: Expectation) -> ExpectationId {
        let id = self
            .correlator
            .register(expectation)
            .expect("bundle expectations never overlap: distinct modes/colours/frames");
        self.active_probe.push(id);
        id
    }

    /// Feeds a whole decode batch through the correlator, collecting bundle replies into
    /// `capabilities`, and returns `true` when the DA1 fence completed in this batch.
    ///
    /// This is the FM-Q7 primitive: it processes every event in the batch (buffering non-bundle
    /// passthroughs into `pending` in arrival order) **before** returning, so a DA1 reply and a
    /// slower reply arriving in one `read()` both land. The caller acts on the DA1 completion only
    /// after this returns.
    fn feed_batch_into_bundle(
        &mut self,
        bundle: &ProbeBundle,
        events: Vec<Event>,
        capabilities: &mut Capabilities,
    ) -> bool {
        let mut fenced = false;
        for event in events {
            match self.correlator.feed(event) {
                Feed::Completed { id, .. } => {
                    let reply = self
                        .correlator
                        .take_reply(id)
                        .expect("a completion always has a reply to take");
                    store_bundle_reply(bundle, id, reply, capabilities);
                    if Some(id) == bundle.fence {
                        // The fence completed — but keep feeding the rest of the batch first.
                        fenced = true;
                    }
                }
                Feed::Passthrough(event) => self.pending.push_back(event),
            }
        }
        fenced
    }

    /// Fires the fence: resolves every still-pending bundle expectation as
    /// `Resolution::NoReply` and clears the probe's id set.
    ///
    /// Called once the DA1 fence completes, or once the whole-probe deadline/EOF ends the probe. A
    /// still-pending expectation is one whose reply never arrived; resolving it `NoReply` removes
    /// it so a later matching reply passes through as an event (rule 4), and leaves its
    /// [`Capabilities`] field `None` (unknown, FM-C4).
    fn finish_probe(&mut self, bundle: &ProbeBundle) {
        self.resolve_bundle(bundle, Resolution::NoReply);
        self.active_probe.clear();
    }

    /// Resolves every still-registered bundle expectation with `resolution`.
    fn resolve_bundle(&mut self, bundle: &ProbeBundle, resolution: Resolution) {
        for id in bundle.ids() {
            self.correlator.resolve(id, resolution);
        }
    }

    /// Sweeps a leftover probe bundle from a dropped/cancelled prior probe as cancelled.
    ///
    /// Mirrors [`sweep_active_query`](Self::sweep_active_query) for the bundle: if a probe future
    /// was dropped mid-await, its expectations are still registered, so resolving them
    /// `Resolution::Cancelled` before a new query registers keeps a stale bundle reply from being
    /// misdelivered (rule 4).
    fn sweep_active_probe(&mut self) {
        for id in std::mem::take(&mut self.active_probe) {
            self.correlator.resolve(id, Resolution::Cancelled);
        }
    }

    /// Enables mouse reporting for the given tracking mode, paired with SGR coordinates (1006).
    ///
    /// This writes `CSI ? N h CSI ? 1006 h` through the readiness path, flushes, and records the
    /// change in the composed session's mode ledger so `enter` re-applies it and teardown (leave,
    /// drop, or the panic-safe emergency path) resets both modes. Mouse reports then decode to
    /// [`Event::Mouse`] through [`next_event`](Self::next_event) with no scroll coalescing (FM-V6).
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write or flush the enable bytes.
    pub async fn enable_mouse(&mut self, mode: MouseMode) -> terminal::Result<()> {
        self.command(commands::terminal::enable_mouse(mode)).await?;
        self.flush().await?;
        self.session.record_mouse_enabled(mode);
        Ok(())
    }

    /// Enables focus reporting (mode 1004).
    ///
    /// Writes `CSI ? 1004 h`, flushes, and records the change so teardown resets it. Focus reports
    /// then decode to [`Event::Focus`] through [`next_event`](Self::next_event).
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write or flush the enable bytes.
    pub async fn enable_focus_events(&mut self) -> terminal::Result<()> {
        self.command(commands::terminal::enable_focus_events())
            .await?;
        self.flush().await?;
        self.session.record_focus_events_enabled();
        Ok(())
    }

    /// Enables bracketed paste (mode 2004).
    ///
    /// Writes `CSI ? 2004 h`, flushes, and records the change so teardown resets it. Pasted text
    /// then arrives as [`Event::Paste`] segments through [`next_event`](Self::next_event),
    /// normalized and delivered as data rather than typed keys (R-IN-7, FM-P12).
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write or flush the enable bytes.
    pub async fn enable_bracketed_paste(&mut self) -> terminal::Result<()> {
        self.command(commands::terminal::enable_bracketed_paste())
            .await?;
        self.flush().await?;
        self.session.record_bracketed_paste_enabled();
        Ok(())
    }

    /// Enables in-band resize reporting (mode 2048).
    ///
    /// Writes `CSI ? 2048 h`, flushes, and records the change so teardown resets it. Size changes
    /// then arrive as [`Event::Resize`] through [`next_event`](Self::next_event), which
    /// **coalesces** a resize storm to one event carrying the final geometry (design 01
    /// §resize, FM-G2).
    ///
    /// In-band resize is the preferred resize source: prefer it to the [`resize_stream`] `SIGWINCH`
    /// fallback wherever the terminal supports mode 2048, because it delivers geometry in the input
    /// stream with no signal handler and no `size()` round-trip (R-IN-8, design 01).
    ///
    /// [`resize_stream`]: Self::resize_stream
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write or flush the enable bytes.
    pub async fn enable_in_band_resize(&mut self) -> terminal::Result<()> {
        self.command(commands::terminal::enable_in_band_resize())
            .await?;
        self.flush().await?;
        self.session.record_in_band_resize_enabled();
        Ok(())
    }

    /// Enters the alternate screen buffer.
    ///
    /// Writes `CSI ? 1049 h` followed by an explicit `CSI 2 J` clear, flushes, and records the pair
    /// as one ledger entry's apply action so teardown (leave, drop, or the panic-safe emergency
    /// path) resets it with `CSI ? 1049 l`.
    ///
    /// The explicit clear after entry is deliberate (R-OUT-3, design 01): mosh does not clear the
    /// alternate buffer on 1049 the way most terminals do, and helix works around exactly this by
    /// clearing right after entering, so qwertty follows that evidence instead of trusting the
    /// terminal's own 1049 behavior.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write or flush the enter-and-clear bytes.
    pub async fn enter_alternate_screen(&mut self) -> terminal::Result<()> {
        self.command(commands::screen::enter_alternate_screen())
            .await?;
        self.command(commands::screen::clear()).await?;
        self.flush().await?;
        self.session.record_alternate_screen_entered();
        Ok(())
    }

    /// Hides the cursor.
    ///
    /// Writes `CSI ? 25 l`, flushes, and records a ledger entry whose undo shows the cursor again
    /// (`CSI ? 25 h`) on `leave`/drop/emergency (FM-L3). Hiding is the tracked state: a session
    /// that hides the cursor is guaranteed to show it again on every exit path.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write or flush the hide bytes.
    pub async fn hide_cursor(&mut self) -> terminal::Result<()> {
        self.command(commands::cursor::hide()).await?;
        self.flush().await?;
        self.session.record_cursor_hidden();
        Ok(())
    }

    /// Shows the cursor.
    ///
    /// Writes `CSI ? 25 h` immediately and flushes. Showing is not itself a ledger-tracked mode
    /// change — the visible cursor is the safe, default state, so there is nothing to undo on
    /// leave. Calling this after [`hide_cursor`](Self::hide_cursor) makes the cursor visible again
    /// right away; the hide entry recorded in the ledger remains, so a later `leave` writes one
    /// more redundant, harmless show.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write or flush the show bytes.
    pub async fn show_cursor(&mut self) -> terminal::Result<()> {
        self.command(commands::cursor::show()).await?;
        self.flush().await
    }

    /// Runs one typed query end to end against the correlator.
    ///
    /// The steps, in the order design 03 mandates:
    ///
    /// 1. **Cancel-sweep.** Resolve any still-registered [`active_query`](Self::active_query) — a
    ///    previous query's expectation that a dropped/cancelled future left behind — as
    ///    `Resolution::Cancelled`. This is the `&mut`-serialized cleanup that makes cancellation
    ///    synchronous: because only one caller holds `&mut self`, a leftover expectation is always
    ///    swept here before a new one registers, so a stale expectation can never misroute the new
    ///    query's reply. See [the cancel-sweep](#the-cancel-sweep) on the type.
    /// 2. **Register** the expectation and record its id in `active_query`.
    /// 3. **Write** the request bytes and flush.
    /// 4. **Drain-before-read.** Feed already-buffered pending events through the correlator before
    ///    any new read (design 03's drain-before-read rule): a reply that arrived interleaved with
    ///    earlier typeahead, already sitting in `pending`, must be able to complete the query. The
    ///    drain re-collects non-matching passthroughs back into `pending` in order.
    /// 5. **Deadline loop.** Await readiness under `timeout_at`; feed each read's events through
    ///    the correlator; timeout resolves the expectation as `Resolution::Timeout`; EOF resolves
    ///    it as `Resolution::Eof`.
    ///
    /// On completion the reply is taken from the correlator and `active_query` is cleared. A reply
    /// that arrives after a timeout is never claimed here — the expectation was removed at resolve
    /// time, so the correlator passes the late reply through as an ordinary event (rule 4), and it
    /// surfaces through [`next_event`](Self::next_event).
    async fn run_query(
        &mut self,
        expectation: Expectation,
        request: impl AsRef<Command>,
        operation: &'static str,
        timeout: Duration,
    ) -> terminal::Result<Reply> {
        // Step 1: sweep a leftover expectation from a dropped/cancelled prior query, and a leftover
        // probe bundle from a dropped/cancelled prior probe.
        self.sweep_active_query();
        self.sweep_active_probe();

        // Step 2: register. The M2 vocabulary never overlaps, and only one query runs at a time
        // (single `active_query`), so registration cannot conflict; a conflict would be a bug.
        let id = self
            .correlator
            .register(expectation)
            .expect("single in-flight query never conflicts with a swept expectation");
        self.active_query = Some(id);

        // Step 3: write the request and flush.
        self.command(request).await?;
        self.flush().await?;

        // Step 4: drain already-buffered events through the correlator before any read.
        if let Some(reply) = self.drain_pending_into_query(id) {
            self.active_query = None;
            return Ok(reply);
        }

        // Step 5: deadline loop.
        let deadline = Instant::now() + timeout;
        loop {
            let events = match timeout_at(deadline, self.read_events()).await {
                Ok(Ok(events)) => events,
                Ok(Err(err)) => {
                    // A read error (including EOF, surfaced below) ends the query. EOF resolves the
                    // expectation as Eof; any other read error still clears the expectation so the
                    // session stays consistent.
                    let resolution = if is_unexpected_eof(&err) {
                        Resolution::Eof
                    } else {
                        Resolution::Cancelled
                    };
                    self.correlator.resolve(id, resolution);
                    self.active_query = None;
                    return Err(err);
                }
                Err(_elapsed) => {
                    self.correlator.resolve(id, Resolution::Timeout);
                    self.active_query = None;
                    return Err(terminal::Error::query_timeout(operation, timeout));
                }
            };

            if let Some(reply) = self.feed_events_into_query(id, events) {
                self.active_query = None;
                return Ok(reply);
            }
        }
    }

    /// Sweeps a leftover [`active_query`](Self::active_query) expectation as cancelled.
    ///
    /// If a previous query future was dropped mid-await, its expectation is still registered on the
    /// correlator and its id still in `active_query`. Resolving it `Resolution::Cancelled`
    /// removes it, so a later matching reply passes through as an event (rule 4) rather than
    /// being misdelivered to a new query. Synchronous and idempotent: an already-resolved id is
    /// a no-op.
    fn sweep_active_query(&mut self) {
        if let Some(id) = self.active_query.take() {
            self.correlator.resolve(id, Resolution::Cancelled);
        }
    }

    /// Feeds every buffered pending event through the correlator, watching for the query reply.
    ///
    /// Non-matching passthroughs are collected back into `pending` in their original order; a
    /// completion for `id` short-circuits and returns the taken reply, leaving the remaining
    /// undrained events in place ahead of the ones already re-collected — order is preserved
    /// because the drain processes `pending` front to back and re-appends passthroughs in that
    /// same order.
    fn drain_pending_into_query(&mut self, id: ExpectationId) -> Option<Reply> {
        let buffered: Vec<Event> = self.pending.drain(..).collect();
        let mut restored = VecDeque::with_capacity(buffered.len());
        let mut reply = None;

        let mut iter = buffered.into_iter();
        for event in iter.by_ref() {
            match self.correlator.feed(event) {
                Feed::Completed { id: completed, .. } if completed == id => {
                    reply = self.correlator.take_reply(id);
                    break;
                }
                Feed::Completed { .. } => {
                    // A completion for some other (impossible with one in-flight query)
                    // expectation: there is nothing to deliver, so drop it.
                    // This arm is defensive; the single active query means only
                    // `id` can complete here.
                }
                Feed::Passthrough(event) => restored.push_back(event),
            }
        }
        // Any events after the completed one were never fed; keep them buffered in order behind the
        // ones we re-collected.
        for event in iter {
            restored.push_back(event);
        }
        self.pending = restored;
        reply
    }

    /// Feeds a freshly read batch of events through the correlator, watching for the query reply.
    ///
    /// Passthroughs are buffered into `pending` in arrival order. On the completion of `id` the
    /// remaining events in the batch stay buffered behind the passthroughs already collected, and
    /// the taken reply is returned.
    fn feed_events_into_query(&mut self, id: ExpectationId, events: Vec<Event>) -> Option<Reply> {
        let mut reply = None;
        let mut iter = events.into_iter();
        for event in iter.by_ref() {
            match self.correlator.feed(event) {
                Feed::Completed { id: completed, .. } if completed == id => {
                    reply = self.correlator.take_reply(id);
                    break;
                }
                Feed::Completed { .. } => {}
                Feed::Passthrough(event) => self.pending.push_back(event),
            }
        }
        for event in iter {
            self.pending.push_back(event);
        }
        reply
    }

    /// Buffers a batch of decoded events through the correlator, appending passthroughs to
    /// `pending`.
    ///
    /// With no query registered every event is a passthrough, which is the ordinary
    /// [`next_event`](Self::next_event) path. A completion here (a reply for a coalesced/held
    /// expectation with no live waiter) is dropped: no waiter is asking for it.
    fn buffer_events(&mut self, events: Vec<Event>) {
        for event in events {
            match self.correlator.feed(event) {
                Feed::Passthrough(event) => self.pending.push_back(event),
                Feed::Completed { .. } => {}
            }
        }
    }

    /// Awaits readiness, performs one operating-system read, and decodes it into events.
    ///
    /// Returns [`terminal::Error::ReadTerminal`] with an `UnexpectedEof` source when the terminal
    /// closes (a zero-length read). Cancel-safe: no decoder state is lost on a cancelled await
    /// because the decoder lives on the session and only advances on a completed read.
    async fn read_events(&mut self) -> terminal::Result<Vec<Event>> {
        loop {
            let mut guard = self
                .readiness
                .readable()
                .await
                .map_err(terminal::Error::read_terminal)?;

            let mut buffer = [0; READ_BUFFER_LEN];
            let read = guard.try_io(|inner| fd_read(inner.get_ref(), &mut buffer));
            match read {
                Ok(Ok(0)) => {
                    return Err(terminal::Error::read_terminal(io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "terminal input closed before another event was available",
                    )));
                }
                Ok(Ok(len)) => {
                    let mut events = self.decoder.feed(&buffer[..len]);
                    // Drain-boundary flush: a read that did not fill the buffer means the operating
                    // system's input buffer is drained, so a trailing text run the syntax layer
                    // parked for split-equivalence is settled input the caller should receive now.
                    // Only *complete* trailing text is flushed; a partial escape, control sequence,
                    // or mid-character UTF-8 run keeps waiting for the bytes that finish it (design
                    // 02: the decoder never guesses across a real split). Without this, the last
                    // character typed before a pause — the `o` in "hello" — would sit unseen until
                    // the next keystroke, which the real-emulator typeahead gate would catch.
                    if len < buffer.len() && self.decoder.has_settled_text() {
                        events.extend(self.decoder.finish());
                    }
                    return Ok(events);
                }
                Ok(Err(err)) => return Err(terminal::Error::read_terminal(err)),
                Err(_would_block) => {}
            }
        }
    }

    /// Flushes buffered terminal output.
    ///
    /// Call this when the preceding command, byte, and text writes must be visible before later
    /// application work continues.
    ///
    /// Writes go straight to the terminal descriptor (through the readiness-registered fd, which
    /// shares its open file description with the device), so there is no library-side buffer to
    /// drain — this method is a synchronous success once the writes above have completed. It stays
    /// an `async fn` for API continuity with the awaited call sites and to leave room for a
    /// buffered write path in a later slice.
    ///
    /// # Errors
    ///
    /// Never returns an error today; the `Result` shape is kept for forward compatibility.
    #[allow(
        clippy::unused_async,
        reason = "raw-fd writes are unbuffered so there is nothing to flush; the async shape is \
                  kept for API continuity with the awaited call sites"
    )]
    pub async fn flush(&mut self) -> terminal::Result<()> {
        Ok(())
    }

    /// Leaves the session and restores cooked mode.
    ///
    /// This is the orderly cleanup path. It replays the composed session's mode ledger — raw-mode
    /// restoration, the input-mode enables, alternate screen, and cursor visibility — and restores
    /// the device status flags captured at construction, reporting terminal-mode restoration errors
    /// to the caller. Teardown never routes through `spawn_blocking` (design 04 amendment): the
    /// ledger replay is synchronous and does not block.
    ///
    /// It does not flush pending output or clean up protocol state such as graphics, clipboard, or
    /// vendor extensions. Call [`flush`](Self::flush) before `leave` when output visibility
    /// matters. Drop still attempts best-effort restoration, but drop-time failures cannot be
    /// returned.
    ///
    /// # Errors
    ///
    /// Returns an error when cooked mode cannot be restored.
    #[allow(
        clippy::unused_async,
        reason = "teardown is synchronous (design 04 forbids spawn_blocking here), but leave stays \
                  an async fn for API continuity with the awaited call sites"
    )]
    pub async fn leave(mut self) -> terminal::Result<()> {
        self.restore_flags();
        self.session.leave()
    }

    /// Returns an awaitable [`ResizeStream`] that yields a synthetic resize on every `SIGWINCH`.
    ///
    /// This is the **fallback** resize source, for terminals that do not support in-band resize
    /// (mode 2048). Prefer [`enable_in_band_resize`](Self::enable_in_band_resize) wherever it is
    /// available: in-band resize delivers geometry (including pixels) in the input stream through
    /// [`next_event`](Self::next_event) with no signal handling at all, and it coalesces storms.
    ///
    /// The stream is deliberately **thin and independent**: qwertty installs no signal handler of
    /// its own (design 01). It owns a Tokio [`SignalKind::window_change`] listener and a private
    /// duplicate of the terminal descriptor; on each `SIGWINCH` it reads the current size with an
    /// `ioctl` and yields a cell-only [`ResizeEvent`] (a `SIGWINCH` carries no pixel geometry, so
    /// [`ResizeEvent::pixels`] is `None`). Because it does not borrow the session, an application
    /// can `select!` it alongside [`next_event`](Self::next_event):
    ///
    /// ```no_run
    /// # async fn run() -> qwertty::Result<()> {
    /// use qwertty::{Event, TokioTerminalSession};
    ///
    /// let mut session = TokioTerminalSession::open()?;
    /// let mut resizes = session.resize_stream()?;
    /// loop {
    ///     tokio::select! {
    ///         event = session.next_event() => { let _event: Event = event?; }
    ///         resize = resizes.next_resize() => {
    ///             let resize = resize?;
    ///             let _ = resize.cells();
    ///         }
    ///     }
    /// }
    /// # }
    /// ```
    ///
    /// Coalescing note: unlike the in-band path, the `SIGWINCH` fallback relies on `SIGWINCH`'s own
    /// signal coalescing plus the application's read cadence; a burst of size changes between two
    /// `next_resize()` awaits collapses to one signal delivery reporting the final size, so the
    /// stream naturally yields the latest geometry rather than every intermediate one.
    ///
    /// # Errors
    ///
    /// Returns an error when the `SIGWINCH` listener cannot be installed or the descriptor cannot
    /// be duplicated for size reads.
    pub fn resize_stream(&self) -> terminal::Result<ResizeStream> {
        let borrowed = self.session.device().as_fd().ok_or_else(|| {
            terminal::Error::unsupported("SIGWINCH resize stream", "device without a fd")
        })?;
        let size_fd = rustix::io::dup(borrowed)
            .map_err(io::Error::from)
            .map_err(terminal::Error::open_terminal)?;
        let signal = signal(SignalKind::window_change()).map_err(terminal::Error::read_terminal)?;
        Ok(ResizeStream { signal, size_fd })
    }

    /// Restores the device status flags captured before this session set the descriptor
    /// non-blocking.
    ///
    /// The readiness dup and the session device share one open file description, so restoring the
    /// flags on either restores them for both. This runs before the session teardown and again from
    /// drop, so every exit path puts the flags back (idempotent; a redundant set is harmless).
    fn restore_flags(&self) {
        // Restore on the shared description via the readiness dup, which is guaranteed open here.
        _ = fcntl_setfl(self.readiness.get_ref(), self.original_flags);
    }
}

impl TokioTerminalSession<Terminal> {
    /// Returns the path used to open the live terminal device.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.session.device().path()
    }
}

impl<D: TerminalDevice> Drop for TokioTerminalSession<D> {
    fn drop(&mut self) {
        // Restore the shared-description status flags before the session's own drop restores cooked
        // mode; with a dup'd stdin description the non-blocking flag would otherwise leak into the
        // parent shell (FM-L class). The session's Drop handles cooked-mode restoration.
        self.restore_flags();
    }
}

/// An awaitable `SIGWINCH`-driven resize source — the fallback for terminals without mode 2048.
///
/// Obtain one from [`TokioTerminalSession::resize_stream`]. It is an independent value that does
/// not borrow the session (design 01: qwertty installs no handler itself, only exposes a stream the
/// app selects on), so it can sit in a `tokio::select!` alongside
/// [`next_event`](TokioTerminalSession::next_event). It holds a Tokio `SIGWINCH` listener and a
/// private duplicate of the terminal descriptor used to read the new size.
///
/// # Shape choice
///
/// This is a small helper type with an `async fn` [`next_resize`](Self::next_resize) rather than a
/// full `futures::Stream` implementation. The awaitable-method shape keeps the type dependency-free
/// (no `futures`/`Stream` in the public API before the vocabulary freeze) and is all a `select!`
/// loop needs; a `Stream` impl can be added later without changing this method (design 04). Prefer
/// in-band resize (mode 2048) where the terminal supports it — this is the fallback.
#[derive(Debug)]
pub struct ResizeStream {
    /// The Tokio `SIGWINCH` (`SIGWINCH` = window change) listener. Tokio owns the actual signal
    /// registration; qwertty installs no handler of its own.
    signal: Signal,
    /// A private duplicate of the terminal descriptor, used only for the `tcgetwinsize` size read.
    ///
    /// A dup shares the open file description, so the size it measures is the session's terminal
    /// size; keeping a separate owned fd is what lets this stream avoid borrowing the session.
    size_fd: OwnedFd,
}

impl ResizeStream {
    /// Awaits the next `SIGWINCH` and yields the terminal's new size as a [`ResizeEvent`].
    ///
    /// On each `SIGWINCH` this reads the current size with a `tcgetwinsize` `ioctl` on its private
    /// descriptor and returns a **cell-only** resize event: a `SIGWINCH` carries no pixel geometry,
    /// so [`ResizeEvent::pixels`] is `None`. Because Tokio coalesces pending `SIGWINCH` deliveries,
    /// a burst of size changes between two awaits yields one event reporting the final size.
    ///
    /// Cancel-safe: dropping the future mid-await abandons only the wait; the listener and
    /// descriptor live on this value, so the next call resumes cleanly.
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::GetTerminalSize`] when the size `ioctl` fails, or a read error if
    /// the signal stream closes (which does not happen for `SIGWINCH` in normal operation).
    pub async fn next_resize(&mut self) -> terminal::Result<ResizeEvent> {
        match self.signal.recv().await {
            Some(()) => {
                let size = rustix::termios::tcgetwinsize(&self.size_fd)
                    .map_err(io::Error::from)
                    .map_err(terminal::Error::get_terminal_size)?;
                let cells = TerminalSize::new(size.ws_col, size.ws_row);
                Ok(ResizeEvent::new(cells, None))
            }
            None => Err(terminal::Error::read_terminal(io::Error::new(
                ErrorKind::UnexpectedEof,
                "SIGWINCH signal stream closed",
            ))),
        }
    }
}

/// Writes bytes to the readiness-registered descriptor with one `write(2)`, returning the count.
///
/// I/O runs on the fd Tokio registered — the dup that shares the device's open file description —
/// so readiness stays correct under edge-triggered polling and the bytes are still the device's
/// bytes. On the non-blocking descriptor a short write advances the caller's remaining slice, and a
/// `WouldBlock` surfaces as an error so `try_io` clears the readiness guard and the caller retries
/// on the next writable notification. This is the exact partial-write semantics of the old
/// direct-`File::write` loop.
fn fd_write(fd: &OwnedFd, bytes: &[u8]) -> io::Result<usize> {
    rustix::io::write(fd, bytes).map_err(io::Error::from)
}

/// Reads into `buffer` from the readiness-registered descriptor with one `read(2)`.
///
/// Returns `Ok(0)` at end of input. Runs on the registered fd for the same readiness-correctness
/// reason as [`fd_write`].
fn fd_read(fd: &OwnedFd, buffer: &mut [u8]) -> io::Result<usize> {
    rustix::io::read(fd, buffer).map_err(io::Error::from)
}

/// Returns whether a terminal error is a read error whose source is `UnexpectedEof`.
fn is_unexpected_eof(error: &terminal::Error) -> bool {
    matches!(
        error,
        terminal::Error::ReadTerminal { source } if source.kind() == ErrorKind::UnexpectedEof
    )
}

/// Returns whether an event is a resize (the only coalesced event kind — design 01 §resize).
fn is_resize(event: &Event) -> bool {
    matches!(event, Event::Resize(_))
}

/// Pops the next event from a pending queue, applying resize coalescing (design 01 §resize, FM-G2).
///
/// A front resize is dropped whenever a later resize is still queued behind it, so a resize storm
/// collapses to the burst's last resize — carrying the final geometry, in that resize's position —
/// while every non-resize event keeps its order and identity. This is the ordering invariant, and
/// the never-coalesce mouse/scroll policy (FM-V6) falls out of it: only resize events are ever the
/// event this rule drops. Returns `None` only when the queue is empty.
fn take_coalesced_event(pending: &mut VecDeque<Event>) -> Option<Event> {
    while let Some(event) = pending.pop_front() {
        if is_resize(&event) && pending.iter().any(is_resize) {
            continue;
        }
        return Some(event);
    }
    None
}

/// The registered expectation ids of one capability probe bundle (design 03).
///
/// Keyed for the fence and for reply collection: `fence` is the DA1 expectation whose completion
/// resolves the rest as no-reply; the others are paired with the [`Capabilities`] field their reply
/// fills. Every id is also mirrored in the session's `active_probe` for the cancel-sweep.
#[derive(Default)]
struct ProbeBundle {
    fence: Option<ExpectationId>,
    xtversion: Option<ExpectationId>,
    kitty: Option<ExpectationId>,
    foreground: Option<ExpectationId>,
    background: Option<ExpectationId>,
    modes: Vec<(ExpectationId, CapabilityField)>,
}

impl ProbeBundle {
    /// Returns every registered id in the bundle (fence included), for whole-bundle resolution.
    fn ids(&self) -> Vec<ExpectationId> {
        let mut ids = Vec::new();
        ids.extend(self.fence);
        ids.extend(self.xtversion);
        ids.extend(self.kitty);
        ids.extend(self.foreground);
        ids.extend(self.background);
        ids.extend(self.modes.iter().map(|(id, _)| *id));
        ids
    }
}

/// The evidence label recorded on every DECRQM-backed [`Finding`] the probe bundle populates.
///
/// One stable string per mode number so a consumer's `Evidence::Probed { via }` match names the
/// exact query that answered (design 06).
const fn decrqm_evidence(mode: u16) -> &'static str {
    match mode {
        2026 => "DECRQM 2026",
        2027 => "DECRQM 2027",
        2048 => "DECRQM 2048",
        2004 => "DECRQM 2004",
        _ => "DECRQM",
    }
}

/// Records one bundle reply into the matching [`Capabilities`] field, as a [`Finding`] with
/// [`Evidence::Probed`] naming the query that answered.
///
/// The XTVERSION reply also feeds `capabilities.identity` (design 06, R-CAP-5: identity is a
/// finding too) via [`identity_from_env`], cross-checked against the environment.
fn store_bundle_reply(
    bundle: &ProbeBundle,
    id: ExpectationId,
    reply: Reply,
    capabilities: &mut Capabilities,
) {
    match reply {
        Reply::XtVersion(report) => {
            let version = report.version().to_owned();
            capabilities.identity = identity_from_env(Some(&version), std_env_source);
        }
        Reply::KittyKeyboardFlags(bits) => {
            capabilities.kitty_keyboard =
                Finding::probed(Some(KittyKeyboardFlags::from_bits(bits)), "CSI ?u");
        }
        Reply::OscColor(report) => match report.kind() {
            OscColorKind::Foreground => {
                capabilities.foreground_color = Finding::probed(Some(report.rgb()), "OSC 10");
            }
            OscColorKind::Background => {
                capabilities.background_color = Finding::probed(Some(report.rgb()), "OSC 11");
            }
        },
        Reply::DecPrivateMode(report) => store_mode_reply(bundle, id, report, capabilities),
        Reply::PrimaryDeviceAttributes(attrs) => {
            capabilities.primary_device_attributes = Some(attrs.into());
        }
        // The bundle never registers CursorPosition/TerminalStatus expectations, so those reply
        // variants cannot appear here.
        Reply::CursorPosition(_) | Reply::TerminalStatus(_) => {}
    }
}

/// Stores a DECRQM answer into the [`Capabilities`] finding its mode maps to (via the bundle), as
/// [`Evidence::Probed`] naming the exact mode queried.
///
/// The mode's enabled/reset/permanently-* state becomes a `Some(true)`/`Some(false)` finding value;
/// a "not recognized" (value 0) answer leaves the finding's value `None` but its evidence is still
/// `Probed` — the terminal *did* answer, just in the negative-unknown way DECRQM allows (FM-C4).
/// The bundle maps the completing expectation id back to which of the four fields it fills.
fn store_mode_reply(
    bundle: &ProbeBundle,
    id: ExpectationId,
    report: DecPrivateModeReport,
    capabilities: &mut Capabilities,
) {
    let Some((_, field)) = bundle.modes.iter().find(|(mode_id, _)| *mode_id == id) else {
        return;
    };
    let enabled = report.is_enabled();
    let evidence_via = decrqm_evidence(report.mode());
    let finding = Finding::probed(enabled, evidence_via);
    match field {
        CapabilityField::SynchronizedOutput => capabilities.synchronized_output = finding,
        CapabilityField::GraphemeClustering => capabilities.grapheme_clustering = finding,
        CapabilityField::InBandResize => capabilities.in_band_resize = finding,
        CapabilityField::BracketedPaste => capabilities.bracketed_paste = finding,
    }
}

/// Builds the error for the impossible "wrong reply type completed a typed query" case.
///
/// The correlator only completes an `Expectation::CursorPosition` with a
/// `Reply::CursorPosition` and an `Expectation::TerminalStatus` with a
/// `Reply::TerminalStatus`, so this never fires; it exists so the typed helpers stay total
/// without an `unreachable!`.
fn unexpected_reply(_reply: Reply) -> terminal::Error {
    terminal::Error::read_terminal(io::Error::new(
        ErrorKind::InvalidData,
        "query completed with an unexpected reply type",
    ))
}

/// Reaches the controlling terminal through the inherited standard-input descriptor.
///
/// On macOS, kqueue rejects a *freshly opened* descriptor for the process's own controlling
/// terminal with `EINVAL` — both through the `/dev/tty` alias and through the underlying device
/// path — while the descriptor inherited as standard input registers fine (verified empirically;
/// this is the incumbent failure class the Phase 1 catalog records for crossterm's dev-tty path on
/// macOS, FM-A11). Duplicating standard input shares its open file description, so the duplicate
/// stays pollable. Because the description is shared with the parent shell's standard input, the
/// session's non-blocking flag would leak into the shell on exit; the Tokio session therefore
/// captures the original status flags from the readiness dup and restores them on leave and on
/// drop.
///
/// The duplicate is only usable when standard input is a terminal opened read-write, which is how
/// interactive shells set up their children. Otherwise (redirected stdin, read-only fd 0) the
/// caller falls back to opening `/dev/tty`, which remains correct on platforms whose pollers accept
/// it.
/// Resolves the controlling terminal to its specific device path for a fresh open.
///
/// When standard input cannot supply the terminal (redirected, or not read-write), a fresh open
/// is required. The `/dev/tty` alias is never pollable through kqueue on macOS, but the specific
/// device path (for example `/dev/ttys003`) is in ordinary pseudoterminals, so the alias is
/// opened briefly only to ask the kernel for the real name. The alias itself is the last resort,
/// which remains correct on platforms whose pollers accept it. Known residual: inside tmux panes
/// even the specific path is not freshly pollable, so redirected-stdin sessions under tmux still
/// fail at registration (FM-A11).
fn resolved_controlling_terminal_path() -> PathBuf {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(DEV_TTY)
        .ok()
        .and_then(|device| rustix::termios::ttyname(&device, Vec::new()).ok())
        .map_or_else(
            || PathBuf::from(DEV_TTY),
            |name| PathBuf::from(OsString::from_vec(name.into_bytes())),
        )
}

fn controlling_terminal_via_stdin() -> Option<(File, PathBuf)> {
    let stdin = rustix::stdio::stdin();
    if !rustix::termios::isatty(stdin) {
        return None;
    }

    let flags = fcntl_getfl(stdin).ok()?;
    if flags & OFlags::ACCMODE != OFlags::RDWR {
        return None;
    }

    let path = rustix::termios::ttyname(stdin, Vec::new())
        .ok()
        .map_or_else(
            || PathBuf::from(DEV_TTY),
            |name| PathBuf::from(OsString::from_vec(name.into_bytes())),
        );
    let device = File::from(rustix::io::dup(stdin).ok()?);
    Some((device, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ResizeEvent;
    use crate::{Key, KeyEvent, TerminalSize};

    /// A resize event with the given column geometry (rows fixed), for ordering assertions.
    fn resize(cols: u16) -> Event {
        Event::Resize(ResizeEvent::new(TerminalSize::new(cols, 24), None))
    }

    /// A key event carrying a single character, for ordering assertions.
    fn key(character: char) -> Event {
        Event::Key(KeyEvent::new(Key::Char(character)))
    }

    /// Drains a queue through the coalescing rule into the delivered sequence.
    fn drain(mut queue: VecDeque<Event>) -> Vec<Event> {
        let mut delivered = Vec::new();
        while let Some(event) = take_coalesced_event(&mut queue) {
            delivered.push(event);
        }
        delivered
    }

    #[test]
    fn a_resize_storm_collapses_to_the_last_geometry() {
        let queue = VecDeque::from(vec![resize(80), resize(85), resize(90), resize(100)]);
        assert_eq!(drain(queue), vec![resize(100)]);
    }

    #[test]
    fn interleaved_keys_keep_order_and_the_last_resize_survives_in_place() {
        // R1 a R2 b R3 -> a b R3: keys in order, one resize (final geometry) in R3's position.
        let queue = VecDeque::from(vec![resize(80), key('a'), resize(85), key('b'), resize(90)]);
        assert_eq!(drain(queue), vec![key('a'), key('b'), resize(90)]);
    }

    #[test]
    fn a_lone_resize_passes_through_unchanged() {
        let queue = VecDeque::from(vec![key('a'), resize(80), key('b')]);
        assert_eq!(drain(queue), vec![key('a'), resize(80), key('b')]);
    }

    #[test]
    fn a_trailing_resize_after_keys_survives() {
        // The surviving resize can be the last event overall; nothing after it forces its position.
        let queue = VecDeque::from(vec![key('a'), resize(70), resize(80)]);
        assert_eq!(drain(queue), vec![key('a'), resize(80)]);
    }

    #[test]
    fn non_resize_events_are_never_coalesced() {
        // A run of identical key events (stand-ins for scroll ticks) is delivered whole (FM-V6).
        let queue = VecDeque::from(vec![key('x'), key('x'), key('x')]);
        assert_eq!(drain(queue), vec![key('x'), key('x'), key('x')]);
    }

    #[test]
    fn an_empty_queue_yields_nothing() {
        assert_eq!(drain(VecDeque::new()), Vec::<Event>::new());
    }
}
