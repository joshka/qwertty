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
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::fmt;
#[cfg(unix)]
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
#[cfg(unix)]
use std::os::fd::OwnedFd;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::time::Duration;

#[cfg(unix)]
use rustix::fs::{OFlags, fcntl_getfl};
#[cfg(unix)]
use rustix::process::{Pid, Signal as ProcessSignal, getpgrp, getsid, kill_process_group};
#[cfg(unix)]
use tokio::signal::unix::{Signal, SignalKind, signal};
#[cfg(windows)]
use tokio::signal::windows::{CtrlBreak, CtrlC, CtrlClose};
use tokio::time::{Instant, timeout_at};

#[cfg(windows)]
use self::readiness::ConsoleReadiness as FdReadiness;
// The readiness transport is a `cfg` sibling: the pollable-fd `FdReadiness` on Unix, the
// worker-thread `ConsoleReadiness` on Windows, aliased to the same name so the session body's
// `FdReadiness` references resolve on both platforms.
#[cfg(unix)]
use self::readiness::FdReadiness;
#[cfg(any(unix, windows))]
use crate::ResizeEvent;
use crate::caps::{
    Capabilities, EnvSource, ProbeBundle, identity_from_env, infer_hyperlinks, infer_truecolor,
    probe_bundle_commands, probe_skip_from_env, skipped_capabilities, std_env_source,
    store_bundle_reply,
};
use crate::commands::terminal::MouseMode;
use crate::correlate::{Correlator, Expectation, ExpectationId, Feed, Reply, Resolution};
use crate::report::{CursorPositionReport, TerminalStatusReport};
use crate::{
    Command, Event, InputBytes, KittyKeyboardFlags, KittyKeyboardGrant, SemanticDecoder, Terminal,
    TerminalDevice, TerminalSession, TerminalSize, commands, terminal,
};

/// The internal readiness transport: `AsyncFd`/`rustix` live here, not in the session body.
mod readiness;

#[cfg(unix)]
const DEV_TTY: &str = "/dev/tty";
const READ_BUFFER_LEN: usize = 1024;

/// How many times the ledger re-enter is retried before giving up, tolerating the shell (Unix
/// `resume`) or a returning child (`run_detached`, both platforms) racing the process for the
/// terminal (FM-G4). Ten tries at [`RESUME_REENTER_RETRY_DELAY`] apart is the helix/neovim pattern
/// (~half a second total).
#[cfg(any(unix, windows))]
const RESUME_REENTER_RETRIES: u32 = 10;
/// How long [`reenter_with_retry`](TokioTerminalSession::reenter_with_retry) waits between re-enter
/// attempts.
#[cfg(any(unix, windows))]
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
/// [`TokioTerminalSession`]. It has no Windows counterpart: a console is addressed by name
/// (`CONIN$`/`CONOUT$`) with no three-branch controlling-terminal fallback (ADR 0022).
#[cfg(unix)]
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

#[cfg(unix)]
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
/// # Lifecycle and hand-back
///
/// [`open`](Self::open) enters raw mode and registers the terminal fd with Tokio;
/// [`leave`](Self::leave) consumes the session and restores the terminal — the final teardown. To
/// hand the terminal back *temporarily* without dropping the session, use
/// [`suspend`](Self::suspend) / [`resume`](Self::resume) (Ctrl-Z job control) or
/// [`run_detached`](Self::run_detached) (running a child such as `$EDITOR`): each restores the
/// terminal and later re-enters over the same device fd.
///
/// # What you can do
///
/// - **Output:** [`command`](Self::command), [`text`](Self::text), [`flush`](Self::flush).
/// - **Input:** [`next_event`](Self::next_event) for decoded [`Event`](crate::Event)s;
///   [`read_input`](Self::read_input) for raw bytes.
/// - **Queries:** [`request_cursor_position`](Self::request_cursor_position) and
///   [`request_terminal_status`](Self::request_terminal_status); kitty keyboard via
///   [`push_kitty_keyboard`](Self::push_kitty_keyboard) (set-only) or
///   [`request_kitty_keyboard`](Self::request_kitty_keyboard) (verify-after-push).
/// - **Capabilities and modes:** [`probe_capabilities`](Self::probe_capabilities) then
///   [`capabilities`](Self::capabilities); the `enable_*` modes such as
///   [`enable_mouse`](Self::enable_mouse); [`synchronized`](Self::synchronized) frames gated on
///   mode 2026; [`set_esc_flush_timeout`](Self::set_esc_flush_timeout).
/// - **Streams:** [`resize_stream`](Self::resize_stream) (SIGWINCH) and [`signals`](Self::signals)
///   (SIGTSTP/CONT/TERM/INT), selected alongside `next_event`.
/// - **Observability:** [`acquisition`](Self::acquisition) reports which branch reached the
///   terminal.
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
    /// The internal readiness transport: a reactor-registered dup plus its saved `fcntl` flags.
    ///
    /// This is the transport seam. The session body never touches `AsyncFd` or `rustix` for its
    /// read/write/registration paths — it drives every byte through [`FdReadiness`], whose dup
    /// shares the same open file description as the device the session owns (so readiness observed
    /// on either applies to both) and whose saved flags are restored on teardown.
    ///
    /// This is `Some` for the whole ordinary lifetime of the session: it is populated at
    /// construction and every method reaches it through the `readiness` / `readiness_mut`
    /// accessors, which expect `Some`. It goes momentarily `None` only inside
    /// [`run_detached`](Self::run_detached), which [`detach`](FdReadiness::detach)es the transport
    /// (dropping the reactor registration and holding the raw fd) while a synchronous child owns
    /// the terminal, then [`reattach`](FdReadiness::reattach)es a fresh registration over the
    /// *same* fd before returning. Holding the fd raw across the handoff — rather than keeping
    /// a dormant registration — is what guarantees the post-handoff registration reads current
    /// readiness with no stale edge-triggered notification carried over from before the child
    /// ran.
    readiness: Option<FdReadiness>,
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
    /// through [`acquisition`](Self::acquisition). Unix-only: Windows has no fallback to record.
    #[cfg(unix)]
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
    /// error for a degenerate process group. Unix-only: Windows job control does not exist
    /// (ADR 0022 §7), so there is no stop signal to send.
    #[cfg(unix)]
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
#[cfg(unix)]
struct StopSignal(Box<dyn FnMut() -> terminal::Result<()> + Send>);

#[cfg(unix)]
impl StopSignal {
    /// Invokes the seam: checks the process group (FM-G7) and sends the stop signal, or records the
    /// call in a test stub.
    fn send(&mut self) -> terminal::Result<()> {
        (self.0)()
    }
}

#[cfg(unix)]
impl fmt::Debug for StopSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StopSignal(..)")
    }
}

#[cfg(unix)]
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
        let mut this = Self::from_session(session)?;
        // `from_session` leaves `acquisition` `None` (the `from_device` default); record which
        // controlling-terminal fallback branch produced this live terminal.
        this.acquisition = Some(acquisition);
        Ok(this)
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

#[cfg(windows)]
impl TokioTerminalSession<Terminal> {
    /// Opens the process console and starts a Tokio-backed session.
    ///
    /// This is the Windows entry point. It opens the console by name (`CONIN$`/`CONOUT$`) through
    /// [`Terminal::open`], enters raw mode through the session's ledger, and spawns the readiness
    /// worker: a dedicated thread that waits on the console input handle and the shutdown waker,
    /// reading input records only after the wait reports them pending, and feeding the translated
    /// VT bytes to the async session through a channel (ADR 0022 §4). Unlike the Unix
    /// [`open`](TokioTerminalSession::open), it registers nothing with the reactor — a console
    /// handle is not pollable — so it does **not** require a running Tokio runtime at construction;
    /// the async methods do, as always.
    ///
    /// # Errors
    ///
    /// Returns an error when no console is attached, raw mode cannot be entered, or the readiness
    /// worker's handles or waker event cannot be created.
    pub fn open() -> terminal::Result<Self> {
        let terminal = Terminal::open()?;
        let session = TerminalSession::from_terminal(terminal)?;
        Self::from_session(session)
    }

    /// Returns a panic-safe restore handle for this session.
    ///
    /// The handle stays valid without borrowing the session, so it can live inside a panic hook
    /// installed once for the whole program. This delegates to the composed
    /// [`TerminalSession::restore_handle`]; see [`RestoreHandle`](crate::RestoreHandle) for the
    /// hook pattern and what the emergency path covers. On Windows the emergency action writes the
    /// teardown blob to a duplicated console output handle and resets the captured console modes
    /// and codepage (ADR 0022 §7).
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
    /// plain unit tests with no pseudoterminal. The device must expose a readiness source — a
    /// pollable descriptor through [`TerminalDevice::as_fd`] on Unix, or console handles through
    /// `TerminalDevice::as_console_handles` on Windows; one that exposes neither is rejected with
    /// [`terminal::Error::Unsupported`] because the readiness transport has nothing to drive.
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
        Self::from_session(session)
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
    ///
    /// Unix-only: a Windows console is addressed by name with no fallback branch to observe.
    #[cfg(unix)]
    #[must_use]
    pub fn acquisition(&self) -> Option<TerminalAcquisition> {
        self.acquisition
    }

    /// Wraps an entered [`TerminalSession`] with the readiness transport and sans-io core.
    ///
    /// The readiness construction is the one platform fork. On Unix the device must expose a
    /// pollable descriptor through [`TerminalDevice::as_fd`]; on Windows it must expose its console
    /// handles through `TerminalDevice::as_console_handles`. A device that supplies neither is
    /// rejected with [`terminal::Error::Unsupported`] because the readiness transport has nothing
    /// to drive. Everything after that — the decoder, correlator, and pending queue — is
    /// identical on both platforms; `acquisition` is set separately by the Unix `from_terminal`
    /// (Windows has no fallback to record).
    fn from_session(session: TerminalSession<D>) -> terminal::Result<Self> {
        #[cfg(unix)]
        let readiness = {
            let borrowed = session.device().as_fd().ok_or_else(|| {
                terminal::Error::unsupported("Tokio readiness registration", "device without a fd")
            })?;
            FdReadiness::new(borrowed)?
        };
        #[cfg(windows)]
        let readiness = {
            let handles = session.device().as_console_handles().ok_or_else(|| {
                terminal::Error::unsupported(
                    "Tokio readiness worker",
                    "device without a console handle",
                )
            })?;
            FdReadiness::new(handles)?
        };

        Ok(Self {
            session,
            readiness: Some(readiness),
            decoder: SemanticDecoder::new(),
            correlator: Correlator::new(),
            pending: VecDeque::new(),
            active_query: None,
            active_probe: Vec::new(),
            capabilities: None,
            #[cfg(unix)]
            acquisition: None,
            #[cfg(unix)]
            stop_signal: StopSignal(Box::new(send_real_stop_signal)),
            esc_flush_timeout: Some(DEFAULT_ESC_FLUSH_TIMEOUT),
        })
    }

