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
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::process::{Pid, Signal as ProcessSignal, getpgrp, getsid, kill_process_group};
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

/// How many times [`resume`](TokioTerminalSession::resume) retries the ledger re-enter before
/// giving up, tolerating the shell racing the returning process for the terminal (FM-G4). Ten tries
/// at [`RESUME_REENTER_RETRY_DELAY`] apart is the helix/neovim pattern (~half a second total).
const RESUME_REENTER_RETRIES: u32 = 10;
/// How long [`resume`](TokioTerminalSession::resume) waits between re-enter attempts.
const RESUME_REENTER_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Which of the controlling-terminal fallback branches produced a session's device.
///
/// [`open`](TokioTerminalSession::open) reaches the controlling terminal through a three-branch
/// fallback (see that method); every branch yields a working device, so the choice is normally
/// invisible. This enum makes the outcome observable: a caller can read
/// [`acquisition`](TokioTerminalSession::acquisition) to log which branch won or surface it in a
/// status view, without having to reconstruct the decision itself. It is deliberately *not* meant
/// for branching on — it records what happened, it does not steer what happens next.
///
/// This type is available when the `tokio` feature is enabled, on unix, alongside
/// [`TokioTerminalSession`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TerminalAcquisition {
    /// The session duplicated the inherited read-write standard-input descriptor.
    ///
    /// This is the robust primary and the branch [`open`](TokioTerminalSession::open) prefers
    /// whenever standard input is a terminal opened read-write (how interactive shells set up their
    /// children). It matters on macOS: kqueue rejects a *freshly opened* controlling-terminal
    /// descriptor with `EINVAL`, while the inherited one registers fine, so duplicating the shared
    /// open file description keeps readiness pollable (FM-A11). Because the description is
    /// inherited, this branch is what makes sessions work under tmux, where a fresh open is not
    /// pollable.
    InheritedStdin,

    /// The session opened the resolved specific device path fresh (for example `/dev/ttys003`).
    ///
    /// Reached when standard input cannot supply the terminal (redirected or read-only, the
    /// fzf-style case where a tool runs with stdin piped from another program). The `/dev/tty`
    /// alias is opened only long enough to ask the kernel for the real device name, then that
    /// specific path is opened afresh — it is pollable through kqueue on macOS where the alias
    /// is not. This is also the branch recorded when a caller opens an explicit path via
    /// [`open_path`](TokioTerminalSession::open_path).
    ResolvedDevicePath,

    /// The session fell back to opening the `/dev/tty` alias.
    ///
    /// The last resort, used when the specific device path could not be resolved. It remains
    /// correct on platforms whose pollers accept the alias; on macOS a freshly opened alias is
    /// not pollable, so this branch marks the least-robust outcome.
    DevTtyAlias,
}

impl fmt::Display for TerminalAcquisition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let phrase = match self {
            Self::InheritedStdin => "duplicated inherited stdin",
            Self::ResolvedDevicePath => "opened resolved device path",
            Self::DevTtyAlias => "opened /dev/tty alias",
        };
        f.write_str(phrase)
    }
}

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
    /// The capability snapshot from the most recent
    /// [`probe_capabilities`](Self::probe_capabilities), or `None` before any probe (FM-C8: the
    /// snapshot is per-attachment and lives on the session).
    ///
    /// Emit-gating reads this: [`synchronized`](Self::synchronized) wraps a frame in mode 2026
    /// only when this snapshot's [`synchronized_output`](Capabilities::synchronized_output) is
    /// a known `true` finding (R-CAP-4, FM-V4). `None` here — never probed — is treated
    /// exactly like an unknown finding: the gate degrades rather than emitting into a terminal
    /// that never answered.
    capabilities: Option<Capabilities>,
    /// Which controlling-terminal fallback branch produced this session's device, when it was
    /// opened from a live terminal.
    ///
    /// Set on every construction path that opens a terminal ([`open`](Self::open) and
    /// [`open_path`](Self::open_path)). It is `None` for [`from_device`](Self::from_device), which
    /// wraps an already-opened device (such as `FakeDevice`) rather than reaching for the
    /// controlling terminal, so no fallback branch was taken. Read-only observability; exposed
    /// through [`acquisition`](Self::acquisition).
    acquisition: Option<TerminalAcquisition>,
    /// The injectable job-control stop-signal seam used by [`suspend`](Self::suspend).
    ///
    /// This is the sans-io seam that keeps [`suspend`](Self::suspend) testable without stopping
    /// the test process. The default is [`send_real_stop_signal`], which checks the process
    /// group defensively (FM-G7) and then sends `SIGTSTP` to the whole group so the shell
    /// resumes cleanly. Tests swap in a stub that records the call and returns `Ok(())`, so
    /// every mechanic around the signal — ledger undo, restore-handle disarm, flags/mode
    /// resync, synthetic resize — runs while no real `SIGTSTP` is ever delivered to the runner
    /// (a CI-safety requirement, not just a test convenience). The closure returns a typed
    /// error for a degenerate process group.
    stop_signal: StopSignal,
    /// How long [`next_event`](Self::next_event) waits on a lone pending `ESC` before flushing it
    /// as [`Key::Escape`](crate::Key::Escape), or `None` to never flush on a timeout.
    ///
    /// A bare `ESC` (`0x1b`) byte is held pending by the decoder because it may begin an escape
    /// sequence (an arrow key, a CSI, an OSC). Without a timeout, a standalone Esc keypress would
    /// not surface until more input arrived (possibly completing a sequence) or EOF — blocking
    /// Esc-to-cancel in a TUI. When this is `Some(d)` and the decoder's only pending state is a
    /// lone `ESC`, `next_event` bounds the next read at `d`: input arriving first is decoded
    /// normally (it may complete a real sequence), and on elapse the lone `ESC` is flushed as
    /// `Key::Escape`.
    ///
    /// The default is `Some(25ms)` — a window small enough to feel instant for a human Esc yet
    /// comfortably wider than the inter-byte gap of a real escape sequence on any transport.
    /// `None` restores the wait-for-more-or-EOF behaviour. This is inert under kitty's
    /// disambiguate-escape-codes flag, where Escape arrives as `CSI 27 u` rather than a bare
    /// `0x1b`.
    esc_flush_timeout: Option<Duration>,
}