    /// Borrows the live readiness transport.
    ///
    /// The `readiness` field is `Some` for the whole ordinary lifetime of the session,
    /// going `None` only for the brief handoff window inside
    /// [`run_detached`](Self::run_detached), which never calls this. Every other method reaches
    /// the transport through this accessor so the `Option` stays an internal detail rather than
    /// rippling `expect`s across the read/write paths.
    ///
    /// Unix-only: its callers are the job-control lifecycle methods (resume/detached handoff/resize
    /// stream), which are Windows-gated; the shared read/write paths use `readiness_mut` instead.
    #[cfg(unix)]
    fn readiness(&self) -> &FdReadiness {
        self.readiness.as_ref().expect(
            "readiness registration is only absent inside run_detached, which never reads it",
        )
    }

    /// Mutably borrows the live readiness transport.
    ///
    /// The mutable sibling of the `readiness` accessor, used by the read/write awaits that
    /// need `&mut FdReadiness`. Same invariant: `Some` everywhere except the handoff window in
    /// [`run_detached`](Self::run_detached).
    fn readiness_mut(&mut self) -> &mut FdReadiness {
        self.readiness.as_mut().expect(
            "readiness registration is only absent inside run_detached, which never reads it",
        )
    }

    /// Replaces the job-control stop-signal seam with a test stub (unit tests only).
    ///
    /// This is how the suspend/resume unit tests exercise every mechanic — ledger undo, restore
    /// disarm, flags/mode resync, synthetic resize — while `SIGTSTP` is stubbed out, so no test
    /// ever stops the test runner (a CI-safety requirement). It is not part of the public API.
    #[cfg(all(test, unix))]
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
        // Write through the readiness transport, whose registered dup shares its open file
        // description with the device the session owns, so bytes written here are the device's
        // bytes and readiness stays correct under edge-triggered polling.
        self.readiness_mut().write_all(bytes.as_ref()).await
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

        let len = self.readiness_mut().read(buffer).await?;
        Ok(InputBytes::new(buffer[..len].to_vec()))
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
    /// - the kitty graphics support query (`APC G … a=q … ST`, the protocol spec's own probe) →
    ///   [`kitty_graphics`](Capabilities::kitty_graphics);
    /// - XTWINOPS text-area and cell pixel sizes (`CSI 14 t`, `CSI 16 t`) →
    ///   [`text_area_pixels`](Capabilities::text_area_pixels) /
    ///   [`cell_size`](Capabilities::cell_size), zero answers staying unknown (FM-Z5);
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
    /// # Dumb terminals are never probed (R-QRY-5)
    ///
    /// A dumb terminal echoes probe bytes as garbage output instead of answering them (FM-C5), so
    /// when the environment declares one — `TERM=dumb`, or the Linux console's `TERM=linux` — this
    /// writes nothing at all and returns immediately: every probe-backed finding is
    /// [`Evidence::Unknown`](crate::Evidence::Unknown) (nothing was asked, so nothing is a
    /// no-reply), the env-inferred findings and identity are still populated (they never touch the
    /// terminal), and the reason is recorded on [`Capabilities::probe_skip`] so a caller can tell
    /// a skipped probe from a silent terminal. See [`crate::caps::probe_skip_from_env`]. The skip
    /// snapshot is stored on the session like any probe result, so emit-gating degrades on it the
    /// same way it degrades on unknown.
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
        self.probe_capabilities_from_env(timeout, std_env_source)
            .await
    }

    /// [`probe_capabilities`](Self::probe_capabilities) with an injected [`EnvSource`] for the
    /// dumb-terminal guard, so tests exercise the skip path without mutating the process
    /// environment (which is unsound from parallel tests). The guard and its skip snapshot use
    /// `env`; the probing path's own env-inferred population keeps using the real environment via
    /// the shared bundle machinery.
    async fn probe_capabilities_from_env(
        &mut self,
        timeout: Duration,
        env: impl EnvSource,
    ) -> terminal::Result<Capabilities> {
        // The dumb-terminal guard (R-QRY-5, FM-C5): a dumb terminal (`TERM=dumb`, Linux console)
        // is never written to — the skip snapshot records why on `probe_skip`, with every
        // probe-backed finding honestly Unknown (nothing was asked, so nothing is a no-reply).
        let capabilities = match probe_skip_from_env(&env) {
            Some(skip) => skipped_capabilities(skip, &env),
            None => self.probe_capabilities_inner(timeout).await?,
        };
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
        let bundle = ProbeBundle::register(&mut self.correlator);
        self.active_probe.extend(bundle.ids());

        // Step 3: write the whole bundle in ONE buffer, DA1 last as the fence, then flush. The
        // command set is shared with the sync driver's `probe_capabilities` (`caps::
        // probe_bundle_commands`) so the two can never drift apart.
        self.bytes(probe_bundle_commands().into_bytes()).await?;
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
                    bundle.resolve_all(&mut self.correlator, Resolution::Cancelled);
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
                    if Some(id) == bundle.fence() {
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
        bundle.resolve_all(&mut self.correlator, Resolution::NoReply);
        self.active_probe.clear();
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
        let mut buffer = [0; READ_BUFFER_LEN];
        let len = self.readiness_mut().read(&mut buffer).await?;
        if len == 0 {
            return Err(terminal::Error::read_terminal(io::Error::new(
                ErrorKind::UnexpectedEof,
                "terminal input closed before another event was available",
            )));
        }

        // The decoder stays on the session, not in the readiness transport: the seam yields raw
        // bytes and the session owns all decode/correlate/pending state, so cancel-safety keeps
        // falling out of "state lives on the struct" (design 04). The read above performed no state
        // mutation until it returned bytes here.
        let mut events = self.decoder.feed(&buffer[..len]);
        // Drain-boundary flush: a read that did not fill the buffer means the operating system's
        // input buffer is drained, so a trailing text run the syntax layer parked for
        // split-equivalence is settled input the caller should receive now. Only *complete*
        // trailing text is flushed; a partial escape, control sequence, or mid-character
        // UTF-8 run keeps waiting for the bytes that finish it (design 02: the decoder
        // never guesses across a real split). Without this, the last character typed before
        // a pause — the `o` in "hello" — would sit unseen until the next keystroke, which
        // the real-emulator typeahead gate would catch.
        if len < buffer.len() && self.decoder.has_settled_text() {
            events.extend(self.decoder.finish());
        }
        Ok(events)
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
        // Restore the shared-description status flags before cooked mode (Unix only; the Windows
        // console transport has no descriptor flags to put back). Dropping `self` afterward joins
        // the readiness worker on Windows through the transport's `Drop`.
        #[cfg(unix)]
        self.restore_flags();
        self.session.leave()
    }
}

/// The lifecycle steps both platforms share: the bounded ledger re-enter and the synthetic-resize
/// enqueue. Unix `resume` and both platforms' `run_detached` re-establish terminal state through
/// these, so they live outside the Unix-only job-control block.
#[cfg(any(unix, windows))]
impl<D: TerminalDevice> TokioTerminalSession<D> {
    /// Re-enters the mode ledger with a bounded retry, tolerating the shell or a returning child
    /// racing for the terminal.
    ///
    /// The first re-enter after a handoff can fail transiently while the shell (Unix `SIGCONT`) or
    /// the just-returned child still holds the terminal (FM-G4). This retries up to
    /// [`RESUME_REENTER_RETRIES`] times with [`RESUME_REENTER_RETRY_DELAY`] between attempts — the
    /// helix/neovim ~10×50 ms pattern — before surfacing the last error. Each retry re-runs the
    /// same idempotent ledger replay; a re-enter that partially applied is safe to replay because
    /// the ledger entries are unchanged. Re-entering also re-arms the panic-safe restore handle.
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

    /// Reads the current terminal size and enqueues a synthetic [`Event::Resize`] for delivery.
    ///
    /// The window may have been resized while the process was stopped or the child owned the
    /// terminal, so the handoff enqueues one cell-geometry resize (no pixel geometry is available
    /// out of band, matching the `SIGWINCH` fallback) onto the same `pending` queue
    /// [`next_event`](Self::next_event) drains. The app then repaints at the size the terminal is
    /// now without waiting for a `SIGWINCH` or an in-band report.
    fn queue_synthetic_resize(&mut self) -> terminal::Result<()> {
        let size = self.session.size()?;
        let resize = ResizeEvent::new(size, None);
        self.pending.push_back(Event::Resize(resize));
        Ok(())
    }
}