/// The default [`esc_flush_timeout`](TokioTerminalSession::esc_flush_timeout): flush a lone pending
/// `ESC` as [`Key::Escape`](crate::Key::Escape) `25ms` after the last byte, unless more input
/// arrives first.
const DEFAULT_ESC_FLUSH_TIMEOUT: Duration = Duration::from_millis(25);

/// The injectable job-control stop-signal seam a [`TokioTerminalSession`] holds.
///
/// Boxing the send behind a trait object is what makes [`suspend`](TokioTerminalSession::suspend)
/// sans-io: the real send (`SIGTSTP` to the process group) is one implementation, and a test stub
/// that records the call without signalling is another. The closure performs the whole
/// check-then-send so the FM-G7 process-group guard and the raw `kill` stay behind the same seam.
/// The wrapper carries a manual [`fmt::Debug`] so the session can keep deriving `Debug` even though
/// a boxed closure is not itself `Debug`.
struct StopSignal(Box<dyn FnMut() -> terminal::Result<()> + Send>);

impl StopSignal {
    /// Invokes the seam: checks the process group (FM-G7) and sends the stop signal, or records the
    /// call in a test stub.
    fn send(&mut self) -> terminal::Result<()> {
        (self.0)()
    }
}

impl fmt::Debug for StopSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StopSignal(..)")
    }
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
        if let Some((device, path)) = controlling_terminal_via_stdin() {
            let terminal = Terminal::from_file(device, path)?;
            Self::from_terminal(terminal, TerminalAcquisition::InheritedStdin)
        } else {
            let (path, acquisition) = resolved_controlling_terminal_path();
            let terminal = Terminal::open_path(path)?;
            Self::from_terminal(terminal, acquisition)
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
        Self::from_terminal(terminal, TerminalAcquisition::ResolvedDevicePath)
    }

    /// Builds a Tokio-backed session from an already-opened terminal.
    ///
    /// `acquisition` records which controlling-terminal fallback branch produced `terminal`, so the
    /// session can report it through [`acquisition`](Self::acquisition).
    fn from_terminal(
        terminal: Terminal,
        acquisition: TerminalAcquisition,
    ) -> terminal::Result<Self> {
        let session = TerminalSession::from_terminal(terminal)?;
        Self::from_session(session, Some(acquisition))
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
        // No controlling-terminal fallback runs here: the device is already open, so there is no
        // acquisition branch to record. `acquisition()` therefore returns `None` for this path.
        Self::from_session(session, None)
    }

    /// Reports which controlling-terminal fallback branch produced this session's device.
    ///
    /// [`open`](Self::open) reaches the controlling terminal through a three-branch fallback and
    /// every branch yields a working session, so which one won is otherwise invisible. This
    /// accessor makes that outcome observable — a caller can log it or show it in a status view
    /// — without having to reconstruct the decision. It is read-only observability, not a
    /// branching control: see [`TerminalAcquisition`] for the variants and why each branch
    /// exists.
    ///
    /// Returns `None` for sessions built with [`from_device`](Self::from_device), which wrap an
    /// already-opened device (such as `FakeDevice`) and so run no fallback and take no branch.
    #[must_use]
    pub fn acquisition(&self) -> Option<TerminalAcquisition> {
        self.acquisition
    }

    /// Wraps an entered [`TerminalSession`] with the readiness registration and sans-io core.
    ///
    /// This duplicates the device descriptor for Tokio readiness (a dup shares the same open file
    /// description, so readiness is shared), captures the original status flags, sets the dup
    /// non-blocking, and registers it with the current runtime.
    ///
    /// `acquisition` is the fallback branch that produced the device, or `None` when the device was
    /// supplied already open (the [`from_device`](Self::from_device) path).
    fn from_session(
        session: TerminalSession<D>,
        acquisition: Option<TerminalAcquisition>,
    ) -> terminal::Result<Self> {
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
            capabilities: None,
            acquisition,
            stop_signal: StopSignal(Box::new(send_real_stop_signal)),
            esc_flush_timeout: Some(DEFAULT_ESC_FLUSH_TIMEOUT),
        })
    }

    /// Replaces the job-control stop-signal seam with a test stub (unit tests only).
    ///
    /// This is how the suspend/resume unit tests exercise every mechanic — ledger undo, restore
    /// disarm, flags/mode resync, synthetic resize — while `SIGTSTP` is stubbed out, so no test
    /// ever stops the test runner (a CI-safety requirement). It is not part of the public API.
    #[cfg(test)]
    fn with_stop_signal(
        mut self,
        stop_signal: impl FnMut() -> terminal::Result<()> + Send + 'static,
    ) -> Self {
        self.stop_signal = StopSignal(Box::new(stop_signal));
        self
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

    /// Returns the lone-Escape flush timeout for [`next_event`](Self::next_event).
    ///
    /// `Some(d)` means a lone pending `ESC` is flushed as [`Key::Escape`](crate::Key::Escape) `d`
    /// after the last byte unless more input arrives first; `None` means it is never flushed on
    /// a timeout (it waits for more input or EOF). The default is `Some(25ms)`. See
    /// [`set_esc_flush_timeout`](Self::set_esc_flush_timeout) for the full policy.
    #[must_use]
    pub fn esc_flush_timeout(&self) -> Option<Duration> {
        self.esc_flush_timeout
    }

    /// Sets the lone-Escape flush timeout for [`next_event`](Self::next_event).
    ///
    /// A bare `ESC` (`0x1b`) is held pending by the decoder because it may begin an escape sequence
    /// (an arrow key, a CSI, an OSC), so a standalone Esc keypress does not otherwise surface until
    /// more input arrives or the stream ends. With `Some(d)`, when the decoder's only pending state
    /// is a lone `ESC`, `next_event` bounds its next read at `d`: bytes arriving before the
    /// deadline are decoded normally (they may complete a real sequence, in which case no bare
    /// Escape is produced), and on elapse the lone `ESC` is flushed as
    /// [`Key::Escape`](crate::Key::Escape). A non-lone-ESC pending state (a partial CSI/OSC or
    /// a mid-character UTF-8 run) is never flushed this way — it keeps waiting for the bytes
    /// that finish it. `None` disables the timeout entirely, restoring the wait-for-more-or-EOF
    /// behaviour.
    ///
    /// The default is `Some(25ms)`: small enough to feel instant for a human Esc, yet comfortably
    /// wider than the inter-byte gap of a real escape sequence on any transport. This is inert
    /// under kitty's disambiguate-escape-codes flag, where Escape arrives as `CSI 27 u` rather
    /// than a bare `0x1b`, so the timeout never interferes with the kitty path. It also does
    /// not affect query methods such as
    /// [`request_cursor_position`](Self::request_cursor_position), which carry their
    /// own timeout.
    pub fn set_esc_flush_timeout(&mut self, timeout: Option<Duration>) -> &mut Self {
        self.esc_flush_timeout = timeout;
        self
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
    /// # Lone-Escape flush
    ///
    /// A bare `ESC` (`0x1b`) is held pending by the decoder because it may begin an escape
    /// sequence. When [`esc_flush_timeout`](Self::esc_flush_timeout) is `Some(d)` and the
    /// decoder's only pending state is a lone `ESC`, this bounds its next read at `d`: bytes
    /// arriving first are decoded normally (they may complete a real sequence), and on elapse
    /// the lone `ESC` is flushed as [`Key::Escape`](crate::Key::Escape) so Esc-to-cancel is
    /// responsive. A non-lone-ESC pending state (a partial CSI/OSC or a mid-character UTF-8
    /// run) is never flushed this way. `None` disables the timeout. The default is
    /// `Some(25ms)`, and the policy is inert under kitty's disambiguate-escape-codes flag
    /// (Escape arrives there as `CSI 27 u`). See
    /// [`set_esc_flush_timeout`](Self::set_esc_flush_timeout).
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

            // Lone-Escape flush policy: with a configured timeout and a decoder holding *only* a
            // lone pending `ESC`, bound the read at a deadline. If bytes arrive first they are
            // decoded normally (they may complete a real sequence); if the deadline elapses first
            // the lone `ESC` is flushed as `Key::Escape`. Any other pending state (a partial
            // CSI/OSC or a mid-character UTF-8 run) keeps its wait-for-more behaviour. This is
            // scoped to `next_event` only — the query path (`run_query`) has its own deadline and
            // never sees this. Cancel-safe: all decoder/queue state lives on the session, so a
            // dropped future leaves the pending `ESC` pending and the session usable.
            let events = match self.esc_flush_timeout {
                Some(timeout) if self.decoder.has_pending_lone_escape() => {
                    let deadline = Instant::now() + timeout;
                    match timeout_at(deadline, self.read_events()).await {
                        Ok(result) => result?,
                        Err(_elapsed) => {
                            if let Some(event) = self.decoder.flush_pending_escape() {
                                return Ok(event);
                            }
                            // The pending state changed out from under the deadline (it cannot,
                            // with state on the session and no concurrent access, but stay
                            // defensive): fall through and read again rather than flush wrongly.
                            continue;
                        }
                    }
                }
                _ => self.read_events().await?,
            };
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

    /// Pushes kitty keyboard progressive-enhancement flags, without verifying what was granted.
    ///
    /// This is the narrow primitive: it writes `CSI > flags u`, flushes, and records the matching
    /// pop so teardown pops it — the async analogue of
    /// [`TerminalSession::push_kitty_keyboard`](crate::TerminalSession::push_kitty_keyboard). It
    /// does **not** query what the terminal granted. Drive that readback yourself at your own
    /// timing with [`commands::terminal::query_kitty_keyboard_flags`] and
    /// [`next_event`](Self::next_event), or use
    /// [`request_kitty_keyboard`](Self::request_kitty_keyboard) for the verify-after-push
    /// convenience built on top of this.
    ///
    /// # Errors
    ///
    /// Returns an error when the push bytes cannot be written or flushed.
    pub async fn push_kitty_keyboard(&mut self, flags: KittyKeyboardFlags) -> terminal::Result<()> {
        self.command(commands::terminal::push_kitty_keyboard_flags(flags))
            .await?;
        self.flush().await?;
        self.session.record_kitty_keyboard(flags);
        Ok(())
    }

    /// Requests kitty keyboard progressive-enhancement flags and verifies what was granted.
    ///
    /// This is the opt-in verify-after-push convenience layered on the set-only primitive
    /// [`push_kitty_keyboard`](Self::push_kitty_keyboard) (design 06). It:
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
        let capabilities = self.probe_capabilities_inner(timeout).await?;
        // Store the snapshot on the session (FM-C8: per-attachment, session-owned). Emit-gating
        // (`synchronized`) reads it after the probe. A caller that never probes leaves this `None`,
        // which the gate treats as unknown and degrades on (FM-V4). The returned value is a clone
        // so the caller can inspect it independently of the stored snapshot.
        self.capabilities = Some(capabilities.clone());
        Ok(capabilities)
    }

    /// Returns the capability snapshot from the most recent
    /// [`probe_capabilities`](Self::probe_capabilities), or `None` before any probe has run.
    ///
    /// The snapshot is per-attachment (FM-C8): it reflects the terminal this session was probing at
    /// probe time, not a live view, and a resume/reattach to a different outer terminal does not
    /// refresh it. Emit-gating on this session (see [`synchronized`](Self::synchronized)) reads
    /// this snapshot; a consumer can read it directly to make its own gated-emission decisions.
    #[must_use]
    pub fn capabilities(&self) -> Option<&Capabilities> {
        self.capabilities.as_ref()
    }

    /// Runs a full frame with synchronized output (DEC private mode 2026) **only when the probed
    /// capability says the terminal supports it** (R-CAP-4, FM-V4).
    ///
    /// The gate reads the session's stored [`capabilities`](Self::capabilities) snapshot:
    ///
    /// - When [`synchronized_output`](Capabilities::synchronized_output) is a **known `true`**
    ///   finding (the terminal answered the mode-2026 DECRQM probe affirmatively), this emits
    ///   [`begin_synchronized_update`](commands::screen::begin_synchronized_update) before running
    ///   `frame` and [`end_synchronized_update`](commands::screen::end_synchronized_update) after,
    ///   through the same write path [`command`](Self::command) uses, so the terminal paints the
    ///   frame atomically.
    /// - In **every other case** — the finding is unknown, known `false`, or the session was never
    ///   probed (`capabilities()` is `None`) — the frame body runs **without** the 2026 wrap. The
    ///   frame still renders; it is simply not batched. This is the FM-V4 rule: qwertty never emits
    ///   the 2026 begin/end into a terminal that did not answer the probe, because those bytes leak
    ///   raw onto terminals that do not understand them (codex#24543). Degrading is not an error.
    ///
    /// The `frame` closure draws through the session (its output methods are `async`, so `frame` is
    /// an `async` closure receiving `&mut Self`); its return value is returned on success. The gate
    /// wraps the whole closure: begin is emitted first, the closure runs, then end is emitted, in
    /// that order.
    ///
    /// # Forcing the wrap (escape hatch)
    ///
    /// There is intentionally no override argument in this method. A caller that must emit the 2026
    /// wrap regardless of the probe — because it probed out of band, or accepts the FM-V4 risk —
    /// drives the raw builders through [`command`](Self::command) directly:
    ///
    /// ```no_run
    /// # use qwertty::{TokioTerminalSession, commands};
    /// # async fn run(session: &mut TokioTerminalSession) -> qwertty::Result<()> {
    /// session
    ///     .command(commands::screen::begin_synchronized_update())
    ///     .await?;
    /// // ... draw the frame ...
    /// session
    ///     .command(commands::screen::end_synchronized_update())
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if writing the begin bytes, or the end bytes, fails; the underlying write
    /// error is propagated. When the frame runs un-gated (degraded), only the closure's own writes
    /// can fail. `frame`'s own errors surface through its return value `R`, not through this
    /// method's `Result`.
    pub async fn synchronized<R>(
        &mut self,
        frame: impl AsyncFnOnce(&mut Self) -> R,
    ) -> terminal::Result<R> {
        let gated = self
            .capabilities()
            .is_some_and(|caps| caps.synchronized_output.value_copied() == Some(true));

        if gated {
            self.command(commands::screen::begin_synchronized_update())
                .await?;
        }
        let result = frame(self).await;
        if gated {
            self.command(commands::screen::end_synchronized_update())
                .await?;
        }
        Ok(result)
    }

    /// The unstored probe: writes the bundle, gathers replies, and returns the snapshot. The public
    /// [`probe_capabilities`](Self::probe_capabilities) wraps this to store the snapshot on the
    /// session; keeping the gather logic here means its several exit paths do not each repeat the
    /// store.
    async fn probe_capabilities_inner(
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
    #[expect(
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
    #[expect(
        clippy::unused_async,
        reason = "teardown is synchronous (design 04 forbids spawn_blocking here), but leave stays \
                  an async fn for API continuity with the awaited call sites"
    )]
    pub async fn leave(mut self) -> terminal::Result<()> {
        self.restore_flags();
        self.session.leave()
    }

    /// Suspends the process to the shell, leaving the terminal clean, then requests a job-control
    /// stop (design 01 §4, playbook M6-S1).
    ///
    /// This is the `SIGTSTP` half of the suspend/resume lifecycle. It performs, in order:
    ///
    /// 1. **Ledger undo, entries kept.** It replays the composed session's mode ledger in reverse —
    ///    cooked mode, the mode-offs (mouse, paste, focus, in-band resize, alternate screen, cursor
    ///    show) — so the shell the user drops to sees a clean terminal, but **keeps** the ledger
    ///    entries so [`resume`](Self::resume) can re-apply them. This is exactly the re-entrant
    ///    [`TerminalSession::leave`], which also **disarms** the panic-safe restore handle: a
    ///    suspended process must not have its emergency hook fire while it sits stopped.
    /// 2. **Process-group guard (FM-G7).** Before signalling, it checks the process group
    ///    defensively. A degenerate group — a session leader whose process-group id equals its
    ///    session id, so there is no job-control shell to hand control back to — returns
    ///    [`terminal::Error::DegenerateProcessGroup`] rather than stopping a process nothing will
    ///    resume. The terminal has already been restored to cooked mode at this point, so a guard
    ///    rejection still leaves a usable terminal.
    /// 3. **Stop signal.** It sends `SIGTSTP` to the whole process group (not to self — FM-G3) so
    ///    the shell regains the terminal and the process stops. The send runs through an injectable
    ///    seam so tests exercise every mechanic above without stopping the test runner.
    ///
    /// The caller drives this from its own `SIGTSTP` integration (qwertty installs no signal
    /// handler); on `SIGCONT` it calls [`resume`](Self::resume). After the guard passes and the
    /// signal is sent the process is stopped, so this call returns only once the process is
    /// continued again.
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::DegenerateProcessGroup`] when the process group cannot safely be
    /// stopped, or a terminal-mode restoration error if the ledger undo fails. A stop-signal send
    /// error is surfaced as-is.
    #[expect(
        clippy::unused_async,
        reason = "suspend is synchronous today (ledger undo + a signal send), but stays an async \
                  fn for API symmetry with the awaited resume and the async lifecycle boundary — \
                  and to leave room for an awaited stop-signal seam later"
    )]
    pub async fn suspend(&mut self) -> terminal::Result<()> {
        // Step 1: undo the terminal state the re-entrant way (keeps the ledger entries for resume)
        // and disarm the restore handle — both are exactly what the inner `leave` does. The fcntl
        // status flags are intentionally *not* restored here: resume re-asserts non-blocking
        // itself, and the shared-description flags are the shell's concern only on a full
        // teardown.
        self.session.leave()?;

        // Steps 2 and 3: the FM-G7 guard and the `SIGTSTP` send, both behind the injectable seam.
        self.stop_signal.send()
    }

    /// Resumes the session after a `SIGCONT`, re-establishing terminal state the shell may have
    /// scrambled (design 01 §4, playbook M6-S1).
    ///
    /// This is the `SIGCONT` half of the lifecycle. The order matters — **termios resync first,
    /// then flags resync** — because raw mode and the readiness fd's non-blocking flag are separate
    /// pieces of terminal state the shell can each have reset, and the input path only works once
    /// both are back. It performs:
    ///
    /// 1. **Termios resync (re-enter raw mode).** It replays the kept mode ledger through the
    ///    composed [`TerminalSession::enter`], re-entering raw mode and re-applying every recorded
    ///    mode, and **re-arms** the panic-safe restore handle. This never trusts a cached termios:
    ///    the shell may have left the terminal cooked, so the session re-asserts its own modes
    ///    (codex's disable→enable discipline). Because the shell races the returning process for
    ///    the terminal (FM-G4), the re-enter is retried with a bounded budget (~10 tries × 50 ms,
    ///    the helix/neovim pattern) before giving up.
    /// 2. **Flags resync (re-assert non-blocking).** It re-applies the `O_NONBLOCK` status flag on
    ///    the readiness descriptor. The session set the descriptor non-blocking at construction,
    ///    but the shell the process returned from may have cleared it on the shared open file
    ///    description; [`AsyncFd`] requires non-blocking, so this must be re-asserted after
    ///    `SIGCONT`, after the termios resync.
    /// 3. **Optional input flush.** When `flush_input` is `true`, it `tcflush`es the pending input
    ///    so stale bytes the user typed at the shell (or a partially typed line) are dropped rather
    ///    than fed to the application as if typed into it. This is the caller's choice: an editor
    ///    usually wants the flush; a REPL replaying a command buffer may not.
    /// 4. **Synthetic resize.** It reads the current terminal size and queues an [`Event::Resize`]
    ///    so the next [`next_event`](Self::next_event) reports it — the window may have been
    ///    resized while the process was stopped, and this makes the application repaint at the size
    ///    the terminal is now, without waiting for a `SIGWINCH` or an in-band report.
    ///
    /// # Errors
    ///
    /// Returns a terminal-mode error if the bounded-retry re-enter never succeeds, if the readiness
    /// flag cannot be re-asserted, if the optional flush fails, or if the current size cannot be
    /// read for the synthetic resize.
    pub async fn resume(&mut self, flush_input: bool) -> terminal::Result<()> {
        // Step 1: termios resync — re-enter the kept ledger (raw mode + recorded modes) with a
        // bounded retry, since the shell races the returning process for the terminal (FM-G4).
        self.reenter_with_retry().await?;

        // Step 2: flags resync — re-assert non-blocking on the readiness fd. The shell may have
        // cleared it on the shared description after SIGCONT; AsyncFd requires it. This is done
        // *after* the termios resync, so the ordering the playbook mandates holds.
        self.reassert_nonblocking()?;

        // Step 3: optional stale-input flush. The caller decides whether typeahead at the shell is
        // dropped (`tcflush` of the input queue) or kept.
        if flush_input {
            self.flush_pending_input()?;
        }

        // Step 4: queue a synthetic resize so the app repaints at whatever size the terminal is now
        // (the window may have been resized while suspended). Read through the same size ladder and
        // enqueue an Event::Resize on the pending queue next_event drains.
        self.queue_synthetic_resize()?;

        Ok(())
    }

    /// Re-enters the mode ledger with a bounded retry, tolerating the shell racing for the
    /// terminal.
    ///
    /// The first re-enter after `SIGCONT` can fail transiently while the shell still holds the
    /// terminal (FM-G4). This retries up to [`RESUME_REENTER_RETRIES`] times with
    /// [`RESUME_REENTER_RETRY_DELAY`] between attempts — the helix/neovim ~10×50 ms pattern —
    /// before surfacing the last error. Each retry re-runs the same idempotent ledger replay; a
    /// re-enter that partially applied is safe to replay because the ledger entries are unchanged.
    async fn reenter_with_retry(&mut self) -> terminal::Result<()> {
        let mut last_error = None;
        for attempt in 0..RESUME_REENTER_RETRIES {
            match self.session.enter() {
                Ok(()) => return Ok(()),
                Err(error) => {
                    last_error = Some(error);
                    // The inner `enter` is a no-op once it has marked itself entered, so a retry
                    // needs the session to be left again before it will replay. Leaving is
                    // idempotent and keeps the ledger, so this cycles cleanly for the next attempt.
                    _ = self.session.leave();
                    if attempt + 1 < RESUME_REENTER_RETRIES {
                        tokio::time::sleep(RESUME_REENTER_RETRY_DELAY).await;
                    }
                }
            }
        }
        Err(last_error.expect("the retry loop runs at least once, so an error was recorded"))
    }

    /// Re-asserts the readiness descriptor's non-blocking flag after a `SIGCONT`.
    ///
    /// [`AsyncFd`] requires the registered descriptor to be non-blocking. The session set it so at
    /// construction, but the shell the process returned from may have cleared `O_NONBLOCK` on the
    /// shared open file description while it owned the terminal, so resume re-asserts it. Reads the
    /// current flags and sets non-blocking on top, so no other status flag the shell may have set
    /// is disturbed.
    fn reassert_nonblocking(&self) -> terminal::Result<()> {
        let fd = self.readiness.get_ref();
        let flags = fcntl_getfl(fd)
            .map_err(io::Error::from)
            .map_err(terminal::Error::set_terminal_mode)?;
        fcntl_setfl(fd, flags | OFlags::NONBLOCK)
            .map_err(io::Error::from)
            .map_err(terminal::Error::set_terminal_mode)
    }

    /// Drops pending input on the terminal (`tcflush` of the input queue).
    ///
    /// Called by [`resume`](Self::resume) only when the caller passes `flush_input: true`. This
    /// discards stale bytes typed at the shell while the process was stopped so they are not
    /// delivered to the application as if typed into it. It flushes the *input* queue only; queued
    /// output the session still owes is untouched.
    fn flush_pending_input(&self) -> terminal::Result<()> {
        rustix::termios::tcflush(
            self.readiness.get_ref(),
            rustix::termios::QueueSelector::IFlush,
        )
        .map_err(io::Error::from)
        .map_err(terminal::Error::set_terminal_mode)
    }

    /// Reads the current terminal size and enqueues a synthetic [`Event::Resize`] for delivery.
    ///
    /// The window may have been resized while the process was stopped, so resume enqueues one
    /// cell-geometry resize (a `SIGCONT` carries no pixel geometry, matching the `SIGWINCH`
    /// fallback) onto the same `pending` queue [`next_event`](Self::next_event) drains. The app
    /// then repaints at the size the terminal is now without waiting for a `SIGWINCH` or an
    /// in-band report.
    fn queue_synthetic_resize(&mut self) -> terminal::Result<()> {
        let size = self.session.size()?;
        let resize = ResizeEvent::new(size, None);
        self.pending.push_back(Event::Resize(resize));
        Ok(())
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

/// The default job-control stop-signal seam: guards the process group (FM-G7), then sends
/// `SIGTSTP` to the whole group.
///
/// This is what a session holds until a test swaps in a stub. It reads the process group and
/// session ids, rejects a degenerate group through [`process_group_is_suspendable`], and — when the
/// group is safe — sends `SIGTSTP` to the whole process group (not to self, FM-G3) so the
/// controlling shell regains the terminal and the process stops.
///
/// # Errors
///
/// Returns [`terminal::Error::DegenerateProcessGroup`] when the group is a session leader with no
/// job-control parent, or a read/write error if the id queries or the signal send fail.
fn send_real_stop_signal() -> terminal::Result<()> {
    let pgrp = getpgrp();
    // A `getsid(None)` failure is treated as "cannot prove the group is safe", which is itself the
    // degenerate case: refuse rather than stop into an unknown job-control state.
    let sid = getsid(None)
        .ok()
        .and_then(|sid| Pid::from_raw(sid.as_raw_pid()));
    process_group_is_suspendable(pgrp, sid)?;

    kill_process_group(pgrp, ProcessSignal::TSTP)
        .map_err(io::Error::from)
        .map_err(terminal::Error::write_terminal)
}

/// The FM-G7 process-group guard: decides whether a job-control stop signal is safe to send.
///
/// `pgrp` is the caller's process group; `sid` is its session id (or `None` when the session id
/// could not be determined). A stop signal is *unsafe* when the process is a session leader whose
/// process-group id equals its session id: such a process has no job-control shell to hand the
/// terminal back to, so `SIGTSTP` would drop it into a stopped state nothing will continue. This
/// case, and an undeterminable session id, both yield [`terminal::Error::DegenerateProcessGroup`].
///
/// Kept as a pure function of the two ids so the guard is unit-testable without depending on the
/// test process's real process group: a test passes the ids it wants to exercise.
fn process_group_is_suspendable(pgrp: Pid, sid: Option<Pid>) -> terminal::Result<()> {
    let Some(sid) = sid else {
        return Err(terminal::Error::degenerate_process_group(
            "the session id could not be determined",
        ));
    };
    if pgrp.as_raw_pid() == sid.as_raw_pid() {
        return Err(terminal::Error::degenerate_process_group(
            "the process is a session leader with no job-control shell to resume it",
        ));
    }
    Ok(())
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

/// Resolves the controlling terminal to its specific device path for a fresh open.
///
/// When standard input cannot supply the terminal (redirected, or not read-write), a fresh open
/// is required. The `/dev/tty` alias is never pollable through kqueue on macOS, but the specific
/// device path (for example `/dev/ttys003`) is in ordinary pseudoterminals, so the alias is
/// opened briefly only to ask the kernel for the real name. The alias itself is the last resort,
/// which remains correct on platforms whose pollers accept it. Known residual: inside tmux panes
/// even the specific path is not freshly pollable, so redirected-stdin sessions under tmux still
/// fail at registration (FM-A11).
///
/// Returns the path to open together with the [`TerminalAcquisition`] branch it represents:
/// [`ResolvedDevicePath`](TerminalAcquisition::ResolvedDevicePath) when the kernel yielded the
/// specific device name, or [`DevTtyAlias`](TerminalAcquisition::DevTtyAlias) when it did not and
/// the alias itself is the path. Threading the branch out here is what lets the session record an
/// accurate acquisition instead of collapsing both cases into one path.
fn resolved_controlling_terminal_path() -> (PathBuf, TerminalAcquisition) {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(DEV_TTY)
        .ok()
        .and_then(|device| rustix::termios::ttyname(&device, Vec::new()).ok())
        .map_or_else(
            || (PathBuf::from(DEV_TTY), TerminalAcquisition::DevTtyAlias),
            |name| {
                let path = PathBuf::from(OsString::from_vec(name.into_bytes()));
                (path, TerminalAcquisition::ResolvedDevicePath)
            },
        )
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

    // --- suspend/resume lifecycle (M6-S1) --------------------------------------------------------
    //
    // These drive the real suspend/resume mechanics over a headless `FakeDevice` (and one
    // PTY-backed session for the restore-handle disarm/re-arm, which only exists on a live
    // terminal). The actual `SIGTSTP` send is stubbed through the injectable seam so NO test
    // ever stops the test runner — a CI-safety requirement, not just a convenience (see the
    // module's `StopSignal`). A real TSTP/CONT signal round-trip is owed to the attended/manual
    // checklist (playbook M6-S1); it cannot be exercised headlessly without stopping the
    // harness, so we test every mechanic around the signal directly and leave the raw signal
    // delivery to the attended run.

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::time::timeout;

    use crate::{FakeDevice, FakeTerminal};

    /// Opens a fake-device Tokio session whose stop-signal seam is a counter-recording stub.
    ///
    /// Returns the session, the terminal peer for byte assertions, and the shared counter the stub
    /// increments on each `suspend` — so a test asserts the (stubbed) stop signal fired exactly
    /// once without a real `SIGTSTP`.
    fn fake_session_with_stub() -> (
        TokioTerminalSession<FakeDevice>,
        FakeTerminal,
        Arc<AtomicUsize>,
    ) {
        let (device, peer) = FakeDevice::open().expect("open fake device");
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let session = TokioTerminalSession::from_device(device)
            .expect("open Tokio session over fake device")
            .with_stop_signal(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
        (session, peer, calls)
    }

    #[tokio::test]
    async fn suspend_writes_mode_resets_and_invokes_the_stop_signal_once() {
        let (mut session, mut peer, calls) = fake_session_with_stub();

        // Enable a byte-based mode so the ledger undo has an observable reset to write.
        session
            .enable_mouse(MouseMode::Normal)
            .await
            .expect("mouse");
        session.flush().await.expect("flush");
        _ = peer.output().expect("drain enable bytes");

        session.suspend().await.expect("suspend");

        // The ledger undo wrote the mouse reset (CSI ? 1006 l CSI ? 1000 l) so the shell is clean.
        let undo = peer.output().expect("read suspend output");
        assert!(
            undo.windows(8).any(|w| w == b"\x1b[?1000l"),
            "suspend wrote the mouse reset, got {undo:?}",
        );

        // The (stubbed) stop signal fired exactly once — no real SIGTSTP touched the runner.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn resume_reapplies_modes_reasserts_nonblocking_and_queues_a_resize() {
        let (mut session, mut peer, _calls) = fake_session_with_stub();

        session
            .enable_mouse(MouseMode::Normal)
            .await
            .expect("mouse");
        session.flush().await.expect("flush");
        session.suspend().await.expect("suspend");
        _ = peer.output().expect("drain suspend bytes");

        // Clear the non-blocking flag on the shared description, as a shell would after SIGCONT, so
        // the re-assert has something to fix.
        let flags = fcntl_getfl(session.readiness.get_ref()).expect("getfl");
        fcntl_setfl(session.readiness.get_ref(), flags & !OFlags::NONBLOCK)
            .expect("clear nonblock");

        peer.set_size(TerminalSize::new(132, 43));
        session.resume(false).await.expect("resume");

        // Re-enter replayed the ledger: the mouse enable bytes are back on the wire.
        let reenter = peer.output().expect("read resume output");
        assert!(
            reenter.windows(8).any(|w| w == b"\x1b[?1000h"),
            "resume re-applied the mouse enable, got {reenter:?}",
        );

        // Non-blocking was re-asserted on the readiness fd (AsyncFd requires it).
        let flags = fcntl_getfl(session.readiness.get_ref()).expect("getfl after resume");
        assert!(
            flags.contains(OFlags::NONBLOCK),
            "resume must re-assert O_NONBLOCK on the readiness fd",
        );

        // A synthetic resize carrying the current size is queued for next_event.
        let event = session.next_event().await.expect("synthetic resize");
        let resize = event.resize_event().expect("the queued event is a resize");
        assert_eq!(resize.cells(), TerminalSize::new(132, 43));
        assert_eq!(resize.pixels(), None, "a SIGCONT resize carries no pixels");
    }

    #[tokio::test]
    async fn resume_with_flush_drops_stale_typeahead() {
        use std::io::Write;

        // `tcflush` is a real-tty operation (it errors on a socketpair), so the flush path is
        // tested over a PTY. The master writes stale typeahead into the slave's input
        // queue; resume's `tcflush(IFlush)` drops it, and a fresh sentinel confirms the
        // stale bytes are gone. The stop signal is still stubbed — no SIGTSTP is sent.
        let Some((mut master, slave_path, _sized)) = open_test_pty_for_suspend() else {
            return;
        };
        set_pty_nonblocking(&master);
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let mut session = TokioTerminalSession::open_path(slave_path)
            .expect("open PTY-backed session")
            .with_stop_signal(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });

        session.suspend().await.expect("suspend");

        // The user typed at the shell while stopped: stale bytes sit in the slave's input queue.
        // Write them, then drain any bytes the cooked-mode pty echoed back to the master so the
        // master's buffer cannot backpressure the session's next re-enter.
        master.write_all(b"stale").expect("write stale typeahead");
        master.flush().expect("flush master");
        // Let the bytes land in the slave's input queue before the flush, then clear the echo.
        tokio::time::sleep(Duration::from_millis(20)).await;
        drain_master(&master);

        session.resume(true).await.expect("resume with flush");
        drain_master(&master);

        // The synthetic resize is delivered first (queued by resume). Then a sentinel: if the stale
        // bytes had survived, they would arrive before it.
        let event = timeout(Duration::from_secs(1), session.next_event())
            .await
            .expect("next_event did not hang")
            .expect("synthetic resize first");
        assert!(event.resize_event().is_some(), "resize is delivered first");

        master.write_all(b"z").expect("write sentinel");
        master.flush().expect("flush sentinel");
        let event = timeout(Duration::from_secs(1), session.next_event())
            .await
            .expect("next_event did not hang")
            .expect("read after flush");
        assert_eq!(
            event,
            Event::Key(KeyEvent::new(Key::Char('z')).with_text('z')),
            "stale typeahead must be dropped by the flush; only the sentinel remains",
        );
    }

    /// Sets a PTY master non-blocking so the test's own reads never block. Best-effort.
    fn set_pty_nonblocking(master: &std::fs::File) {
        if let Ok(flags) = fcntl_getfl(master) {
            _ = fcntl_setfl(master, flags | OFlags::NONBLOCK);
        }
    }

    /// Drains and discards whatever the pty has queued toward the master (echoed input, session
    /// output), so an undrained master buffer cannot backpressure the session's blocking writes.
    /// Best-effort; the master is non-blocking, so this returns as soon as it would block.
    fn drain_master(mut master: &std::fs::File) {
        use std::io::Read;

        let mut sink = [0u8; 1024];
        while let Ok(read) = master.read(&mut sink) {
            if read == 0 {
                break;
            }
        }
    }

    #[tokio::test]
    async fn resume_without_flush_keeps_typeahead() {
        // The mirror of the flush test: with flush_input=false, stale bytes survive resume.
        let (mut session, mut peer, _calls) = fake_session_with_stub();
        session.suspend().await.expect("suspend");
        peer.feed_input(b"k").expect("feed typeahead");

        session.resume(false).await.expect("resume without flush");

        // Resize first (queued by resume), then the preserved keystroke.
        let event = session.next_event().await.expect("synthetic resize first");
        assert!(event.resize_event().is_some());
        let event = session.next_event().await.expect("preserved typeahead");
        assert_eq!(
            event,
            Event::Key(KeyEvent::new(Key::Char('k')).with_text('k')),
            "without flush, typeahead typed at the shell survives",
        );
    }

    // --- lone-Escape flush timing policy (consumer P0) -------------------------------------------
    //
    // A bare `ESC` is held pending by the decoder (it may begin a sequence), so `next_event`
    // applies a bounded flush timeout: a lone pending `ESC` becomes `Key::Escape` after the window
    // unless more input arrives first. These tests are deterministic — time is paused and advanced
    // manually, never a real sleep — and drive over the headless `FakeDevice`.

    /// Opens a plain fake-device Tokio session and its terminal peer (no stop-signal stub needed).
    fn fake_session() -> (TokioTerminalSession<FakeDevice>, FakeTerminal) {
        let (device, peer) = FakeDevice::open().expect("open fake device");
        let session =
            TokioTerminalSession::from_device(device).expect("open Tokio session over fake device");
        (session, peer)
    }

    #[tokio::test]
    async fn the_default_esc_flush_timeout_is_25ms() {
        let (session, _peer) = fake_session();
        assert_eq!(
            session.esc_flush_timeout(),
            Some(Duration::from_millis(25)),
            "the default lone-Escape flush window is 25ms",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_lone_escape_flushes_as_key_escape_after_the_timeout() {
        let (mut session, mut peer) = fake_session();
        peer.feed_input(b"\x1b").expect("feed lone ESC");

        // Drive next_event on a task so we can advance time past the window while it waits on the
        // bounded read. With paused time the timer only fires when we advance it.
        let handle = tokio::spawn(async move { session.next_event().await });
        // Yield so the spawned task reads the ESC and parks on the timeout deadline.
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(26)).await;

        let event = handle
            .await
            .expect("next_event task joined")
            .expect("next_event yielded");
        assert_eq!(
            event,
            Event::Key(KeyEvent::new(Key::Escape)),
            "a lone pending ESC flushes as Key::Escape once the window elapses",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_escape_sequence_arriving_within_the_window_suppresses_the_bare_escape() {
        let (mut session, mut peer) = fake_session();
        peer.feed_input(b"\x1b").expect("feed ESC");

        // Complete the sequence *before* advancing past the window: ESC [ A is an Up arrow, so the
        // bare Escape must never surface.
        peer.feed_input(b"[A").expect("feed CSI Up tail");

        // Do not advance time past the window; the sequence is already available to read.
        let event = session.next_event().await.expect("next_event yielded");
        assert_eq!(
            event,
            Event::Key(KeyEvent::new(Key::Up)),
            "ESC completed into an arrow key within the window yields Key::Up, not Escape",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_none_timeout_never_flushes_a_lone_escape_on_time_advance() {
        let (mut session, mut peer) = fake_session();
        session.set_esc_flush_timeout(None);
        assert_eq!(session.esc_flush_timeout(), None, "opt-out took effect");
        peer.feed_input(b"\x1b").expect("feed lone ESC");

        let handle = tokio::spawn(async move { session.next_event().await });
        tokio::task::yield_now().await;
        // Advancing well past the default window must NOT produce an Escape: with None there is no
        // deadline, so next_event stays parked waiting for more input. The still-pending task
        // proves the opt-out (the ESC stays held on the decoder for a later completing byte).
        tokio::time::advance(Duration::from_millis(500)).await;
        tokio::task::yield_now().await;
        assert!(
            !handle.is_finished(),
            "with esc_flush_timeout(None) a lone ESC never flushes on a time advance",
        );
        handle.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_partial_csi_is_not_flushed_as_escape_on_timeout() {
        let (mut session, mut peer) = fake_session();
        // A partial CSI: ESC [ with no final byte. This is NOT a lone ESC, so the flush timeout
        // must not apply — it keeps waiting for the bytes that finish the sequence.
        peer.feed_input(b"\x1b[").expect("feed partial CSI");

        let handle = tokio::spawn(async move { session.next_event().await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(500)).await;
        tokio::task::yield_now().await;
        assert!(
            !handle.is_finished(),
            "a partial CSI (ESC [) must not be flushed as Escape on a time advance",
        );
        handle.abort();
    }

    #[test]
    fn fm_g7_guard_rejects_a_session_leader_process_group() {
        // A degenerate group: the process is a session leader, so pgrp == sid. Stopping it would
        // leave nothing to resume it — the guard must reject with a typed error. The ids are
        // injected, so this never depends on the test's real process group (and never signals).
        let leader = Pid::from_raw(4321).expect("nonzero pid");
        let result = process_group_is_suspendable(leader, Some(leader));
        assert!(
            matches!(result, Err(terminal::Error::DegenerateProcessGroup { .. })),
            "a session-leader group must be rejected, got {result:?}",
        );
    }

    #[test]
    fn fm_g7_guard_rejects_an_undeterminable_session_id() {
        // When the session id cannot be read, the guard cannot prove the group is safe, so it
        // refuses rather than stopping into an unknown job-control state.
        let pgrp = Pid::from_raw(4321).expect("nonzero pid");
        let result = process_group_is_suspendable(pgrp, None);
        assert!(matches!(
            result,
            Err(terminal::Error::DegenerateProcessGroup { .. })
        ));
    }

    #[test]
    fn fm_g7_guard_allows_a_normal_job_controlled_group() {
        // A process running under a job-control shell has pgrp != sid (the shell is the session
        // leader). The guard allows the stop signal in that ordinary case.
        let pgrp = Pid::from_raw(4321).expect("nonzero pgrp");
        let sid = Pid::from_raw(1000).expect("nonzero sid");
        assert!(process_group_is_suspendable(pgrp, Some(sid)).is_ok());
    }

    #[tokio::test]
    async fn suspend_disarms_and_resume_rearms_the_restore_handle() {
        // The restore-handle disarm/re-arm is only observable on a live-terminal session (a
        // FakeDevice session carries no restore handle by design), so this uses a PTY-backed
        // session. The stop signal is still stubbed — no SIGTSTP is sent.
        let Some((_master, slave_path, _sized)) = open_test_pty_for_suspend() else {
            return;
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let mut session = TokioTerminalSession::open_path(slave_path)
            .expect("open PTY-backed session")
            .with_stop_signal(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });

        let handle = session.restore_handle();
        session.suspend().await.expect("suspend");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "stop signal fired once");

        // After suspend the handle is disarmed: a restore call finds nothing armed to restore.
        assert!(
            !handle.restore(),
            "suspend must disarm the restore handle so the emergency hook cannot fire while stopped",
        );
        // Re-arm it (restore() disarmed it as a side effect of the probe), so resume's re-arm is
        // the observable transition below, independent of that probe.
        handle.arm();

        session.resume(false).await.expect("resume");

        // After resume the handle is armed again: a restore now performs restoration.
        assert!(
            handle.restore(),
            "resume must re-arm the restore handle so panic-safe teardown is live again",
        );
    }

    /// Opens a PTY for the live-terminal suspend tests, or `None` when a PTY is unavailable (some
    /// sandboxes), mirroring the integration harness's skip behavior.
    ///
    /// Returns the master, the slave path the session opens, and a **held-open** slave fd carrying
    /// an 80x24 window size. The caller must keep that fd alive for the test's duration: on macOS a
    /// pty's winsize resets to 0x0 once every slave fd closes, so holding one open is what lets
    /// resume's synthetic-resize `size()` read a non-degenerate geometry instead of the 0x0 an
    /// unsized pty reports.
    fn open_test_pty_for_suspend() -> Option<(std::fs::File, PathBuf, std::fs::File)> {
        use std::os::unix::ffi::OsStringExt;

        use rustix::pty::{grantpt, ptsname, unlockpt};
        use rustix::termios::{Winsize, tcsetwinsize};

        let master = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/ptmx")
            .ok()?;
        grantpt(&master).ok()?;
        unlockpt(&master).ok()?;
        let slave = ptsname(&master, Vec::new()).ok()?;
        let slave = PathBuf::from(OsString::from_vec(slave.into_bytes()));

        let sized = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&slave)
            .ok()?;
        tcsetwinsize(
            &sized,
            Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .ok()?;
        Some((master, slave, sized))
    }
}