/// The Windows lifecycle: the editor/pager handoff and the console ctrl-event signal stream, plus
/// the job-control methods that are typed `Unsupported` on a platform with no job control (ADR 0022
/// §7). Unlike Unix there is no `suspend`/`resume` pair (no `SIGTSTP`/`SIGCONT`) and no
/// `resize_stream` (resize arrives in band through [`next_event`](Self::next_event)); the handoff
/// swaps Unix's fd detach/reattach for pausing and resuming the readiness worker.
#[cfg(windows)]
impl<D: TerminalDevice> TokioTerminalSession<D> {
    /// Reports that suspend (job control) is unsupported on Windows.
    ///
    /// Windows has no job control: there is no `SIGTSTP`, no process-group stop, and no shell to
    /// hand the terminal back to (ADR 0022 §7). `Ctrl+Z` is an ordinary input byte on a Windows
    /// console, **not** a suspend request, so this never stops the process and never touches the
    /// terminal — it returns a typed [`terminal::Error::Unsupported`] immediately. An application
    /// that selects on [`signals`](Self::signals) simply never receives a
    /// [`Suspend`](TerminalSignal::Suspend) to act on. To run a child with the terminal, use
    /// [`run_detached`](Self::run_detached).
    ///
    /// # Errors
    ///
    /// Always returns [`terminal::Error::Unsupported`].
    #[expect(
        clippy::unused_async,
        reason = "mirrors the awaited Unix suspend so the call site is identical on both platforms"
    )]
    pub async fn suspend(&mut self) -> terminal::Result<()> {
        Err(terminal::Error::unsupported(
            "suspend (job control)",
            "windows",
        ))
    }

    /// Reports that the out-of-band resize stream is unsupported on Windows.
    ///
    /// Windows has no `SIGWINCH`. Resize is delivered **in band** instead: the readiness worker
    /// translates each `WINDOW_BUFFER_SIZE_EVENT` record into an in-band resize report, so a size
    /// change surfaces as an [`Event::Resize`](crate::Event::Resize) through
    /// [`next_event`](Self::next_event) with no signal handling (ADR 0022 §7, MW-1/MW-2). The
    /// `SIGWINCH` fallback this stream provides on Unix therefore has no role here, and the method
    /// returns a typed [`terminal::Error::Unsupported`] rather than a stream that never fires.
    ///
    /// # Errors
    ///
    /// Always returns [`terminal::Error::Unsupported`].
    pub fn resize_stream(&self) -> terminal::Result<ResizeStream> {
        Err(terminal::Error::unsupported("resize stream", "windows"))
    }

    /// Hands the console to a synchronous child, runs it, and reclaims the console cleanly
    /// afterward — the `$EDITOR`/pager/subshell handoff (ADR 0022 §7).
    ///
    /// The Windows sibling of the Unix
    /// [`run_detached`](TokioTerminalSession::run_detached). Where the Unix handoff detaches and
    /// reattaches the reactor-registered fd, this **pauses and resumes the readiness worker**:
    /// while the child owns the console, qwertty's reader must not be calling
    /// `ReadConsoleInputW` on the same input, or the child and the worker would steal each
    /// other's keystrokes. `f` is a synchronous `FnOnce`; the async session is quiescent for
    /// the whole handoff (no `.await` between releasing and reclaiming the console), so the
    /// child owns a clean console and this session is fully usable again on return. Whatever
    /// `f` returns is returned on success.
    ///
    /// # Steps
    ///
    /// 1. **Ledger undo, entries kept.** Replays the mode ledger's resets so the child sees a
    ///    normal (cooked) console and **disarms** the panic-safe restore handle, exactly as the
    ///    Unix handoff does (the re-entrant [`TerminalSession::leave`]). The entries are kept for
    ///    the re-enter.
    /// 2. **Pause the reader worker.** Signals the worker's waker, joins it, and discards any
    ///    buffered pre-child input — but keeps the console handles and the waker event alive. No
    ///    worker is reading the console while the child runs.
    /// 3. **Run `f()`** — the child blocks here, owning the console.
    /// 4. **Resume the reader worker.** Resets the waker event and respawns the worker over the
    ///    same handles with a fresh channel, so post-child reads start clean.
    /// 5. **Termios resync.** Re-enters the kept ledger with the bounded retry through
    ///    [`TerminalSession::enter`], re-entering raw mode, re-applying every recorded mode, and
    ///    **re-arming** the restore handle. This never trusts what the child left (a child like
    ///    `vi` may have scrambled the console modes).
    /// 6. **Synthetic resize.** Queues an [`Event::Resize`](crate::Event::Resize) so the next
    ///    [`next_event`](Self::next_event) reports the current geometry — the window may have been
    ///    resized while the child owned the console.
    ///
    /// # Failed resume (well-defined state)
    ///
    /// The worker is joined in step 2 before the child runs, so at most one of the worker and the
    /// child ever reads the console. If resuming the worker (step 4) fails — the waker cannot be
    /// reset or the reader thread cannot be respawned — this returns the error with the transport
    /// left worker-less (no live worker, so `Drop` never double-joins) and the restore handle
    /// disarmed from step 1. The console has already been restored to a clean cooked state by step
    /// 1, so the failure leaves a usable console for the caller even though this session can no
    /// longer drive it — the same guarantee the Unix handoff makes on a failed re-registration.
    ///
    /// # Errors
    ///
    /// Returns an error if the ledger undo fails, if the reader worker cannot be resumed, if the
    /// bounded-retry re-enter never succeeds, or if the current size cannot be read for the
    /// synthetic resize.
    pub async fn run_detached<R>(&mut self, f: impl FnOnce() -> R) -> terminal::Result<R> {
        // Step 1: undo the terminal state the re-entrant way (keeps the ledger entries) and disarm
        // the restore handle — exactly what the Unix handoff does. `enter` in step 5 replays the
        // kept entries and re-arms the handle.
        self.session.leave()?;

        // Step 2: pause the reader worker so it is not calling `ReadConsoleInputW` on the console
        // the child is about to own. Pausing signals the waker, joins the worker, and discards
        // buffered pre-child input, while keeping the handles and waker alive for the resume.
        self.readiness_mut().pause();

        // Run the child. It blocks on the current thread and owns the console for its lifetime. No
        // `.await` runs while the worker is paused, so the session stays quiescent.
        let result = f();

        // Step 3 (steps 4-6 below): resume the reader worker — reset the waker and respawn the
        // worker over the same handles with a fresh channel, so post-child reads start clean. A
        // failed resume leaves the transport worker-less (Drop will not double-join) and the
        // restore handle disarmed; the console is already cooked from step 1.
        self.readiness_mut().resume()?;

        // Step 5: termios resync — re-enter the kept ledger with the bounded retry (re-entering raw
        // mode, re-applying every recorded mode, re-arming the restore handle). Never trust the
        // console modes the child left.
        self.reenter_with_retry().await?;

        // Step 6: queue a synthetic resize so the app repaints at whatever size the console is now
        // (the window may have been resized while the child owned it).
        self.queue_synthetic_resize()?;

        Ok(result)
    }

    /// Returns an awaitable [`SignalStream`] of the console control events a TUI cares about,
    /// mapped to typed [`TerminalSignal`] values.
    ///
    /// This is the Windows analogue of the Unix
    /// [`signals`](TokioTerminalSession::signals). qwertty installs no handler at construction; the
    /// listeners are registered only when this is called, and the stream only *reports* — the
    /// application owns the response. It owns three `tokio::signal::windows` listeners and maps
    /// them: `Ctrl+C` and `Ctrl+Break` both to [`Interrupt`](TerminalSignal::Interrupt), and the
    /// console close signal to [`Terminate`](TerminalSignal::Terminate).
    ///
    /// [`Suspend`](TerminalSignal::Suspend) and [`Continue`](TerminalSignal::Continue) **never
    /// arrive on Windows** — there is no job control (ADR 0022 §7) — so an application's
    /// `match` on those arms is simply never taken. The [`TerminalSignal`] enum is
    /// `#[non_exhaustive]`, so the same `select!` wiring compiles unchanged across platforms.
    ///
    /// # Errors
    ///
    /// Returns an error when any of the three console control listeners cannot be installed.
    pub fn signals(&self) -> terminal::Result<SignalStream> {
        use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close};

        // Install the listeners now — never at construction (the app owns registration by calling
        // this). Ctrl+C and Ctrl+Break both map to Interrupt; the console close signal maps to
        // Terminate. There is no Suspend/Continue source on Windows.
        let ctrl_c = ctrl_c().map_err(terminal::Error::read_terminal)?;
        let ctrl_break = ctrl_break().map_err(terminal::Error::read_terminal)?;
        let ctrl_close = ctrl_close().map_err(terminal::Error::read_terminal)?;
        Ok(SignalStream {
            ctrl_c,
            ctrl_break,
            ctrl_close,
        })
    }
}

/// The Unix job-control and signal lifecycle: suspend/resume, the detached handoff, the resize and
/// signal streams, and the fd-flag restore they share. Windows has no job control (ADR 0022 §7) and
/// no pollable descriptor, so none of this exists on the Windows build; its teardown is the
/// readiness worker's `Drop`.
#[cfg(unix)]
impl<D: TerminalDevice> TokioTerminalSession<D> {
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
    ///    description; `AsyncFd` requires non-blocking, so this must be re-asserted after
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
        self.readiness().reassert_nonblocking()?;

        // Step 3: optional stale-input flush. The caller decides whether typeahead at the shell is
        // dropped (`tcflush` of the input queue) or kept.
        if flush_input {
            self.readiness().flush_input()?;
        }

        // Step 4: queue a synthetic resize so the app repaints at whatever size the terminal is now
        // (the window may have been resized while suspended). Read through the same size ladder and
        // enqueue an Event::Resize on the pending queue next_event drains.
        self.queue_synthetic_resize()?;

        Ok(())
    }

    /// Hands the terminal to a synchronous child, runs it, and reclaims the terminal cleanly
    /// afterward — the `$EDITOR`/pager/subshell handoff (design 01 §4, playbook M6-S2).
    ///
    /// This is the detached-handoff sibling of [`suspend`](Self::suspend)/[`resume`](Self::resume).
    /// Where suspend/resume drop to the *shell* on `SIGTSTP` and come back on `SIGCONT`, this hands
    /// the terminal to a child process the caller spawns and waits for **inside `f`**, with no
    /// job-control stop. `f` is a synchronous `FnOnce`: the async session is quiescent for the
    /// whole handoff window (there is no `.await` between releasing and reclaiming the
    /// terminal), so the child owns a clean blocking terminal and this session is fully usable
    /// again on return.
    ///
    /// # What `f` is
    ///
    /// The child is the caller's business. A typical `f` is
    /// `|| std::process::Command::new(editor).status()`, or launching a pager or subshell —
    /// anything that blocks the current thread while a foreground child owns the terminal.
    /// Whatever `f` returns (`R`) is returned from `run_detached` on success, so the caller
    /// inspects the child's `ExitStatus` (or any other result) directly.
    ///
    /// # Steps
    ///
    /// **Before `f` — release the terminal to the child:**
    ///
    /// 1. **Ledger undo, entries kept.** Replays the mode ledger's resets so the child sees a clean
    ///    terminal and **disarms** the panic-safe restore handle, exactly as
    ///    [`suspend`](Self::suspend) does (the re-entrant [`TerminalSession::leave`]). The entries
    ///    are kept so the terminal can be re-entered afterward.
    /// 2. **Restore the original blocking flags.** Puts the construction-time fcntl status flags
    ///    back on the shared open file description, clearing the session's `O_NONBLOCK`. The child
    ///    does ordinary *blocking* reads on stdin; without this its reads would spuriously hit
    ///    `EAGAIN` from the non-blocking flag the session set on the shared description (FM-L
    ///    class).
    /// 3. **Release the reactor registration.** Detaches the readiness transport, dropping the
    ///    Tokio readiness registration entirely and holding the raw fd (and its saved flags) across
    ///    the closure.
    ///
    /// **Run `f()`** — call the closure and capture its `R`. It blocks; the caller's child runs
    /// here.
    ///
    /// **After `f` returns — reclaim the terminal, never trusting what the child left:**
    ///
    /// 4. **Re-register.** Reattaches a **fresh** reactor registration over the *same* fd. Because
    ///    the registration was fully released in step 3 and re-taken here, the reactor reads the
    ///    descriptor's *current* readiness with no stale edge-triggered notification carried over
    ///    from before the child ran — see below.
    /// 5. **Termios + flags resync.** Re-asserts `O_NONBLOCK` on the readiness fd (the same
    ///    `reassert_nonblocking` step [`resume`](Self::resume) uses) and re-enters the kept ledger
    ///    with the same bounded retry through [`TerminalSession::enter`], which re-enters raw mode,
    ///    re-applies every recorded mode, and **re-arms** the restore handle. This never trusts the
    ///    termios the child left: a child like `vi` or `stty` may have left the terminal cooked,
    ///    with wrong flags, or otherwise scrambled (FM-L9), so the session re-asserts its own modes
    ///    wholesale.
    /// 6. **Synthetic resize.** Queues an [`Event::Resize`] — the same synthetic-resize step
    ///    [`resume`](Self::resume) uses — so the next [`next_event`](Self::next_event) reports the
    ///    current geometry — the window may have been resized while the child owned it.
    ///
    /// Then it returns `Ok(f_result)`.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime: reclaiming the terminal re-registers the fd with
    /// the current runtime's reactor (`AsyncFd::try_new`), which requires an active runtime, the
    /// same requirement as [`open`](Self::open).
    ///
    /// # Why a fresh registration (the edge-triggered readiness concern)
    ///
    /// Tokio's `AsyncFd` registers the descriptor with an edge-triggered reactor (kqueue/epoll).
    /// Readiness is delivered as *edges*: a transition to readable/writable notifies once, and the
    /// reactor then waits for the next transition. While the synchronous child owns the terminal
    /// the runtime is not polling this fd, yet the child freely reads and writes it — draining
    /// input the session had been notified about, and generating new input the session was not.
    /// If the session kept a dormant registration across the handoff, the reactor's cached
    /// readiness could be stale in either direction: an edge that fired (or was consumed by the
    /// child) before the child ran is not reliably re-delivered, so the first post-handoff
    /// `readable()` could either miss input that is already waiting or believe the fd is ready
    /// and then spin on `WouldBlock` until a *new* edge arrives — a wedged input path. Clearing
    /// the guard's readiness (`clear_ready`) cannot fix this: it clears this session's view,
    /// not a missed kernel-level edge. Fully dropping the registration (step 3) and taking a
    /// fresh one (step 4) sidesteps the whole class: the new registration performs a fresh
    /// readiness assessment on the same fd, so the session resumes reading from the terminal's
    /// *current* state with no carried-over edge. The `O_NONBLOCK` restore in step 2 and
    /// re-assert in step 5 keep the child's blocking reads and the reactor's non-blocking
    /// requirement each correct on their own side of the handoff.
    ///
    /// # Errors
    ///
    /// Returns an error if the ledger undo fails, if the fresh registration cannot be taken
    /// (surfaced as an open-terminal error, matching construction), if non-blocking cannot be
    /// re-asserted, if the bounded-retry re-enter never succeeds, or if the current size cannot be
    /// read for the synthetic resize. When re-registration fails the terminal has still been
    /// restored to a clean blocking state by steps 1 and 2, so the failure leaves a usable terminal
    /// for the caller even though this session can no longer drive it.
    pub async fn run_detached<R>(&mut self, f: impl FnOnce() -> R) -> terminal::Result<R> {
        // Step 1: undo the terminal state the re-entrant way (keeps the ledger entries) and disarm
        // the restore handle — exactly what `suspend` does. `enter` in step 5 replays the kept
        // entries and re-arms the handle.
        self.session.leave()?;

        // Step 2: restore the original (blocking) fcntl flags on the shared description so the
        // child's blocking stdin reads do not hit EAGAIN from the session's O_NONBLOCK. Done while
        // the readiness registration is still present, so `restore_flags` finds it.
        self.restore_flags();

        // Step 3: release the reactor registration by detaching the readiness transport, holding
        // the raw fd (and its saved flags) across the closure. `readiness` is guaranteed `Some`
        // here: a session with no pollable fd is rejected at construction, so the only way it is
        // ever `None` is inside this very window. Detaching drops the edge-triggered registration
        // so step 4 can reattach a guaranteed-fresh one.
        let (owned, original_flags) = self
            .readiness
            .take()
            .expect("readiness is Some outside the run_detached window")
            .detach();

        // Run the child. It blocks; the caller spawns/waits their foreground process here. No
        // `.await` runs while the registration is released, so the session stays quiescent and no
        // read/write path can observe the `None`.
        let result = f();

        // Step 4: reattach a FRESH registration over the SAME fd. A fresh registration reads the
        // descriptor's current readiness with no stale edge carried over from before the child ran
        // (see the method docs' edge-triggered discussion). On failure the fd is dropped with the
        // error; the terminal is already clean (steps 1-2), so the caller keeps a usable terminal
        // even though this session cannot continue.
        self.readiness = Some(FdReadiness::reattach(owned, original_flags)?);

        // Step 5: flags + termios resync. Re-assert non-blocking on the fresh registration (AsyncFd
        // requires it), then re-enter the kept ledger with the bounded retry — re-entering raw
        // mode, re-applying every recorded mode, and re-arming the restore handle. Never
        // trust the termios the child left (FM-L9): the re-enter re-asserts the session's
        // own modes wholesale.
        self.readiness().reassert_nonblocking()?;
        self.reenter_with_retry().await?;

        // Step 6: queue a synthetic resize so the app repaints at whatever size the terminal is now
        // (the window may have been resized while the child owned it).
        self.queue_synthetic_resize()?;

        Ok(result)
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
        // The private size descriptor comes from the readiness transport, so the session body stops
        // touching `rustix` for the dup: `dup_fd` duplicates the registered dup, which shares the
        // device's open file description, so the size it measures is the session's terminal size.
        let size_fd = self.readiness().dup_fd()?;
        let signal = signal(SignalKind::window_change()).map_err(terminal::Error::read_terminal)?;
        Ok(ResizeStream { signal, size_fd })
    }

    /// Returns an awaitable [`SignalStream`] that yields the terminal-relevant process signals a
    /// TUI cares about — [`Suspend`](TerminalSignal::Suspend),
    /// [`Continue`](TerminalSignal::Continue), [`Terminate`](TerminalSignal::Terminate), and
    /// [`Interrupt`](TerminalSignal::Interrupt) — as typed [`TerminalSignal`] values the
    /// application selects on and responds to itself.
    ///
    /// This is the **opt-in** companion to [`resize_stream`](Self::resize_stream). qwertty installs
    /// no signal handler at session construction and never auto-acts on a signal (design 01): the
    /// listeners are installed only when this method is called, and the stream only *reports* — the
    /// application owns the response. On a `Suspend` (`SIGTSTP`) it typically calls
    /// [`suspend`](Self::suspend); on a `Continue` (`SIGCONT`), [`resume`](Self::resume); on a
    /// `Terminate` (`SIGTERM`) or `Interrupt` (`SIGINT`), it exits gracefully. Nothing forces those
    /// responses — a REPL might treat `Interrupt` as "cancel the current line" instead.
    ///
    /// `SIGWINCH` is deliberately **not** handled here; that is
    /// [`resize_stream`](Self::resize_stream)'s job. An application selects on
    /// [`signals`](Self::signals), [`resize_stream`](Self::resize_stream),
    /// and [`next_event`](Self::next_event) together. Because the stream does not borrow the
    /// session, it can sit in a `tokio::select!` alongside those:
    ///
    /// ```no_run
    /// # async fn run() -> qwertty::Result<()> {
    /// use qwertty::{TerminalSignal, TokioTerminalSession};
    ///
    /// let mut session = TokioTerminalSession::open()?;
    /// let mut signals = session.signals()?;
    /// loop {
    ///     tokio::select! {
    ///         event = session.next_event() => { let _event = event?; }
    ///         signal = signals.next() => match signal? {
    ///             TerminalSignal::Suspend => session.suspend().await?,
    ///             TerminalSignal::Continue => session.resume(true).await?,
    ///             TerminalSignal::Terminate | TerminalSignal::Interrupt => break,
    ///             // `TerminalSignal` is `#[non_exhaustive]`; future signals land here.
    ///             _ => {}
    ///         }
    ///     }
    /// }
    /// # session.leave().await
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when any of the four signal listeners cannot be installed.
    pub fn signals(&self) -> terminal::Result<SignalStream> {
        // Install the four listeners now — never at construction (design 01: the app owns
        // registration by calling this). SIGINT/SIGTERM have named Tokio constructors; SIGTSTP and
        // SIGCONT are addressed by raw number via rustix's signal table (no libc dependency).
        let suspend = signal(SignalKind::from_raw(ProcessSignal::TSTP.as_raw()))
            .map_err(terminal::Error::read_terminal)?;
        let continue_ = signal(SignalKind::from_raw(ProcessSignal::CONT.as_raw()))
            .map_err(terminal::Error::read_terminal)?;
        let terminate = signal(SignalKind::terminate()).map_err(terminal::Error::read_terminal)?;
        let interrupt = signal(SignalKind::interrupt()).map_err(terminal::Error::read_terminal)?;
        Ok(SignalStream {
            suspend,
            continue_,
            terminate,
            interrupt,
        })
    }

    /// Restores the device status flags captured before this session set the descriptor
    /// non-blocking.
    ///
    /// Delegates to [`FdReadiness::restore_flags`], which puts the flags back on the shared open
    /// file description (the readiness dup and the session device share one, so restoring on either
    /// restores them for both). This runs before the session teardown and again from drop, so every
    /// exit path puts the flags back (idempotent; a redundant set is harmless). The `Option` guard
    /// keeps this a no-op inside the `run_detached` handoff window — the one path that holds the
    /// dup raw and restored the flags itself before detaching.
    fn restore_flags(&self) {
        if let Some(readiness) = self.readiness.as_ref() {
            readiness.restore_flags();
        }
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
        // parent shell (FM-L class). The session's Drop handles cooked-mode restoration. On Windows
        // there are no descriptor flags: the readiness field's own `Drop` signals the waker and
        // joins the worker, and the session's `Drop` restores the console modes.
        #[cfg(unix)]
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
#[cfg(unix)]
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

#[cfg(unix)]
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

/// The Windows placeholder for the `SIGWINCH`-driven resize source, which Windows does not have.
///
/// It is the return type of the Windows [`resize_stream`](TokioTerminalSession::resize_stream),
/// which always returns [`terminal::Error::Unsupported`]: resize is delivered in band through
/// [`next_event`](TokioTerminalSession::next_event) instead (ADR 0022 §7). The type is
/// **uninhabited** — its only field is [`Infallible`](std::convert::Infallible) — so a
/// `ResizeStream` can never be constructed on Windows, and [`next_resize`](Self::next_resize) is
/// statically unreachable. It exists only so the method keeps its cross-platform signature.
#[cfg(windows)]
#[derive(Debug)]
pub struct ResizeStream {
    /// Uninhabited: Windows never constructs a `ResizeStream`, so this field can never be
    /// produced.
    never: std::convert::Infallible,
}

#[cfg(windows)]
impl ResizeStream {
    /// Statically unreachable: a `ResizeStream` cannot be constructed on Windows.
    ///
    /// # Errors
    ///
    /// Never returns; the method cannot be called because no `ResizeStream` value exists.
    #[expect(
        clippy::unused_async,
        reason = "uninhabited-type method kept async for signature parity with the Unix next_resize"
    )]
    pub async fn next_resize(&mut self) -> terminal::Result<ResizeEvent> {
        match self.never {}
    }
}

/// A terminal-relevant process signal a TUI selects on, reported by [`SignalStream`].
///
/// These are the signals a full-screen application typically wants to respond to itself, distinct
/// from the resize signal (`SIGWINCH`), which [`ResizeStream`] handles. qwertty only *reports*
/// them; the application owns the response. `#[non_exhaustive]` because more terminal-relevant
/// signals may be added.
///
/// Obtain a stream of these from [`TokioTerminalSession::signals`].
///
/// # Platform coverage
///
/// The enum is whole on every platform, but which variants a stream can yield differs. On Unix all
/// four arrive. On Windows there is no job control (ADR 0022 §7): the console signal source yields
/// only [`Interrupt`](Self::Interrupt) (from `Ctrl+C`/`Ctrl+Break`) and
/// [`Terminate`](Self::Terminate) (from the console close) — [`Suspend`](Self::Suspend) and
/// [`Continue`](Self::Continue) never arrive. Because the enum is `#[non_exhaustive]`, one
/// `select!` matching all four compiles and runs unchanged on both.
#[cfg(any(unix, windows))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TerminalSignal {
    /// `SIGTSTP` — a job-control stop request, usually from the user pressing `Ctrl-Z`.
    ///
    /// The intended response is [`TokioTerminalSession::suspend`], which restores the terminal to a
    /// clean cooked state and stops the process group so the controlling shell regains the
    /// terminal.
    Suspend,
    /// `SIGCONT` — the process has been continued, usually by the shell bringing the job back with
    /// `fg`.
    ///
    /// The intended response is [`TokioTerminalSession::resume`], which re-enters raw mode and the
    /// recorded modes, re-asserts non-blocking, and queues a synthetic resize so the application
    /// repaints at the current size.
    Continue,
    /// `SIGTERM` — a polite request to terminate (the default `kill`).
    ///
    /// The intended response is a graceful exit: leave the session (restoring the terminal) and
    /// shut down.
    Terminate,
    /// `SIGINT` — an interrupt, usually from the user pressing `Ctrl-C` (when the terminal delivers
    /// it as a signal rather than a keystroke).
    ///
    /// The intended response is typically a graceful exit, though an application is free to treat
    /// it as a lighter-weight cancellation (a REPL cancelling the current input line, say).
    Interrupt,
}

/// An awaitable stream of the terminal-relevant process signals a TUI selects on.
///
/// Obtain one from [`TokioTerminalSession::signals`]. Like [`ResizeStream`], it is an independent
/// value that does not borrow the session (design 01: qwertty installs no handler itself, only
/// exposes a stream the app selects on), so it can sit in a `tokio::select!` alongside
/// [`next_event`](TokioTerminalSession::next_event) and
/// [`resize_stream`](TokioTerminalSession::resize_stream). It owns the platform's signal listeners
/// and `select!`s across them on each [`next`](Self::next), yielding whichever fired as a typed
/// [`TerminalSignal`]: on Unix the four job-control/terminate/interrupt listeners (`SIGTSTP`,
/// `SIGCONT`, `SIGTERM`, `SIGINT`); on Windows the three console control listeners (`Ctrl+C`,
/// `Ctrl+Break`, and the console close), which never yield suspend/continue (ADR 0022 §7).
///
/// The resize signal (`SIGWINCH` on Unix) is deliberately excluded — that is [`ResizeStream`]'s
/// responsibility on Unix, and on Windows resize arrives in band through
/// [`next_event`](TokioTerminalSession::next_event) — so mixing it in would blur the resize path
/// with the lifecycle-signal path.
///
/// # Shape choice
///
/// Like [`ResizeStream`], this is a small helper with an `async fn` [`next`](Self::next) rather
/// than a full `futures::Stream` implementation. The awaitable-method shape keeps the type
/// dependency-free (no `futures`/`Stream` in the public API before the vocabulary freeze) and is
/// all a `select!` loop needs; a `Stream` impl can be added later without changing this method
/// (design 04).
#[cfg(any(unix, windows))]
#[cfg_attr(
    windows,
    allow(
        clippy::struct_field_names,
        reason = "the three Windows listeners are all console `ctrl_*` events; the shared prefix \
                  names the source, it is not redundant"
    )
)]
#[derive(Debug)]
pub struct SignalStream {
    /// The `SIGTSTP` (job-control stop) listener. Tokio owns the registration; qwertty installs no
    /// handler of its own.
    #[cfg(unix)]
    suspend: Signal,
    /// The `SIGCONT` (continue) listener.
    #[cfg(unix)]
    continue_: Signal,
    /// The `SIGTERM` (terminate) listener.
    #[cfg(unix)]
    terminate: Signal,
    /// The `SIGINT` (interrupt) listener.
    #[cfg(unix)]
    interrupt: Signal,
    /// The `Ctrl+C` console control listener, mapped to [`TerminalSignal::Interrupt`].
    #[cfg(windows)]
    ctrl_c: CtrlC,
    /// The `Ctrl+Break` console control listener, mapped to [`TerminalSignal::Interrupt`].
    #[cfg(windows)]
    ctrl_break: CtrlBreak,
    /// The console close listener, mapped to [`TerminalSignal::Terminate`].
    #[cfg(windows)]
    ctrl_close: CtrlClose,
}

#[cfg(unix)]
impl SignalStream {
    /// Awaits the next terminal-relevant signal and yields it as a typed [`TerminalSignal`].
    ///
    /// `select!`s across the four owned listeners and returns whichever fires first. This only
    /// *reports* the signal — it never calls [`suspend`](TokioTerminalSession::suspend),
    /// [`resume`](TokioTerminalSession::resume), or exits; the application decides the response
    /// (see [`signals`](TokioTerminalSession::signals) for the recommended wiring).
    ///
    /// Cancel-safe: dropping the future mid-await abandons only the wait; the listeners live on
    /// this value, so the next call resumes cleanly. Tokio coalesces pending deliveries of a
    /// given signal the same way it does for `SIGWINCH`.
    ///
    /// # Errors
    ///
    /// Returns a read error if a signal stream closes, which does not happen for these signals in
    /// normal operation.
    pub async fn next(&mut self) -> terminal::Result<TerminalSignal> {
        let signal = tokio::select! {
            received = self.suspend.recv() => received.map(|()| TerminalSignal::Suspend),
            received = self.continue_.recv() => received.map(|()| TerminalSignal::Continue),
            received = self.terminate.recv() => received.map(|()| TerminalSignal::Terminate),
            received = self.interrupt.recv() => received.map(|()| TerminalSignal::Interrupt),
        };
        signal.ok_or_else(|| {
            terminal::Error::read_terminal(io::Error::new(
                ErrorKind::UnexpectedEof,
                "terminal signal stream closed",
            ))
        })
    }
}

#[cfg(windows)]
impl SignalStream {
    /// Awaits the next console control event and yields it as a typed [`TerminalSignal`].
    ///
    /// `select!`s across the three console control listeners and returns whichever fires first,
    /// mapped through [`map_ctrl_event`]: `Ctrl+C` and `Ctrl+Break` to
    /// [`TerminalSignal::Interrupt`], the console close to [`TerminalSignal::Terminate`]. Suspend
    /// and continue never arrive on Windows (ADR 0022 §7). This only *reports* the event — the
    /// application decides the response (see [`signals`](TokioTerminalSession::signals)).
    ///
    /// Cancel-safe: dropping the future mid-await abandons only the wait; the listeners live on
    /// this value, so the next call resumes cleanly.
    ///
    /// # Errors
    ///
    /// Returns a read error if a console control stream closes, which does not happen in normal
    /// operation.
    pub async fn next(&mut self) -> terminal::Result<TerminalSignal> {
        let event = tokio::select! {
            received = self.ctrl_c.recv() => received.map(|()| ConsoleCtrlEvent::CtrlC),
            received = self.ctrl_break.recv() => received.map(|()| ConsoleCtrlEvent::CtrlBreak),
            received = self.ctrl_close.recv() => received.map(|()| ConsoleCtrlEvent::CtrlClose),
        };
        event.map(map_ctrl_event).ok_or_else(|| {
            terminal::Error::read_terminal(io::Error::new(
                ErrorKind::UnexpectedEof,
                "terminal signal stream closed",
            ))
        })
    }
}

/// A console control event the Windows [`SignalStream`] observes, before mapping to a
/// [`TerminalSignal`].
///
/// This is the pure input to [`map_ctrl_event`]: naming the three `tokio::signal::windows` sources
/// as a plain enum keeps the ctrl-event → signal mapping a pure function, unit-tested on every
/// platform even though the listeners themselves are Windows-only.
#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConsoleCtrlEvent {
    /// `Ctrl+C` — the interrupt request.
    CtrlC,
    /// `Ctrl+Break` — a second interrupt request the console delivers separately.
    CtrlBreak,
    /// The console close signal — the window is closing or the user logged off.
    CtrlClose,
}

/// Maps a Windows console control event to the typed [`TerminalSignal`] the stream reports.
///
/// `Ctrl+C` and `Ctrl+Break` both surface as [`TerminalSignal::Interrupt`] — a Windows console
/// has no separate "quit" the way a Unix terminal's `SIGQUIT` does, so both interrupt keys map to
/// the one interrupt signal an application already handles. The console close maps to
/// [`TerminalSignal::Terminate`], the graceful-shutdown request (ADR 0022 §7). There is no
/// suspend/continue source, so those variants are never produced here. Pure, so the mapping is
/// unit-tested on every platform.
#[cfg(any(windows, test))]
fn map_ctrl_event(event: ConsoleCtrlEvent) -> TerminalSignal {
    match event {
        ConsoleCtrlEvent::CtrlC | ConsoleCtrlEvent::CtrlBreak => TerminalSignal::Interrupt,
        ConsoleCtrlEvent::CtrlClose => TerminalSignal::Terminate,
    }
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
#[cfg(unix)]
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
#[cfg(unix)]
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
#[cfg(unix)]
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
#[cfg(unix)]
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

// The existing suite drives the real suspend/resume/detached-handoff/signal mechanics and the
// coalescing/esc-flush policy over a Unix `FakeDevice` and PTYs (rustix, signals, fds), so it is
// Unix-only. The Windows async driver's construction-and-teardown is covered by the
// `#[cfg(all(test, windows))]` live suite below (compile-checked via `check-cross`, run on the
// windows-latest CI job).
#[cfg(all(test, unix))]
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

    // `fcntl_setfl` moved out of the module body with the readiness transport; the flag-poking
    // tests (clearing/asserting `O_NONBLOCK` on the readiness fd, and the PTY-master
    // helper) still need it.
    use rustix::fs::fcntl_setfl;
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
    async fn tokio_probe_skips_a_dumb_terminal_without_writing_a_byte() {
        // R-QRY-5/FM-C5: mirrors the sync driver's skip test. The env is injected (not
        // process-global) so this test is safe to run in parallel; the public
        // `probe_capabilities` passes the real environment.
        let (mut session, mut peer, _calls) = fake_session_with_stub();

        let env = |key: &str| (key == "TERM").then(|| "dumb".to_owned());
        let caps = session
            .probe_capabilities_from_env(Duration::from_secs(30), env)
            .await
            .expect("a skipped probe is not an error");

        assert_eq!(
            peer.output().expect("output"),
            Vec::<u8>::new(),
            "a skipped probe must write nothing"
        );
        assert_eq!(caps.probe_skip, Some(crate::ProbeSkip::TermDumb));
        assert!(caps.is_all_unknown(), "nothing was asked: {caps:?}");
        // The skip snapshot is stored on the session like any probe result, so emit-gating
        // (`synchronized`) degrades on it the same way it degrades on unknown.
        assert_eq!(
            session.capabilities().and_then(|caps| caps.probe_skip),
            Some(crate::ProbeSkip::TermDumb)
        );
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
        let flags = fcntl_getfl(session.readiness().get_ref()).expect("getfl");
        fcntl_setfl(session.readiness().get_ref(), flags & !OFlags::NONBLOCK)
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
        let flags = fcntl_getfl(session.readiness().get_ref()).expect("getfl after resume");
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

    // --- detached handoff lifecycle (M6-S2) ------------------------------------------------------
    //
    // These drive `run_detached` over the same headless `FakeDevice` (and one PTY-backed session
    // for the termios resync, which is only observable on a real tty). No real editor/child is
    // ever spawned: the "child" is a synchronous closure that scrambles terminal state the way
    // `vi`/`stty` would, so the reclaim mechanics are exercised without a subprocess. A real
    // `$EDITOR` round-trip is owed to the attended/manual checklist (playbook M6-S2) — it
    // cannot be shown headlessly.

    #[tokio::test]
    async fn run_detached_resets_before_the_child_and_reapplies_after_returning_r() {
        let (mut session, peer, _calls) = fake_session_with_stub();

        // Enable a byte-based mode so the ledger has an observable reset/enable pair on the wire.
        session
            .enable_mouse(MouseMode::Normal)
            .await
            .expect("mouse");
        session.flush().await.expect("flush");

        // The closure is the synchronous "child". It captures the terminal peer so it can (1) prove
        // the ledger reset was written *before* it ran (the child sees a clean terminal), and (2)
        // simulate a window resize during the handoff. It returns the peer back out alongside a
        // sentinel `R`, so the test both recovers the peer and checks the `R` passthrough.
        let sentinel = 1234u32;
        let (mut peer, returned) = session
            .run_detached(move || {
                let mut peer = peer;
                // Drain the enable bytes and the reset the leave in step 1 just wrote. The reset
                // for Normal mouse mode (CSI ? 1000 l) must be present: the child
                // got a clean terminal.
                let before = peer.output().expect("drain output before child");
                assert!(
                    before.windows(8).any(|w| w == b"\x1b[?1000l"),
                    "run_detached wrote the mouse reset before the child ran, got {before:?}",
                );
                // The window changed size while the child owned the terminal.
                peer.set_size(TerminalSize::new(150, 50));
                (peer, sentinel)
            })
            .await
            .expect("run_detached");

        // The closure's `R` is returned verbatim.
        assert_eq!(
            returned, sentinel,
            "run_detached returns the closure's value"
        );

        // After returning, the ledger was re-entered: the mouse enable bytes are back on the wire.
        let after = peer.output().expect("drain output after reclaim");
        assert!(
            after.windows(8).any(|w| w == b"\x1b[?1000h"),
            "run_detached re-applied the mouse enable after the child, got {after:?}",
        );

        // Non-blocking was re-asserted on the fresh readiness registration (AsyncFd requires it).
        let flags = fcntl_getfl(session.readiness().get_ref()).expect("getfl after reclaim");
        assert!(
            flags.contains(OFlags::NONBLOCK),
            "run_detached must re-assert O_NONBLOCK on the fresh readiness fd",
        );

        // A synthetic resize carrying the current size is queued for next_event.
        let event = session.next_event().await.expect("synthetic resize");
        let resize = event.resize_event().expect("the queued event is a resize");
        assert_eq!(resize.cells(), TerminalSize::new(150, 50));
        assert_eq!(resize.pixels(), None, "a handoff resize carries no pixels");
    }

    #[tokio::test]
    async fn run_detached_leaves_the_session_readable_afterward() {
        // The session must be fully usable once the child returns: a fresh readiness registration
        // is taken over the same fd, so input fed after the handoff decodes normally.
        let (mut session, mut peer, _calls) = fake_session_with_stub();

        session
            .run_detached(|| {})
            .await
            .expect("run_detached with a no-op child");

        // Drain the synthetic resize the reclaim queued.
        let event = session.next_event().await.expect("synthetic resize first");
        assert!(event.resize_event().is_some(), "resize is delivered first");

        // Input arriving after the handoff is read through the fresh registration.
        peer.feed_input(b"a").expect("feed post-handoff input");
        let event = session.next_event().await.expect("post-handoff input");
        assert_eq!(
            event,
            Event::Key(KeyEvent::new(Key::Char('a')).with_text('a')),
            "the session reads correctly through the fresh readiness registration",
        );
    }

    #[tokio::test]
    async fn run_detached_restores_cooked_mode_for_a_child_that_scrambles_termios() {
        use rustix::termios::{LocalModes, tcgetattr};

        // FM-L9: a child like `vi`/`stty` can leave the terminal in cooked mode. The reclaim must
        // resync termios wholesale (re-enter raw), not trust what the child left. This is only
        // observable on a real tty, so it runs over a PTY; the child closure directly forces cooked
        // mode on the slave to stand in for `stty sane`.
        let Some((_master, slave_path, sized)) = open_test_pty_for_suspend() else {
            return;
        };
        let mut session =
            TokioTerminalSession::open_path(slave_path).expect("open PTY-backed session");

        // Raw mode is entered at construction: ICANON/ECHO are cleared on the slave now.
        let raw = tcgetattr(&sized).expect("tcgetattr before handoff");
        assert!(
            !raw.local_modes.contains(LocalModes::ICANON),
            "session entered raw mode at construction (ICANON cleared)",
        );

        session
            .run_detached(|| {
                // The "child" scrambles termios back to cooked mode, as `vi` or `stty sane` would.
                let mut attrs = tcgetattr(&sized).expect("child tcgetattr");
                attrs.local_modes |= LocalModes::ICANON | LocalModes::ECHO;
                rustix::termios::tcsetattr(&sized, rustix::termios::OptionalActions::Now, &attrs)
                    .expect("child forces cooked mode");
                // Confirm the child really left the terminal cooked before reclaim runs.
                let cooked = tcgetattr(&sized).expect("child tcgetattr after set");
                assert!(
                    cooked.local_modes.contains(LocalModes::ICANON),
                    "the child left the terminal in cooked mode",
                );
            })
            .await
            .expect("run_detached resyncs termios");

        // FM-L9 proven: after reclaim the terminal is back in the session's raw state, not the
        // cooked state the child left. The reclaim re-entered raw mode wholesale.
        let after = tcgetattr(&sized).expect("tcgetattr after reclaim");
        assert!(
            !after.local_modes.contains(LocalModes::ICANON),
            "run_detached must resync termios back to raw mode (FM-L9), ICANON must be cleared",
        );
        assert!(
            !after.local_modes.contains(LocalModes::ECHO),
            "run_detached must resync termios back to raw mode (FM-L9), ECHO must be cleared",
        );
    }

    #[tokio::test]
    async fn run_detached_disarms_during_the_child_and_rearms_after() {
        // The restore-handle disarm/re-arm is only observable on a live-terminal session (a
        // FakeDevice session carries no restore handle by design), so this uses a PTY-backed
        // session. Mirrors the suspend/resume disarm/re-arm test.
        let Some((_master, slave_path, _sized)) = open_test_pty_for_suspend() else {
            return;
        };
        let mut session =
            TokioTerminalSession::open_path(slave_path).expect("open PTY-backed session");

        let handle = session.restore_handle();
        // During the child the handle must be disarmed: the emergency hook must not fire while the
        // child owns the terminal. Assert it from inside the closure.
        let handle_for_child = session.restore_handle();
        session
            .run_detached(move || {
                assert!(
                    !handle_for_child.restore(),
                    "run_detached must disarm the restore handle before the child runs",
                );
                // `restore()` disarmed it as a side effect of the probe; re-arm so the reclaim's
                // re-arm below is the observable transition, not this probe.
                handle_for_child.arm();
            })
            .await
            .expect("run_detached");

        // After reclaim the handle is armed again: panic-safe teardown is live once more.
        assert!(
            handle.restore(),
            "run_detached must re-arm the restore handle so panic-safe teardown is live again",
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

    // --- terminal-signals stream (M6-S3) ---------------------------------------------------------
    //
    // Signal delivery is process-global. A real SIGINT/SIGTERM delivered to the test process would
    // abort the runner, and SIGTSTP would stop it — so we deliberately do NOT deliver any of the
    // four signals in a unit test (a CI-safety requirement, not just a convenience). We test the
    // enum's value semantics and that `signals()` installs the four listeners and hands back a
    // live, usable stream. A genuine end-to-end signal round-trip is owed to the
    // attended/manual checklist (playbook M6-S3): it cannot be exercised headlessly without
    // endangering the harness.

    #[test]
    fn terminal_signal_is_copy_eq_and_debug() {
        // Copy: using a value after a "move" only compiles because it is Copy.
        let suspend = TerminalSignal::Suspend;
        let copied = suspend;
        assert_eq!(suspend, copied);

        // Eq/PartialEq across the variants, and distinctness.
        assert_eq!(TerminalSignal::Continue, TerminalSignal::Continue);
        assert_ne!(TerminalSignal::Terminate, TerminalSignal::Interrupt);

        // Debug renders the variant name, which the derive guarantees.
        assert_eq!(format!("{:?}", TerminalSignal::Interrupt), "Interrupt");
    }

    #[tokio::test]
    async fn signals_installs_the_listeners_and_returns_a_usable_stream() {
        let (session, _peer) = fake_session();

        // Installing the four listeners must succeed; this is the app-owns-registration step
        // (design 01) and the only thing we can assert without delivering a real signal.
        let mut signals = session
            .signals()
            .expect("install the terminal-signal listeners");

        // The stream is live and usable: nothing is delivered, so `next()` must simply keep waiting
        // rather than resolve. A short timeout that elapses confirms the await parks correctly.
        let waited = timeout(Duration::from_millis(20), signals.next()).await;
        assert!(
            waited.is_err(),
            "with no signal delivered, next() stays pending (a live, parked stream)",
        );
    }
}

// The console ctrl-event → TerminalSignal mapping is a pure function, so it is unit-tested on every
// platform (this runs on the Unix CI too, not only windows-latest) — the neutral-logic rule.
#[cfg(test)]
mod ctrl_event_map_tests {
    use super::{ConsoleCtrlEvent, TerminalSignal, map_ctrl_event};

    #[test]
    fn ctrl_c_and_ctrl_break_map_to_interrupt_and_close_maps_to_terminate() {
        // Both interrupt keys collapse to the one Interrupt an application already handles (a
        // Windows console has no separate SIGQUIT-style quit); the console close is the graceful
        // Terminate. There is no suspend/continue source on Windows, so those are never produced.
        assert_eq!(
            map_ctrl_event(ConsoleCtrlEvent::CtrlC),
            TerminalSignal::Interrupt
        );
        assert_eq!(
            map_ctrl_event(ConsoleCtrlEvent::CtrlBreak),
            TerminalSignal::Interrupt
        );
        assert_eq!(
            map_ctrl_event(ConsoleCtrlEvent::CtrlClose),
            TerminalSignal::Terminate
        );
    }
}

// Live-console tests for the Windows async driver, run only on the windows-latest CI host (a real
// console is required) and compile-checked off Windows via `just check-cross --tests`. They
// construct a session over the real console through `open()`, write output, and tear down — proving
// the readiness worker's construction and its `leave`/`Drop` teardown link and run, plus the MW-3
// lifecycle surface (restore handle, typed-`Unsupported` job control, the detached handoff, and the
// console signal stream). Real keystroke reads are deliberately not asserted (no one types on CI),
// per the MW-2 spec.
#[cfg(all(test, windows))]
mod windows_live_tests {
    //! See the module comment above. Two groups of live-console tests share this module (and its
    //! one `CONSOLE` serialization lock):
    //!
    //! - **Construction + teardown** (MW-2/MW-3): open and tear the session down over the real
    //!   console without reading input.
    //! - **Read path end-to-end** (MW-5): inject console input records with `WriteConsoleInputW`
    //!   and assert the decoded [`Event`] the async session yields — the injection helpers
    //!   ([`inject_text`], [`inject_resize`]) and the `*_reads_*` tests below. This drives the real
    //!   `open()` session (no production test-seam), proving the transport `records -> worker ->
    //!   channel -> decoder -> next_event` and the query correlator over it.

    // SAFETY: `AllocConsole` takes no arguments and is only reached inside `console_guard`; the
    // crate lint is `unsafe_code = "deny"`, so this test-only console attach opts in the same
    // way the device's live tests do.
    #![allow(
        unsafe_code,
        reason = "attaching a console for the CI test binary is a single argument-free FFI call"
    )]
    // The console lock is a std `Mutex` held across the session's awaits to serialize
    // process-global console mode changes. `#[tokio::test]` runs each test on its own
    // current-thread runtime, so the guard never moves threads and cannot deadlock — the lint's
    // concern does not apply.
    #![allow(
        clippy::await_holding_lock,
        reason = "the console lock serializes process-global mode state; the current-thread test \
                  runtime makes holding it across awaits safe"
    )]

    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::{Mutex, Once};

    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        COORD, FlushConsoleInputBuffer, GetConsoleMode, GetConsoleOutputCP, INPUT_RECORD,
        INPUT_RECORD_0, KEY_EVENT, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0, WINDOW_BUFFER_SIZE_EVENT,
        WINDOW_BUFFER_SIZE_RECORD, WriteConsoleInputW,
    };

    use super::*;
    use crate::Key;

    /// Serializes console access: the console modes are process-global, so two tests entering raw
    /// mode at once would corrupt each other's captured/restored state.
    static CONSOLE: Mutex<()> = Mutex::new(());
    /// Ensures a console is attached exactly once for the whole test binary.
    static ALLOC: Once = Once::new();

    /// Attaches a console if the test process has none, then takes the serialization lock.
    fn console_guard() -> std::sync::MutexGuard<'static, ()> {
        ALLOC.call_once(|| {
            // SAFETY: AllocConsole takes no arguments and fails harmlessly when a console already
            // exists (the CI host may attach one), so its result is intentionally ignored.
            let _ = unsafe { windows_sys::Win32::System::Console::AllocConsole() };
        });
        CONSOLE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Opens a console device by name (`CONIN$`/`CONOUT$`) for a mode readback in these tests.
    fn open_console(name: &str) -> OwnedHandle {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is a null-terminated UTF-16 name owned for the call; the access/share/
        // disposition/flag arguments are plain values; the security and template pointers are null,
        // which CreateFileW permits.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        assert!(
            handle != INVALID_HANDLE_VALUE && !handle.is_null(),
            "open {name}"
        );
        // SAFETY: CreateFileW returned a valid handle; adopting it transfers sole ownership.
        unsafe { OwnedHandle::from_raw_handle(handle) }
    }

    /// Reads a console mode via `GetConsoleMode` for a restore-readback assertion.
    fn console_mode(handle: &OwnedHandle) -> u32 {
        let mut mode: u32 = 0;
        // SAFETY: `handle` is a live owned console handle; `mode` is a live out-param.
        let ok = unsafe { GetConsoleMode(handle.as_raw_handle() as HANDLE, &raw mut mode) };
        assert!(ok != 0, "GetConsoleMode");
        mode
    }

    /// Reads the process-global console output codepage via `GetConsoleOutputCP`.
    fn output_codepage() -> u32 {
        // SAFETY: GetConsoleOutputCP takes no arguments and reads process-global console state.
        unsafe { GetConsoleOutputCP() }
    }

    #[tokio::test]
    async fn open_writes_output_and_leave_tears_down_the_worker() {
        let _guard = console_guard();
        // Construction spawns the readiness worker over a duplicated console input handle and
        // enters raw mode.
        let mut session = TokioTerminalSession::open().expect("open console session");

        // Output flows through the transport's `WriteFile` path.
        session
            .text("qwertty MW-2 ready\r\n")
            .await
            .expect("write output");
        session.flush().await.expect("flush");

        // `leave` restores cooked mode and drops the transport, whose `Drop` signals the waker and
        // joins the worker within the bounded teardown contract — this call returning proves the
        // worker cannot wedge shutdown (RR-2).
        session.leave().await.expect("leave restores the console");
    }

    #[tokio::test]
    async fn dropping_the_session_joins_the_worker_without_hanging() {
        let _guard = console_guard();
        let session = TokioTerminalSession::open().expect("open console session");
        // Drop (not `leave`) must also tear the worker down: the transport's `Drop` sets the waker
        // and joins. If teardown could wedge, this test would hang rather than complete.
        drop(session);
    }

    #[tokio::test]
    async fn restore_handle_resets_the_console_and_is_disarm_once() {
        let _guard = console_guard();

        // Capture the live console modes and codepage BEFORE the session enters raw mode, so the
        // readback after `restore()` can prove the emergency path put back the captured originals
        // (the FM-W4 discipline), not synthesized defaults.
        let input = open_console("CONIN$");
        let output = open_console("CONOUT$");
        let original_input = console_mode(&input);
        let original_output = console_mode(&output);
        let original_codepage = output_codepage();

        let session = TokioTerminalSession::open().expect("open console session");
        let restore = session.restore_handle();

        // The armed handle restores exactly once and reports it.
        assert!(restore.restore(), "the armed restore performs restoration");
        assert_eq!(console_mode(&input), original_input, "input mode restored");
        assert_eq!(
            console_mode(&output),
            original_output,
            "output mode restored"
        );
        assert_eq!(output_codepage(), original_codepage, "codepage restored");

        // Disarm-once: a second restore is a no-op and reports false (the atomic swap).
        assert!(
            !restore.restore(),
            "a second restore is a no-op after the first disarmed the handle"
        );

        // Leaving after the restore already disarmed the handle is a no-op that still succeeds.
        session.leave().await.expect("leave after restore");
    }

    #[tokio::test]
    async fn suspend_and_resize_stream_are_typed_unsupported() {
        let _guard = console_guard();
        let mut session = TokioTerminalSession::open().expect("open console session");

        assert!(
            matches!(
                session.suspend().await,
                Err(terminal::Error::Unsupported { .. })
            ),
            "suspend is unsupported on Windows (no job control)"
        );
        assert!(
            matches!(
                session.resize_stream(),
                Err(terminal::Error::Unsupported { .. })
            ),
            "resize_stream is unsupported on Windows (resize is in band)"
        );

        session.leave().await.expect("leave");
    }

    #[tokio::test]
    async fn run_detached_round_trips_and_leaves_the_session_readable() {
        let _guard = console_guard();
        let mut session = TokioTerminalSession::open().expect("open console session");

        // The worker is paused for the handoff and resumed afterward; the closure's value round
        // trips through.
        let value = session
            .run_detached(|| 42_u32)
            .await
            .expect("run_detached round-trips");
        assert_eq!(value, 42);

        // The session must be fully usable after the resume: output still flows, no panic from a
        // resumed worker or a stale channel.
        session
            .text("after detached handoff\r\n")
            .await
            .expect("write after resume");
        session.flush().await.expect("flush after resume");
        session.leave().await.expect("leave after resume");
    }

    #[tokio::test]
    async fn signals_installs_console_ctrl_listeners_and_returns_a_usable_stream() {
        let _guard = console_guard();
        let session = TokioTerminalSession::open().expect("open console session");

        // Installing the three console control listeners must succeed; the stream is not awaited
        // (no ctrl event is delivered on CI), only constructed.
        let _signals = session
            .signals()
            .expect("install console control listeners");

        session.leave().await.expect("leave");
    }

    // --- MW-5: console input injection + read-path integration tests -----------------------------
    //
    // These drive the real `open()` session and assert the decoded events. Input is injected with
    // `WriteConsoleInputW` (the only way to feed a real console object — a pipe cannot back
    // `ReadConsoleInputW`), which the worker reads exactly as it would live input. Every read is
    // bounded by a timeout so a transport-wiring bug fails the CI job fast instead of hanging it.

    /// The upper bound on every `next_event`/query await in these tests.
    ///
    /// Injected records are already pending when the worker's wait wakes, so a correct transport
    /// delivers in milliseconds; this generous ceiling exists only so a *broken* transport fails
    /// the CI job promptly instead of hanging it.
    const READ_TIMEOUT: Duration = Duration::from_secs(5);

    /// Injects `text` as a run of key-down records — one `KEY_EVENT_RECORD` per UTF-16 code unit —
    /// into the shared console input buffer, exactly as conhost delivers VT bytes under VT input.
    ///
    /// Each unit becomes a `bKeyDown = 1`, `wRepeatCount = 1` record whose `UnicodeChar` is the
    /// unit; the readiness worker's `translate_key` reads `UnicodeChar` straight back and re-emits
    /// it (pairing surrogates across records through its persistent carry). So the three units of
    /// `"\x1b[C"` reach the decoder as the bytes `ESC [ C`, and an emoji's two surrogate units
    /// reach it as the one astral code point. The whole run is written in a single
    /// `WriteConsoleInputW` call so it lands atomically and the worker reads it as one batch —
    /// the session then decodes a complete sequence in one read, with no lone-`ESC` flush race.
    fn inject_text(input: &OwnedHandle, text: &str) {
        let records: Vec<INPUT_RECORD> = text
            .encode_utf16()
            .map(|unit| INPUT_RECORD {
                EventType: u16::try_from(KEY_EVENT).expect("KEY_EVENT fits in u16"),
                Event: INPUT_RECORD_0 {
                    KeyEvent: KEY_EVENT_RECORD {
                        bKeyDown: 1,
                        wRepeatCount: 1,
                        wVirtualKeyCode: 0,
                        wVirtualScanCode: 0,
                        uChar: KEY_EVENT_RECORD_0 { UnicodeChar: unit },
                        dwControlKeyState: 0,
                    },
                },
            })
            .collect();
        write_records(input, &records);
    }

    /// Injects a single `WINDOW_BUFFER_SIZE_EVENT` record into the shared console input buffer.
    ///
    /// The worker's resize path re-reads the live window rectangle via
    /// `GetConsoleScreenBufferInfo`, never the record's `dwSize` (which is the scrollback buffer),
    /// so the `dwSize` here is deliberately meaningless — the record only needs to be the type that
    /// drives resize synthesis.
    fn inject_resize(input: &OwnedHandle) {
        let record = INPUT_RECORD {
            EventType: u16::try_from(WINDOW_BUFFER_SIZE_EVENT)
                .expect("WINDOW_BUFFER_SIZE_EVENT fits in u16"),
            Event: INPUT_RECORD_0 {
                WindowBufferSizeEvent: WINDOW_BUFFER_SIZE_RECORD {
                    dwSize: COORD { X: 0, Y: 0 },
                },
            },
        };
        write_records(input, &[record]);
    }

    /// Writes a slice of input records into the console input buffer via `WriteConsoleInputW`.
    ///
    /// Constructing the `INPUT_RECORD` union above needs no `unsafe` (only *reading* an inactive
    /// union field would); the sole FFI is this one call.
    fn write_records(input: &OwnedHandle, records: &[INPUT_RECORD]) {
        let count = u32::try_from(records.len()).expect("record count fits in u32");
        let mut written: u32 = 0;
        // SAFETY: `input` is a live owned console input handle opened with GENERIC_WRITE; `records`
        // is readable for `count` entries; `written` is a live out-param.
        let ok = unsafe {
            WriteConsoleInputW(
                input.as_raw_handle() as HANDLE,
                records.as_ptr(),
                count,
                &raw mut written,
            )
        };
        assert!(ok != 0, "WriteConsoleInputW");
        assert_eq!(written as usize, records.len(), "all records written");
    }

    /// Empties the shared console input buffer so a test starts from a known-clean slate.
    ///
    /// The console input buffer is process-global and survives across the serialized tests; this
    /// drops any record a prior test left behind (or any stray focus/menu record the host queued)
    /// before injection, so each test observes only the records it wrote.
    fn flush_console_input(input: &OwnedHandle) {
        // SAFETY: `input` is a live owned console input handle.
        let ok = unsafe { FlushConsoleInputBuffer(input.as_raw_handle() as HANDLE) };
        assert!(ok != 0, "FlushConsoleInputBuffer");
    }

    /// Awaits the next key event, bounded by [`READ_TIMEOUT`], skipping a stray resize.
    ///
    /// A freshly attached console can surface an unsolicited `WINDOW_BUFFER_SIZE_EVENT`; it carries
    /// no key intent, so it is skipped rather than allowed to fail a key assertion. Any other
    /// unexpected event is a real transport defect and panics.
    async fn expect_key(session: &mut TokioTerminalSession<Terminal>) -> Key {
        let deadline = Instant::now() + READ_TIMEOUT;
        loop {
            let event = timeout_at(deadline, session.next_event())
                .await
                .expect("a key event arrives within the CI read timeout")
                .expect("next_event decodes without error");
            match event {
                Event::Key(key) => return key.key(),
                // A stray resize carries no key intent: skip it and read again.
                Event::Resize(_) => {}
                other => panic!("unexpected event while awaiting a key: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn open_reads_a_plain_key_from_an_injected_record() {
        let _guard = console_guard();
        let injector = open_console("CONIN$");
        flush_console_input(&injector);

        let mut session = TokioTerminalSession::open().expect("open console session");

        // One record (`UnicodeChar = 'a'`) -> one VT byte -> `Key::Char('a')`.
        inject_text(&injector, "a");
        assert_eq!(
            expect_key(&mut session).await,
            Key::Char('a'),
            "an injected 'a' record decodes to Key::Char('a')"
        );

        session.leave().await.expect("leave after reads");
    }

    #[tokio::test]
    async fn open_reassembles_a_multi_record_arrow_sequence() {
        let _guard = console_guard();
        let injector = open_console("CONIN$");
        flush_console_input(&injector);

        let mut session = TokioTerminalSession::open().expect("open console session");

        // Three separate records (`ESC` `[` `C`) reassemble through the channel and decoder into a
        // single right-arrow key — the multi-record reassembly proof.
        inject_text(&injector, "\x1b[C");
        assert_eq!(
            expect_key(&mut session).await,
            Key::Right,
            "the three-record ESC [ C sequence decodes to one right-arrow key"
        );

        session.leave().await.expect("leave after reads");
    }

    #[tokio::test]
    async fn open_carries_an_astral_surrogate_pair_across_records() {
        let _guard = console_guard();
        let injector = open_console("CONIN$");
        flush_console_input(&injector);

        let mut session = TokioTerminalSession::open().expect("open console session");

        // U+1F600 is a surrogate pair in UTF-16: two records, translated with the worker's carry
        // pairing them into one astral code point.
        let emoji = '\u{1f600}';
        inject_text(&injector, "\u{1f600}");
        assert_eq!(
            expect_key(&mut session).await,
            Key::Char(emoji),
            "the two surrogate records pair into one astral Key::Char"
        );

        session.leave().await.expect("leave after reads");
    }

    #[tokio::test]
    async fn request_cursor_position_correlates_an_injected_report() {
        let _guard = console_guard();
        let injector = open_console("CONIN$");
        flush_console_input(&injector);

        let mut session = TokioTerminalSession::open().expect("open console session");

        // Queue the report before the request: the request writes DSR to output (the console
        // discards it) and then reads, and the correlator only matches replies fed *after* it
        // registers the expectation, so a report already waiting in the worker's channel is
        // consumed by the very first read of the query's deadline loop.
        inject_text(&injector, "\x1b[10;20R");
        let report = session
            .request_cursor_position(READ_TIMEOUT)
            .await
            .expect("the injected cursor report resolves the query");
        assert_eq!(report.row(), 10, "the correlator reports the injected row");
        assert_eq!(
            report.column(),
            20,
            "the correlator reports the injected column"
        );

        session.leave().await.expect("leave after reads");
    }

    #[tokio::test]
    async fn open_reads_a_resize_from_an_injected_record() {
        let _guard = console_guard();
        let injector = open_console("CONIN$");
        flush_console_input(&injector);

        let mut session = TokioTerminalSession::open().expect("open console session");

        // A window-buffer-size record drives in-band resize synthesis from the live window rect.
        inject_resize(&injector);
        let event = timeout_at(Instant::now() + READ_TIMEOUT, session.next_event())
            .await
            .expect("a resize event arrives within the CI read timeout")
            .expect("next_event decodes without error");
        let Event::Resize(resize) = event else {
            panic!("expected a resize event, got {event:?}");
        };
        // The geometry is the live console window, not the record's `dwSize`; assert only that it
        // is a sane, positive extent (the same tolerance as the device's `size` live test).
        let cells = resize.cells();
        assert!(cells.columns() > 0, "resize reports positive columns");
        assert!(cells.rows() > 0, "resize reports positive rows");

        session.leave().await.expect("leave after reads");
    }

    #[tokio::test]
    async fn leave_restores_the_console_after_reads() {
        let _guard = console_guard();
        let injector = open_console("CONIN$");
        flush_console_input(&injector);

        // Capture the live modes/codepage before the session enters raw mode, so the post-`leave`
        // readback proves teardown put the captured originals back after a real read.
        let output = open_console("CONOUT$");
        let original_input = console_mode(&injector);
        let original_output = console_mode(&output);
        let original_codepage = output_codepage();

        let mut session = TokioTerminalSession::open().expect("open console session");

        inject_text(&injector, "a");
        assert_eq!(
            expect_key(&mut session).await,
            Key::Char('a'),
            "the session reads before teardown"
        );

        session.leave().await.expect("leave restores the console");

        assert_eq!(
            console_mode(&injector),
            original_input,
            "input mode restored"
        );
        assert_eq!(
            console_mode(&output),
            original_output,
            "output mode restored"
        );
        assert_eq!(output_codepage(), original_codepage, "codepage restored");
    }
}
