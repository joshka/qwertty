//! Terminal session lifecycle.
//!
//! A session is the first application-facing owner above the low-level terminal device. It enters
//! raw mode, preserves output ordering, reads raw input bytes, exposes explicit flushing, and gives
//! callers an explicit leave path for terminal-mode cleanup errors.
//!
//! Every reversible state change a session makes is recorded in an internal mode ledger with the
//! actions that apply and undo it. All lifecycle paths replay that one ledger:
//! [`TerminalSession::enter`] applies it, and orderly [`TerminalSession::leave`], drop, and the
//! panic-safe [`RestoreHandle`] undo it in reverse enablement order.

mod kitty;
mod ledger;
#[cfg(any(unix, windows))]
mod restore;

#[cfg(unix)]
use std::collections::VecDeque;
#[cfg(unix)]
use std::time::{Duration, Instant};

pub use kitty::{KittyKeyboardFlags, KittyKeyboardGrant};
#[cfg(windows)]
use restore::ConsoleModeRestore;
#[cfg(any(unix, windows))]
pub use restore::RestoreHandle;

#[cfg(unix)]
use crate::caps::{Capabilities, ProbeBundle, store_bundle_reply};
use crate::commands::terminal::MouseMode;
#[cfg(unix)]
use crate::correlate::{Correlator, Expectation, ExpectationId, Feed, Reply, Resolution};
use crate::policy::{Policy, PolicyGate};
#[cfg(unix)]
use crate::report::{CursorPositionReport, TerminalStatusReport};
use crate::session::ledger::{ModeKind, ModeLedger, StateAction};
use crate::{
    Command, DeviceMode, InputBytes, Terminal, TerminalDevice, TerminalSize, commands, terminal,
};
#[cfg(unix)]
use crate::{Event, SemanticDecoder};

/// An active terminal session over a [`TerminalDevice`].
///
/// `TerminalSession` owns its device for application output. The default device is a live
/// [`Terminal`]; tests and embedding environments can run the same session headless over any
/// other [`TerminalDevice`], such as `FakeDevice`, through
/// [`TerminalSession::from_device`].
///
/// Creating a session enters raw mode so later input and query layers can receive terminal bytes
/// directly. The lifecycle is re-entrant: [`TerminalSession::leave`] restores the terminal
/// without consuming the session, and [`TerminalSession::enter`] re-applies session state, so a
/// line-editor-shaped caller can cycle the pair once per prompt over one long-lived session. The
/// cycle replays recorded mode actions only — it never reopens or re-registers the device.
///
/// Restoration runs at most once per entered period, on whichever path claims it first:
/// `leave`, drop, or the panic-safe [`RestoreHandle`] on Unix. Dropping an entered session still
/// restores the terminal, but drop-time failures cannot be reported.
///
/// This session is synchronous and runtime-neutral: it writes through the terminal-device boundary
/// directly, reads input as raw bytes with [`read_input`](Self::read_input), and answers
/// [`report`](crate::report) queries with a blocking poll loop. For decoded [`Event`](crate::Event)
/// delivery and live query routing over an async runtime, use `TokioTerminalSession` (the `tokio`
/// feature).
///
/// # What you can do
///
/// - **Output:** [`command`](Self::command), [`text`](Self::text), [`flush`](Self::flush).
/// - **Input:** [`read_input`](Self::read_input) for one raw read;
///   [`request_cursor_position`](Self::request_cursor_position) and
///   [`request_terminal_status`](Self::request_terminal_status) for blocking queries.
/// - **Modes:** [`enable_mouse`](Self::enable_mouse),
///   [`enable_focus_events`](Self::enable_focus_events),
///   [`enable_bracketed_paste`](Self::enable_bracketed_paste),
///   [`enable_in_band_resize`](Self::enable_in_band_resize), and
///   [`push_kitty_keyboard`](Self::push_kitty_keyboard) — each recorded in the ledger and undone on
///   leave.
/// - **Security:** [`set_clipboard`](Self::set_clipboard) behind the [`Policy`] gate
///   ([`policy`](Self::policy) / [`set_policy`](Self::set_policy)).
/// - **Lifecycle:** [`enter`](Self::enter) / [`leave`](Self::leave) cycle raw mode re-entrantly.
///
/// # Example
///
/// ```no_run
/// use qwertty::{ProtocolPosition, TerminalSession, commands};
///
/// fn main() -> qwertty::Result<()> {
///     let mut session = TerminalSession::open()?;
///
///     session
///         .command(commands::screen::clear())?
///         .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))?
///         .text("session active\r\n")?
///         .flush()?;
///
///     session.leave()
/// }
/// ```
#[derive(Debug)]
pub struct TerminalSession<D: TerminalDevice = Terminal> {
    device: D,
    ledger: ModeLedger,
    entered: bool,
    policy: Policy,
    #[cfg(any(unix, windows))]
    restore: Option<RestoreHandle>,
    /// The sans-io query state driven by the synchronous query helpers (design 04, review-02 §2).
    ///
    /// This is the second, no-Tokio consumer of the sans-io correlator: the blocking query helpers
    /// (for example [`request_cursor_position`](Self::request_cursor_position)) register an
    /// `Expectation` here, poll the device fd, decode bytes into [`Event`]s, and feed them
    /// through the correlator until the reply completes. It is only built on Unix, where the
    /// `poll`-based readiness seam ([`as_fd`](Self::as_fd)) and live terminal I/O exist.
    #[cfg(unix)]
    query: QueryState,
}

/// The synchronous query driver's sans-io state (design 04, review-02 §2).
///
/// Owns the decoder and correlator the blocking query helpers drive, plus the raw bytes of any
/// input that arrived during a query but was **not** the reply. That leftover-byte buffer is what
/// keeps typeahead safe: input read while waiting for a reply is not swallowed by the query — the
/// next [`read_input`](TerminalSession::read_input) returns it first, in arrival order, before
/// touching the device again (FM-Q1). The state mirrors the Tokio driver's
/// `decoder`/`correlator`/`pending`/`active_query`, minus the reactor; the only shape difference is
/// that this driver returns **raw bytes** from `read_input` (the narrow-primitive rule), so it
/// buffers raw typeahead bytes rather than decoded events.
#[cfg(unix)]
#[derive(Debug)]
struct QueryState {
    /// The semantic decoder turning each read's raw bytes into typed events (design 02).
    ///
    /// Held across queries so a reply split over two reads still assembles: the decoder is the
    /// same stateful parser the Tokio driver keeps on its session.
    decoder: SemanticDecoder,
    /// Raw bytes fed to the decoder that have not yet completed an event, carried across reads.
    ///
    /// The decoder holds bytes across read boundaries (a parked text run, a half-finished
    /// sequence), so its raw carry is tracked here in lockstep: it prefixes the next read's
    /// bytes so a completed event's full raw byte span is known for byte-accurate typeahead
    /// attribution.
    decoder_carry: Vec<u8>,
    /// The sans-io correlator matching query replies to expectations (design 03).
    correlator: Correlator,
    /// Raw bytes read during a query that did not carry the reply, awaiting a later `read_input`.
    ///
    /// A query buffers each non-reply event's raw bytes here (in arrival order) so the unrelated
    /// input stays deliverable as ordinary bytes — typeahead is never consumed or misattributed by
    /// the query (FM-Q1). Drained front-first by [`read_input`](TerminalSession::read_input).
    typeahead: VecDeque<u8>,
    /// The id of the single in-flight query expectation, if any; swept before the next query.
    active_query: Option<ExpectationId>,
}

#[cfg(unix)]
impl QueryState {
    /// Creates empty query state (no decoder progress, no expectations, no buffered typeahead).
    fn new() -> Self {
        Self {
            decoder: SemanticDecoder::new(),
            decoder_carry: Vec::new(),
            correlator: Correlator::new(),
            typeahead: VecDeque::new(),
            active_query: None,
        }
    }
}

impl TerminalSession<Terminal> {
    /// Opens the current controlling terminal and starts a session.
    ///
    /// This opens the current terminal through [`Terminal::open`] and enters raw mode before
    /// returning. No alternate screen, cursor visibility, mouse mode, paste mode, or vendor
    /// protocol state is changed by this constructor.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal cannot be opened or raw mode cannot be entered.
    pub fn open() -> terminal::Result<Self> {
        Self::from_terminal(Terminal::open()?)
    }

    /// Starts a session from an already opened terminal.
    ///
    /// This is useful for tests and embedding environments that already resolved the terminal
    /// device they want qwertty to own.
    ///
    /// # Errors
    ///
    /// Returns an error when the emergency restore path cannot be prepared or raw mode cannot be
    /// entered.
    pub fn from_terminal(terminal: Terminal) -> terminal::Result<Self> {
        #[cfg(unix)]
        let restore = Some(RestoreHandle::new(
            emergency_device(&terminal)?,
            terminal.cooked_mode(),
        ));
        #[cfg(windows)]
        let restore = {
            let (output, input, modes) = emergency_console(&terminal)?;
            Some(RestoreHandle::new(output, input, modes))
        };

        let mut session = Self {
            device: terminal,
            ledger: ModeLedger::new(),
            entered: false,
            policy: Policy::default(),
            #[cfg(any(unix, windows))]
            restore,
            #[cfg(unix)]
            query: QueryState::new(),
        };
        session.record_initial_state();
        session.enter()?;
        Ok(session)
    }

    /// Returns a panic-safe restore handle for this session.
    ///
    /// The handle stays valid without borrowing the session, so it can live inside a panic hook
    /// installed once for the whole program. See [`RestoreHandle`] for the hook pattern and what
    /// the emergency path covers.
    #[cfg(any(unix, windows))]
    #[must_use]
    #[expect(
        clippy::missing_panics_doc,
        reason = "from_terminal always constructs the handle, so the expect cannot fire"
    )]
    pub fn restore_handle(&self) -> RestoreHandle {
        self.restore
            .clone()
            .expect("sessions over a live terminal always carry a restore handle")
    }
}

impl<D: TerminalDevice> TerminalSession<D> {
    /// Starts a session over any terminal device.
    ///
    /// The session behaves exactly as over a live terminal, minus the pieces that need a real
    /// one: the panic-safe restore handle is only available through
    /// [`TerminalSession::restore_handle`] on live-terminal sessions.
    ///
    /// # Errors
    ///
    /// Returns an error when raw mode cannot be entered.
    pub fn from_device(device: D) -> terminal::Result<Self> {
        let mut session = Self {
            device,
            ledger: ModeLedger::new(),
            entered: false,
            policy: Policy::default(),
            #[cfg(any(unix, windows))]
            restore: None,
            #[cfg(unix)]
            query: QueryState::new(),
        };
        session.record_initial_state();
        session.enter()?;
        Ok(session)
    }

    /// Re-applies session terminal state after a [`TerminalSession::leave`].
    ///
    /// Entering replays the recorded mode actions in enablement order and re-arms the emergency
    /// restore path. It never reopens the device, so cycling enter and leave once per prompt
    /// stays as cheap as the mode changes themselves. Entering an already-entered session does
    /// nothing.
    ///
    /// # Errors
    ///
    /// Returns the first error encountered while applying terminal state.
    pub fn enter(&mut self) -> terminal::Result<()> {
        if self.entered {
            return Ok(());
        }

        let mut first_error = None;
        for action in self.ledger.apply_actions() {
            let result = match action {
                StateAction::WriteBytes(bytes) => self.device.write_all(bytes),
                StateAction::SetMode(mode) => self.device.set_mode(*mode),
            };
            if let Err(error) = result
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        self.entered = true;

        #[cfg(any(unix, windows))]
        if let Some(restore) = &self.restore {
            restore.publish_blob(&self.ledger.protocol_undo_bytes());
            restore.arm();
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Restores the terminal without consuming the session.
    ///
    /// This is the orderly cleanup path. It replays the session's mode ledger in reverse
    /// enablement order, attempts every step even after a failure, flushes, and reports the
    /// first error. Today the ledger holds raw-mode restoration, the input-mode enables, alternate
    /// screen, and cursor visibility; paste mode and vendor protocol cleanup join it in later
    /// slices.
    ///
    /// Leaving is idempotent: if the session already left, or the panic-safe restore handle
    /// already restored the terminal, `leave` does nothing and returns success. Call
    /// [`TerminalSession::enter`] to re-apply session state afterwards.
    ///
    /// Call [`TerminalSession::flush`] before `leave` when the visibility ordering of your own
    /// output matters.
    ///
    /// # Errors
    ///
    /// Returns the first error encountered while restoring terminal state.
    pub fn leave(&mut self) -> terminal::Result<()> {
        if !self.entered {
            return Ok(());
        }
        self.entered = false;

        #[cfg(any(unix, windows))]
        if let Some(restore) = &self.restore
            && !restore.disarm()
        {
            return Ok(());
        }

        self.restore_state()
    }

    /// Returns the current terminal size.
    ///
    /// The result is a snapshot. This method does not subscribe to future resize events.
    ///
    /// Degenerate sizes are never returned: when the device reports zero or `u16::MAX`
    /// dimensions (piped stdio, some CI environments, and IDE terminals do), the session falls
    /// back to the `COLUMNS` and `LINES` environment variables. Environment values are the
    /// caller's own configuration, not a measurement. When neither source yields a usable size,
    /// an error is returned so the caller can apply its own default.
    ///
    /// # Errors
    ///
    /// Returns an error when neither the device nor the environment yields a usable size.
    pub fn size(&self) -> terminal::Result<TerminalSize> {
        match self.device.size() {
            Ok(size) if size_is_usable(size) => Ok(size),
            Ok(size) => environment_size().ok_or(terminal::Error::InvalidTerminalSize {
                columns: size.columns(),
                rows: size.rows(),
            }),
            Err(error) => environment_size().ok_or(error),
        }
    }

    /// Returns the session's current security policy.
    ///
    /// The policy gates side-effecting and exfiltrating features (clipboard write/read,
    /// notifications, file transfer, mux passthrough). A new session starts at
    /// [`Policy::restricted`], the safe default. [`Policy`] is [`Copy`], so this returns a value.
    #[must_use]
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// Sets the session's security policy, returning `&mut Self` so it chains with other setters.
    ///
    /// The new policy takes effect for every later gated call (for example
    /// [`set_clipboard`](Self::set_clipboard)). It does not retroactively affect bytes already
    /// written.
    pub fn set_policy(&mut self, policy: Policy) -> &mut Self {
        self.policy = policy;
        self
    }

    /// Builder-style variant of [`set_policy`](Self::set_policy): sets the policy and returns the
    /// session by value.
    ///
    /// This composes with the constructors, letting a caller open a session and choose its policy
    /// in one expression: `TerminalSession::from_device(device)?.with_policy(Policy::trusted())`.
    #[must_use]
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// Writes a clipboard selection through the session's [`Policy`] gate (OSC 52, FM-X4).
    ///
    /// When the policy allows [`PolicyGate::ClipboardWrite`], this emits
    /// [`commands::osc::set_clipboard`] through the same immediate-write path as
    /// [`command`](Self::command) and returns `Ok(self)` so it chains like the other session
    /// methods. A restricted (default) session allows clipboard writes, because the terminal itself
    /// gates the sensitive paste-back direction (FM-X4).
    ///
    /// Clipboard writes are an exfiltration surface, not merely a formatting choice: any emitted
    /// output can reach the system clipboard (MITRE ATT&CK T1115). This gate is the session's own
    /// opt-in above the encode-only [`commands::osc::set_clipboard`] builder, which has no policy
    /// of its own.
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::PolicyDenied`] with [`PolicyGate::ClipboardWrite`] — **without
    /// writing anything** — when the policy has clipboard write off. Otherwise returns the terminal
    /// device's write error if the encoded bytes cannot be written.
    pub fn set_clipboard(
        &mut self,
        selection: commands::osc::ClipboardSelection,
        data: &[u8],
    ) -> terminal::Result<&mut Self> {
        if !self.policy.allows(PolicyGate::ClipboardWrite) {
            return Err(terminal::Error::PolicyDenied {
                gate: PolicyGate::ClipboardWrite,
            });
        }
        self.command(commands::osc::set_clipboard(selection, data))
    }

    /// Writes one terminal command immediately.
    ///
    /// Commands, raw bytes, and text are written in the order their session methods are called.
    /// The command bytes are not flushed until [`TerminalSession::flush`] is called or the
    /// operating system decides to make them visible.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write all encoded bytes.
    pub fn command(&mut self, command: impl AsRef<Command>) -> terminal::Result<&mut Self> {
        let mut bytes = Vec::new();
        command.as_ref().encode(&mut bytes);
        self.bytes(bytes)
    }

    /// Writes raw bytes immediately.
    ///
    /// This method does not inspect, escape, or validate bytes. Use it for renderer output that is
    /// already encoded. Prefer [`TerminalSession::text`] for ordinary UTF-8 render text.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write all bytes.
    pub fn bytes(&mut self, bytes: impl AsRef<[u8]>) -> terminal::Result<&mut Self> {
        self.device.write_all(bytes.as_ref())?;
        Ok(self)
    }

    /// Writes UTF-8 render text immediately.
    ///
    /// This method does not escape control characters. Renderers that accept user-controlled text
    /// should perform their own escaping policy before writing to a terminal stream.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write all text bytes.
    pub fn text(&mut self, text: impl AsRef<str>) -> terminal::Result<&mut Self> {
        self.bytes(text.as_ref())
    }

    /// Reads raw terminal input bytes into `buffer`.
    ///
    /// This method returns one operating-system read as [`InputBytes`]. It does not decode UTF-8,
    /// parse Escape sequences, match terminal query responses, classify keys, or apply paste,
    /// mouse, focus, graphics, clipboard, or vendor protocol policy.
    ///
    /// In raw mode, the returned bytes are the foundation for later event and query-routing
    /// layers. A zero-length buffer returns an empty input value without reading from the terminal.
    ///
    /// # Typeahead from a blocking query (Unix)
    ///
    /// On Unix, a blocking query helper such as
    /// [`request_cursor_position`](Self::request_cursor_position) may read input that is **not**
    /// the reply while it waits — typeahead the user sent before the terminal answered. Those
    /// bytes are not consumed by the query; they are buffered on the session and returned here
    /// first, in arrival order, before any new device read. This method therefore drains that
    /// buffer ahead of touching the terminal, so a query never swallows a keystroke (FM-Q1).
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot read input.
    pub fn read_input(&mut self, buffer: &mut [u8]) -> terminal::Result<InputBytes> {
        if buffer.is_empty() {
            return Ok(InputBytes::default());
        }

        // Deliver buffered typeahead from a prior query before reading the device again, so
        // unrelated input a query saw but did not match is never lost (FM-Q1). Only when the buffer
        // is empty do we perform a real read.
        #[cfg(unix)]
        if !self.query.typeahead.is_empty() {
            let take = self.query.typeahead.len().min(buffer.len());
            let drained: Vec<u8> = self.query.typeahead.drain(..take).collect();
            return Ok(InputBytes::new(drained));
        }

        let len = self.device.read(buffer)?;
        Ok(InputBytes::new(buffer[..len].to_vec()))
    }

    /// Flushes buffered terminal output.
    ///
    /// Call this when the preceding command, byte, and text writes must be visible before later
    /// application work continues.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot flush output.
    pub fn flush(&mut self) -> terminal::Result<&mut Self> {
        self.device.flush()?;
        Ok(self)
    }

    /// Returns the readable file descriptor behind the owned device, when one exists.
    ///
    /// This is the runtime-neutral readiness seam a synchronous, no-async-runtime caller needs: it
    /// exposes the same descriptor [`TerminalDevice::as_fd`] reports, so a program can wait for the
    /// terminal to become readable with its own poller (for example `rustix::event::poll`) instead
    /// of blocking in [`read_input`](Self::read_input) or pulling in an async runtime. The session
    /// keeps ownership of the device, its mode ledger, and its restore paths; the borrowed
    /// descriptor lives only as long as the borrow of `self`.
    ///
    /// Returns `None` for a device with no pollable descriptor (for example a headless
    /// `FakeDevice`), matching the trait's own contract.
    #[cfg(unix)]
    #[must_use]
    pub fn as_fd(&self) -> Option<std::os::fd::BorrowedFd<'_>> {
        self.device.as_fd()
    }

    /// Returns a shared reference to the owned device.
    ///
    /// A driver that registers the same device's descriptor with a runtime reactor (the Tokio
    /// session) uses this to reach the device the session owns — for its pollable fd and its path —
    /// without taking it away from the session's mode ledger and restore paths.
    #[cfg_attr(
        not(all(feature = "tokio", any(unix, windows))),
        expect(
            dead_code,
            reason = "tokio async-driver helper (unix or windows); absent from other builds"
        )
    )]
    pub(crate) fn device(&self) -> &D {
        &self.device
    }

    /// Records the terminal state every session applies on entry.
    fn record_initial_state(&mut self) {
        self.ledger.record(
            ModeKind::Raw,
            StateAction::SetMode(DeviceMode::Raw),
            StateAction::SetMode(DeviceMode::Cooked),
        );
    }

    /// Records granted kitty keyboard flags in the ledger so teardown pops the granted reality.
    ///
    /// The apply action re-pushes the granted flags (`CSI > flags u`) on a later `enter`; the undo
    /// pops the single pushed entry (`CSI < 1 u`). This records the *granted* set, not the
    /// requested one (verify-after-push, design 06): the driver calls this only after querying the
    /// terminal, so the ledger — and the emergency blob it feeds — never claims an enhancement the
    /// terminal did not turn on. Recording the empty granted set records nothing, leaving no entry
    /// to pop.
    ///
    /// The push bytes are already on the wire when the driver calls this (the request wrote them to
    /// run the query), so this records the entry for lifecycle replay without re-emitting; the
    /// `enter` replay path emits the apply bytes on a subsequent re-entry.
    #[cfg_attr(
        not(all(feature = "tokio", any(unix, windows))),
        expect(
            dead_code,
            reason = "tokio async-driver helper (unix or windows); absent from other builds"
        )
    )]
    pub(crate) fn record_kitty_keyboard(&mut self, granted: KittyKeyboardFlags) {
        if granted.is_empty() {
            return;
        }
        let mut push = Vec::new();
        commands::terminal::push_kitty_keyboard_flags(granted).encode(&mut push);
        let mut pop = Vec::new();
        commands::terminal::pop_kitty_keyboard_flags().encode(&mut pop);
        self.ledger.record(
            ModeKind::KittyKeyboard,
            StateAction::WriteBytes(push),
            StateAction::WriteBytes(pop),
        );

        // Refresh the emergency blob so a panic between now and the next `enter` still resets the
        // keyboard flags (the ledger's emergency form is the stronger pop-all).
        #[cfg(any(unix, windows))]
        if let Some(restore) = &self.restore {
            restore.publish_blob(&self.ledger.protocol_undo_bytes());
        }
    }

    /// Enables mouse reporting for the given tracking mode, paired with SGR coordinates (1006).
    ///
    /// This writes the enable bytes (`CSI ? N h CSI ? 1006 h`) to the terminal now and records the
    /// change in the mode ledger so `enter` re-applies it and `leave`/drop/the emergency path reset
    /// it (`CSI ? 1006 l CSI ? N l`). Because it is a byte-based ledger entry, the reset bytes flow
    /// into the emergency blob automatically, so a panic teardown turns mouse reporting back off.
    ///
    /// Calling it again with a different [`MouseMode`] replaces the entry in place, so switching
    /// tracking modes never leaves a stale mode enabled.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write the enable bytes.
    pub fn enable_mouse(&mut self, mode: MouseMode) -> terminal::Result<&mut Self> {
        self.enable_mode(
            ModeKind::Mouse,
            &commands::terminal::enable_mouse(mode),
            &commands::terminal::disable_mouse(mode),
        )
    }

    /// Enables focus reporting (mode 1004).
    ///
    /// This writes `CSI ? 1004 h` now and records the change so the session re-applies it on
    /// `enter` and resets it (`CSI ? 1004 l`) on `leave`/drop/emergency. With focus reporting
    /// on, the terminal sends `CSI I`/`CSI O`, decoded to [`FocusEvent`](crate::FocusEvent).
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write the enable bytes.
    pub fn enable_focus_events(&mut self) -> terminal::Result<&mut Self> {
        self.enable_mode(
            ModeKind::Focus,
            &commands::terminal::enable_focus_events(),
            &commands::terminal::disable_focus_events(),
        )
    }

    /// Enables bracketed paste (mode 2004).
    ///
    /// This writes `CSI ? 2004 h` now and records the change so the session re-applies it on
    /// `enter` and resets it (`CSI ? 2004 l`) on `leave`/drop/emergency. With bracketed paste
    /// on, pasted text arrives wrapped in `ESC [ 200 ~ … ESC [ 201 ~` and decodes to
    /// [`PasteEvent`](crate::PasteEvent) segments — delivered as data, never as typed keys.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write the enable bytes.
    pub fn enable_bracketed_paste(&mut self) -> terminal::Result<&mut Self> {
        self.enable_mode(
            ModeKind::BracketedPaste,
            &commands::terminal::enable_bracketed_paste(),
            &commands::terminal::disable_bracketed_paste(),
        )
    }

    /// Enables in-band resize reporting (mode 2048).
    ///
    /// This writes `CSI ? 2048 h` now and records the change so the session re-applies it on
    /// `enter` and resets it (`CSI ? 2048 l`) on `leave`/drop/emergency. With it on, the terminal
    /// reports every size change in band as `CSI 48 ; … t`, decoded to
    /// [`ResizeEvent`](crate::ResizeEvent) — the preferred resize source where available, letting
    /// the application avoid `SIGWINCH` (R-IN-8, design 01).
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write the enable bytes.
    pub fn enable_in_band_resize(&mut self) -> terminal::Result<&mut Self> {
        self.enable_mode(
            ModeKind::InBandResize,
            &commands::terminal::enable_in_band_resize(),
            &commands::terminal::disable_in_band_resize(),
        )
    }

    /// Pushes kitty keyboard progressive-enhancement flags, without verifying what was granted.
    ///
    /// This is the narrow primitive: it writes `CSI > flags u` now and records the matching pop
    /// (`CSI < u`) so the session re-applies the push on `enter` and pops it on
    /// `leave`/drop/emergency. It does **not** query the terminal for what was actually granted —
    /// the kitty protocol is a progressive enhancement, so a terminal may enable only a subset, or
    /// (over a multiplexer, or on an old terminal) none at all.
    ///
    /// Use this when you want to drive the exchange yourself: push the flags, then read back the
    /// active set at your own timing with [`commands::terminal::query_kitty_keyboard_flags`] and
    /// the [`report`](crate::report) parsers. When you want that verify-after-push handled for
    /// you, the Tokio session's
    /// `TokioTerminalSession::request_kitty_keyboard`
    /// is the convenience layered on top of this primitive.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write the push bytes.
    pub fn push_kitty_keyboard(
        &mut self,
        flags: KittyKeyboardFlags,
    ) -> terminal::Result<&mut Self> {
        self.enable_mode(
            ModeKind::KittyKeyboard,
            &commands::terminal::push_kitty_keyboard_flags(flags),
            &commands::terminal::pop_kitty_keyboard_flags(),
        )
    }

    /// Enters the alternate screen buffer.
    ///
    /// This writes `CSI ? 1049 h` **followed by an explicit `CSI 2 J`** now and records the pair as
    /// one ledger entry's apply action, so a later `enter` replays both. The undo action is
    /// `CSI ? 1049 l` alone, written on `leave`/drop/emergency.
    ///
    /// The explicit clear after entry is deliberate, not decorative (R-OUT-3, design 01 evidence):
    /// mosh does not clear the alternate buffer on 1049 the way most terminals do, and helix works
    /// around exactly this by emitting its own clear right after entering. Without it, a host that
    /// skips the implicit clear can show stale primary-screen content (or the previous alternate-
    /// screen session's leftovers) through the new alternate buffer until the application's first
    /// frame overwrites every cell. Because leaving switches back to the primary buffer — which
    /// this session never wrote to while alternate — the undo action never needs a matching
    /// clear.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write the enter-and-clear bytes.
    pub fn enter_alternate_screen(&mut self) -> terminal::Result<&mut Self> {
        let mut apply = Vec::new();
        commands::screen::enter_alternate_screen().encode(&mut apply);
        commands::screen::clear().encode(&mut apply);

        // Apply now so the alternate screen is active for the caller's next write.
        self.device.write_all(&apply)?;
        self.track_mode(
            ModeKind::AlternateScreen,
            apply,
            &commands::screen::leave_alternate_screen(),
        );
        Ok(self)
    }

    /// Hides the cursor.
    ///
    /// This writes `CSI ? 25 l` now and records a ledger entry whose undo shows the cursor again
    /// (`CSI ? 25 h`) on `leave`/drop/emergency (FM-L3). Hiding is the tracked state: a session
    /// that hides the cursor is guaranteed to show it again on every exit path, whether or not
    /// the application calls [`TerminalSession::show_cursor`] itself.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write the hide bytes.
    pub fn hide_cursor(&mut self) -> terminal::Result<&mut Self> {
        self.enable_mode(
            ModeKind::CursorVisibility,
            &commands::cursor::hide(),
            &commands::cursor::show(),
        )
    }

    /// Shows the cursor.
    ///
    /// This writes `CSI ? 25 h` immediately. Showing is not itself a ledger-tracked mode change —
    /// the visible cursor is the safe, default state, so there is nothing to undo on leave. Calling
    /// this after [`TerminalSession::hide_cursor`] makes the cursor visible again right away; the
    /// hide entry recorded in the ledger remains (its undo is the same show bytes this method just
    /// wrote), so a later `leave` writes one more redundant, harmless show.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write the show bytes.
    pub fn show_cursor(&mut self) -> terminal::Result<&mut Self> {
        self.bytes({
            let mut bytes = Vec::new();
            commands::cursor::show().encode(&mut bytes);
            bytes
        })
    }

    /// Enables a byte-based mode: writes its enable bytes now, then
    /// [`track_mode`](Self::track_mode) records the entry and refreshes the emergency blob so
    /// its reset bytes are covered even before the next `enter`. This is the sync path — it
    /// owns the write. The Tokio driver instead writes through its own readiness path and calls
    /// `track_mode` directly, so the two never differ on *what* is recorded, only on *who
    /// writes*.
    fn enable_mode(
        &mut self,
        kind: ModeKind,
        enable: &Command,
        disable: &Command,
    ) -> terminal::Result<&mut Self> {
        let mut apply = Vec::new();
        enable.encode(&mut apply);

        // Apply now so the mode is active for the caller's next read.
        self.device.write_all(&apply)?;
        self.track_mode(kind, apply, disable);
        Ok(self)
    }

    /// Records an already-applied byte-based mode entry in the ledger and refreshes the emergency
    /// blob, **without** writing the enable bytes — the caller has already written them.
    ///
    /// [`enable_mode`](Self::enable_mode) (the sync path) writes through the device and then calls
    /// this; the Tokio driver writes the enable bytes through its own readiness path and then calls
    /// this so the ledger and emergency blob learn the entry without a second, unordered write.
    /// `apply` is the already-encoded enable bytes (replayed by a later `enter`); `disable` is
    /// encoded here for the undo action.
    fn track_mode(&mut self, kind: ModeKind, apply: Vec<u8>, disable: &Command) {
        let mut undo = Vec::new();
        disable.encode(&mut undo);
        self.ledger.record(
            kind,
            StateAction::WriteBytes(apply),
            StateAction::WriteBytes(undo),
        );

        // Refresh the emergency blob so a panic between now and the next `enter` still resets this
        // mode from the ledger's byte-based undo.
        #[cfg(any(unix, windows))]
        if let Some(restore) = &self.restore {
            restore.publish_blob(&self.ledger.protocol_undo_bytes());
        }
    }

    /// Records an already-written mouse enable in the ledger (Tokio driver path).
    ///
    /// The driver has written `CSI ? N h CSI ? 1006 h` through its own readiness path; this records
    /// the ledger entry and refreshes the emergency blob without a second write, keeping the
    /// private [`ModeKind`] out of the driver.
    #[cfg_attr(
        not(all(feature = "tokio", any(unix, windows))),
        expect(
            dead_code,
            reason = "tokio async-driver helper (unix or windows); absent from other builds"
        )
    )]
    pub(crate) fn record_mouse_enabled(&mut self, mode: MouseMode) {
        let mut apply = Vec::new();
        commands::terminal::enable_mouse(mode).encode(&mut apply);
        self.track_mode(
            ModeKind::Mouse,
            apply,
            &commands::terminal::disable_mouse(mode),
        );
    }

    /// Records an already-written focus-events enable in the ledger (Tokio driver path).
    #[cfg_attr(
        not(all(feature = "tokio", any(unix, windows))),
        expect(
            dead_code,
            reason = "tokio async-driver helper (unix or windows); absent from other builds"
        )
    )]
    pub(crate) fn record_focus_events_enabled(&mut self) {
        let mut apply = Vec::new();
        commands::terminal::enable_focus_events().encode(&mut apply);
        self.track_mode(
            ModeKind::Focus,
            apply,
            &commands::terminal::disable_focus_events(),
        );
    }

    /// Records an already-written bracketed-paste enable in the ledger (Tokio driver path).
    #[cfg_attr(
        not(all(feature = "tokio", any(unix, windows))),
        expect(
            dead_code,
            reason = "tokio async-driver helper (unix or windows); absent from other builds"
        )
    )]
    pub(crate) fn record_bracketed_paste_enabled(&mut self) {
        let mut apply = Vec::new();
        commands::terminal::enable_bracketed_paste().encode(&mut apply);
        self.track_mode(
            ModeKind::BracketedPaste,
            apply,
            &commands::terminal::disable_bracketed_paste(),
        );
    }

    /// Records an already-written in-band resize enable in the ledger (Tokio driver path).
    #[cfg_attr(
        not(all(feature = "tokio", any(unix, windows))),
        expect(
            dead_code,
            reason = "tokio async-driver helper (unix or windows); absent from other builds"
        )
    )]
    pub(crate) fn record_in_band_resize_enabled(&mut self) {
        let mut apply = Vec::new();
        commands::terminal::enable_in_band_resize().encode(&mut apply);
        self.track_mode(
            ModeKind::InBandResize,
            apply,
            &commands::terminal::disable_in_band_resize(),
        );
    }

    /// Records an already-written alternate-screen enter-and-clear in the ledger (Tokio driver
    /// path).
    ///
    /// The driver has written `CSI ? 1049 h` followed by the explicit `CSI 2 J` clear (R-OUT-3)
    /// through its own readiness path; this records the ledger entry — apply is the enter-and-clear
    /// pair, undo is `CSI ? 1049 l` — and refreshes the emergency blob without a second write.
    #[cfg_attr(
        not(all(feature = "tokio", any(unix, windows))),
        expect(
            dead_code,
            reason = "tokio async-driver helper (unix or windows); absent from other builds"
        )
    )]
    pub(crate) fn record_alternate_screen_entered(&mut self) {
        let mut apply = Vec::new();
        commands::screen::enter_alternate_screen().encode(&mut apply);
        commands::screen::clear().encode(&mut apply);
        self.track_mode(
            ModeKind::AlternateScreen,
            apply,
            &commands::screen::leave_alternate_screen(),
        );
    }

    /// Records an already-written cursor-hide in the ledger (Tokio driver path).
    ///
    /// The driver has written `CSI ? 25 l` through its own readiness path; this records the ledger
    /// entry — apply hides, undo shows (FM-L3) — and refreshes the emergency blob without a second
    /// write.
    #[cfg_attr(
        not(all(feature = "tokio", any(unix, windows))),
        expect(
            dead_code,
            reason = "tokio async-driver helper (unix or windows); absent from other builds"
        )
    )]
    pub(crate) fn record_cursor_hidden(&mut self) {
        let mut apply = Vec::new();
        commands::cursor::hide().encode(&mut apply);
        self.track_mode(ModeKind::CursorVisibility, apply, &commands::cursor::show());
    }

    /// Undoes the mode ledger in reverse enablement order, reporting the first error.
    fn restore_state(&mut self) -> terminal::Result<()> {
        let mut first_error = None;
        for action in self.ledger.undo_actions() {
            let result = match action {
                StateAction::WriteBytes(bytes) => self.device.write_all(bytes),
                StateAction::SetMode(mode) => self.device.set_mode(*mode),
            };
            if let Err(error) = result
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }

        if let Err(error) = self.device.flush()
            && first_error.is_none()
        {
            first_error = Some(error);
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// The number of bytes one query read pulls from the terminal at a time.
///
/// A single reply is tiny (`CSI row ; column R` is under a dozen bytes), so this only needs to be
/// large enough that a reply plus a little interleaved typeahead arrive in one read.
#[cfg(unix)]
const QUERY_READ_BUFFER_LEN: usize = 1024;

/// The synchronous, no-Tokio query driver over the sans-io correlator (design 04, review-02 §2).
///
/// These helpers are the second consumer of the correlator the async session drives: they register
/// an `Expectation`, write the request, then poll the device fd → read → decode → feed the
/// correlator until the reply completes, all without an async runtime. The narrow-primitive pieces
/// stay reachable — [`command`](Self::command), [`read_input`](Self::read_input), and
/// [`as_fd`](Self::as_fd) are unchanged — so this typed helper is a convenience over them, not a
/// replacement.
#[cfg(unix)]
impl<D: TerminalDevice> TerminalSession<D> {
    /// Requests and reads the current terminal cursor position, blocking without an async runtime.
    ///
    /// This is the synchronous mirror of the Tokio session's
    /// `TokioTerminalSession::request_cursor_position`: it writes
    /// the Device Status Report request `CSI 6 n`, flushes, and drives the sans-io correlator with
    /// a hand-rolled poll/read/decode loop until a `CSI row ; column R` cursor-position report
    /// completes the query. It uses no Tokio and no signal handler — only the runtime-neutral
    /// [`as_fd`](Self::as_fd) readiness seam and `rustix::event::poll`.
    ///
    /// `timeout` bounds the whole request/response operation. A terminal that never answers is the
    /// FM-C4 **unknown** case, not an error: on elapse this resolves the expectation as
    /// `Resolution::NoReply`, returns `Ok(None)`, and never hangs. A reply that arrives after the
    /// budget is never claimed — the expectation is already removed, so the late reply passes
    /// through the correlator as ordinary input (design 03 rule 4).
    ///
    /// # Typeahead safety
    ///
    /// Input that arrives while the query waits but is **not** the reply — a keystroke the user
    /// typed ahead — is never consumed or misattributed. Those bytes are buffered on the session
    /// and returned by the next [`read_input`](Self::read_input) in arrival order (FM-Q1), so
    /// the query leaves the ordinary input stream intact.
    ///
    /// The raw pieces stay available: this helper does not hide [`command`](Self::command),
    /// [`read_input`](Self::read_input), or [`as_fd`](Self::as_fd); a caller that wants to build
    /// its own loop still can (see the `oneshot_background.rs` example).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use qwertty::TerminalSession;
    ///
    /// # fn main() -> qwertty::Result<()> {
    /// let mut session = TerminalSession::open()?;
    /// match session.request_cursor_position(Duration::from_millis(150))? {
    ///     Some(report) => println!("cursor at row {}, column {}", report.row(), report.column()),
    ///     None => println!("no reply within the budget (unknown, not an error)"),
    /// }
    /// session.leave()
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when writing, flushing, or reading terminal I/O fails, or when the device
    /// has no pollable descriptor to wait on (a headless device, reported as
    /// [`terminal::Error::Unsupported`]). A timeout is **not** an error — it is `Ok(None)`.
    pub fn request_cursor_position(
        &mut self,
        timeout: Duration,
    ) -> terminal::Result<Option<CursorPositionReport>> {
        let reply = self.run_query(
            Expectation::CursorPosition,
            &commands::cursor::request_position(),
            timeout,
        )?;
        match reply {
            Some(Reply::CursorPosition(report)) => Ok(Some(report)),
            Some(other) => Err(unexpected_reply(&other)),
            None => Ok(None),
        }
    }

    /// Requests and reads terminal status, blocking without an async runtime.
    ///
    /// This is the synchronous mirror of the Tokio session's
    /// `TokioTerminalSession::request_terminal_status`: it writes
    /// the Device Status Report request `CSI 5 n`, flushes, and drives the correlator with the same
    /// poll/read/decode loop until a `CSI 0 n` ready or `CSI 3 n` malfunction report completes the
    /// query. It composes over the identical machinery as
    /// [`request_cursor_position`](Self::request_cursor_position); only the expectation, request,
    /// and reply type differ.
    ///
    /// `timeout` bounds the whole operation. A silent terminal is the FM-C4 **unknown** case:
    /// `Ok(None)`, never an error and never a hang. Typeahead read while waiting survives for the
    /// next [`read_input`](Self::read_input) (FM-Q1).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use qwertty::TerminalSession;
    /// use qwertty::report::TerminalStatus;
    ///
    /// # fn main() -> qwertty::Result<()> {
    /// let mut session = TerminalSession::open()?;
    /// if let Some(report) = session.request_terminal_status(Duration::from_millis(150))? {
    ///     assert_eq!(report.status(), TerminalStatus::Ready);
    /// }
    /// session.leave()
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when writing, flushing, or reading terminal I/O fails, or when the device
    /// has no pollable descriptor to wait on. A timeout is `Ok(None)`, not an error.
    pub fn request_terminal_status(
        &mut self,
        timeout: Duration,
    ) -> terminal::Result<Option<TerminalStatusReport>> {
        let reply = self.run_query(
            Expectation::TerminalStatus,
            &commands::terminal::request_status(),
            timeout,
        )?;
        match reply {
            Some(Reply::TerminalStatus(report)) => Ok(Some(report)),
            Some(other) => Err(unexpected_reply(&other)),
            None => Ok(None),
        }
    }

    /// Probes terminal capabilities with the DA1-fenced bundle, blocking without an async runtime.
    ///
    /// This is the synchronous mirror of `TokioTerminalSession::probe_capabilities` (design 03):
    /// one write of XTVERSION, the kitty keyboard flags query, OSC 10/11, and the DEC private
    /// mode queries (synchronized output 2026, grapheme clustering 2027, in-band resize 2048,
    /// bracketed paste 2004), with Primary Device Attributes (DA1) last as the fence — a terminal
    /// that answers DA1 has finished answering everything it is going to answer, so the fence
    /// firing ends the probe without waiting out the full timeout on a terminal that replied fast.
    /// A terminal that never answers DA1 is bounded by `timeout` instead (FM-C6: one timeout for
    /// the whole bundle, not one per query). It shares its bundle contents, reply-to-field
    /// mapping, and env-inferred fallbacks (hyperlinks, truecolor, identity) with the Tokio
    /// driver via `crate::caps` — the two can never drift silently out of sync with each other.
    ///
    /// Every unanswered field is `None`, meaning *unknown*, never unsupported (FM-C4): a DECRQM
    /// "mode not recognized" answer is `None` too, and a fully silent terminal yields an
    /// all-unknown [`Capabilities`] rather than an error. Input that is not a bundle reply —
    /// typeahead, keystrokes, unrelated reports — is preserved byte-exact for the next
    /// [`read_input`](Self::read_input) in arrival order (FM-Q1), the same guarantee
    /// [`request_cursor_position`](Self::request_cursor_position) makes for a single query.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use qwertty::TerminalSession;
    ///
    /// # fn main() -> qwertty::Result<()> {
    /// let mut session = TerminalSession::open()?;
    /// let capabilities = session.probe_capabilities(Duration::from_millis(150))?;
    /// match capabilities.background_color.value() {
    ///     Some(rgb) => println!("background color: {rgb:?}"),
    ///     None => println!("unknown (unanswered or not yet supported)"),
    /// }
    /// session.leave()
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when writing, flushing, or reading terminal I/O fails, or when the device
    /// has no pollable descriptor to wait on. A silent or partially silent terminal is not an
    /// error: it is `Ok(Capabilities)` with the unanswered fields `None`.
    pub fn probe_capabilities(&mut self, timeout: Duration) -> terminal::Result<Capabilities> {
        // Step 1: sweep a leftover single-query expectation (defensive; mirrors `run_query`), then
        // register the bundle. DA1 is registered last so it is the fence.
        self.sweep_active_query();
        let bundle = ProbeBundle::register(&mut self.query.correlator);
        let ids = bundle.ids();

        // Step 2: write the whole bundle in one buffer, DA1 last, then flush. Shared with the
        // Tokio driver (`caps::probe_bundle_commands`) so the two request sets can never diverge.
        self.bytes(crate::caps::probe_bundle_commands().into_bytes())?
            .flush()?;

        // The env-inferred findings and the env-only identity fallback never come from a terminal
        // reply (FM-C12: no query exists for hyperlinks/truecolor), so they are populated once, up
        // front, from the environment alone. An XTVERSION reply later overwrites `identity` with
        // the wire-informed cross-check via `store_bundle_reply`.
        let mut capabilities = Capabilities {
            hyperlinks: crate::caps::infer_hyperlinks(crate::caps::std_env_source),
            truecolor: crate::caps::infer_truecolor(crate::caps::std_env_source),
            identity: crate::caps::identity_from_env(None, crate::caps::std_env_source),
            ..Capabilities::default()
        };

        // Step 3: deadline loop over the whole probe, one total timeout (FM-C6).
        #[expect(
            clippy::disallowed_methods,
            reason = "the sync query driver owns its deadline outside the sans-io core (design 04)"
        )]
        let deadline = Instant::now() + timeout;
        loop {
            #[expect(
                clippy::disallowed_methods,
                reason = "the sync query driver owns its deadline outside the sans-io core (design \
                          04)"
            )]
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bundle.resolve_all(&mut self.query.correlator, Resolution::NoReply);
                return Ok(capabilities);
            }

            let Some(fd) = self.device.as_fd() else {
                bundle.resolve_all(&mut self.query.correlator, Resolution::Cancelled);
                return Err(terminal::Error::unsupported(
                    "synchronous terminal query",
                    "device without a fd",
                ));
            };
            if !poll_readable(fd, remaining)? {
                bundle.resolve_all(&mut self.query.correlator, Resolution::NoReply);
                return Ok(capabilities);
            }

            // Readable: one OS read, decoded and matched against every outstanding bundle
            // expectation (several can complete within one read). Feed the whole read before
            // acting on a fence completion (FM-Q7): a DA1 reply and a slower reply arriving in
            // the same read must both land before the fence ends the probe.
            let replies = match self.read_and_match(&ids) {
                Ok(replies) => replies,
                Err(err) => {
                    // EOF ends the probe with what was gathered — unknown, not an error, the same
                    // FM-C4 treatment a timeout gets. `read_and_match` already resolved every id
                    // as EOF; a non-EOF read error is still fatal and propagates.
                    return if is_eof_error(&err) {
                        Ok(capabilities)
                    } else {
                        Err(err)
                    };
                }
            };
            let mut fenced = false;
            for (id, reply) in replies {
                store_bundle_reply(&bundle, id, reply, &mut capabilities);
                if Some(id) == bundle.fence() {
                    fenced = true;
                }
            }
            if fenced {
                bundle.resolve_all(&mut self.query.correlator, Resolution::NoReply);
                return Ok(capabilities);
            }
        }
    }

    /// Runs one typed query end to end against the correlator, synchronously.
    ///
    /// The steps mirror the Tokio driver's `run_query`, minus the reactor and cancellation sweep
    /// (this driver holds `&mut self` for the whole blocking call, so no query can be abandoned
    /// mid-flight):
    ///
    /// 1. **Sweep** any expectation a previous query left behind (defensive; a completed sync query
    ///    always clears its own), then **register** the expectation and record its id.
    /// 2. **Write** the request bytes and flush.
    /// 3. **Deadline loop.** Poll the device fd with the remaining budget; on readiness read one OS
    ///    read, decode it into events, and feed them through the correlator. The reply completes
    ///    the query. A read that carries no reply is buffered raw as typeahead. On timeout the
    ///    expectation resolves `Resolution::NoReply` and the query returns `Ok(None)`.
    fn run_query(
        &mut self,
        expectation: Expectation,
        request: &Command,
        timeout: Duration,
    ) -> terminal::Result<Option<Reply>> {
        // Step 1: sweep a leftover expectation (defensive), then register.
        self.sweep_active_query();
        let id = self
            .query
            .correlator
            .register(expectation)
            .expect("single in-flight sync query never conflicts with a swept expectation");
        self.query.active_query = Some(id);

        // Step 2: write the request and flush.
        self.command(request)?.flush()?;

        // Step 3: deadline loop. The budget covers the whole request/response operation.
        //
        // This driver *is* the clock the sans-io core deliberately lacks (design 03/04): the
        // correlator holds no clock, so the crate-wide `Instant::now` ban keeps time out of the
        // core, and a real driver like this one owns its deadline outside it — exactly the escape
        // the ban's own comment sanctions.
        #[expect(
            clippy::disallowed_methods,
            reason = "the sync query driver owns its deadline outside the sans-io core (design 04)"
        )]
        let deadline = Instant::now() + timeout;
        loop {
            #[expect(
                clippy::disallowed_methods,
                reason = "the sync query driver owns its deadline outside the sans-io core (design \
                          04)"
            )]
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(self.resolve_query_timeout(id));
            }

            // Wait for the fd to become readable within the remaining budget. A device with no
            // pollable descriptor cannot be waited on synchronously; report it as unsupported
            // rather than busy-looping.
            let Some(fd) = self.device.as_fd() else {
                self.query.correlator.resolve(id, Resolution::Cancelled);
                self.query.active_query = None;
                return Err(terminal::Error::unsupported(
                    "synchronous terminal query",
                    "device without a fd",
                ));
            };
            if !poll_readable(fd, remaining)? {
                // poll timed out: the budget elapsed with no readiness.
                return Ok(self.resolve_query_timeout(id));
            }

            // Readable: one OS read, decoded and matched. A read that does not complete the query
            // is buffered raw for a later `read_input` (typeahead survival, FM-Q1).
            if let Some((_, reply)) = self.read_and_match(&[id])?.into_iter().next() {
                self.query.active_query = None;
                return Ok(Some(reply));
            }
        }
    }

    /// Resolves the query's expectation as `Resolution::NoReply` on a timeout and clears it.
    ///
    /// Returning `None` here is the FM-C4 *unknown* outcome the public helpers surface as
    /// `Ok(None)`: the terminal did not answer within the budget, which is unknown, not an error.
    /// Resolving the expectation removes it, so a reply that arrives later passes through the
    /// correlator as ordinary input (design 03 rule 4) instead of completing a stale query.
    fn resolve_query_timeout(&mut self, id: ExpectationId) -> Option<Reply> {
        self.query.correlator.resolve(id, Resolution::NoReply);
        self.query.active_query = None;
        None
    }

    /// Performs one OS read, decodes it, and feeds the events through the correlator.
    ///
    /// Returns every `(id, reply)` among `ids` that completes in this read, in completion order —
    /// usually zero or one for a single query, but a probe bundle's several outstanding
    /// expectations can complete together within one read (their replies arriving close enough to
    /// land in the same OS read), so every completion is collected rather than stopping at the
    /// first. Typeahead is kept byte-accurate by decoding the read a byte at a time and tracking
    /// the raw byte span each completed event occupied: a non-reply event's span is buffered as
    /// typeahead for a later [`read_input`](Self::read_input), while a reply's own span is dropped
    /// (those bytes were the answer, consumed by the correlator). This holds even when a reply and
    /// unrelated input arrive in the **same** read — nothing the query saw is lost or misattributed
    /// (FM-Q1).
    ///
    /// Feeding byte by byte also settles the syntax layer's parked trailing text: a lone
    /// keystroke's text run is parked for split-equivalence until the next byte, so the loop
    /// flushes the decoder once at the end of the read (the drained-OS-buffer boundary) to
    /// release a complete parked run, attributing it to the bytes still pending in the decoder.
    ///
    /// # Errors
    ///
    /// Returns an error when the device read fails or the terminal closed (a zero-length read).
    /// On EOF every id in `ids` is resolved `Resolution::Eof` before the error is returned, so a
    /// bundle's whole expectation set is cleared, not just the first.
    fn read_and_match(
        &mut self,
        ids: &[ExpectationId],
    ) -> terminal::Result<Vec<(ExpectationId, Reply)>> {
        let mut buffer = [0u8; QUERY_READ_BUFFER_LEN];
        let len = self.device.read(&mut buffer)?;
        if len == 0 {
            // The terminal closed before answering. Resolve every outstanding expectation as EOF
            // and surface the close as a read error, matching the device layer's own
            // end-of-input contract.
            for &id in ids {
                self.query.correlator.resolve(id, Resolution::Eof);
            }
            self.query.active_query = None;
            return Err(terminal::Error::read_terminal(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "terminal input closed before the query reply arrived",
            )));
        }
        let chunk = &buffer[..len];

        // `unattributed` is the raw byte run fed to the decoder since the last event completed. It
        // spans reads: the decoder holds bytes across read boundaries (a parked text run, a
        // half-finished sequence), so its own carry from a prior read is prepended before this
        // read's bytes. When an event completes, that whole run is the event's bytes; it is
        // buffered as typeahead unless the event is a reply.
        let mut replies = Vec::new();
        let mut unattributed = std::mem::take(&mut self.query.decoder_carry);
        for &byte in chunk {
            unattributed.push(byte);
            let events = self.query.decoder.feed(&[byte]);
            if events.is_empty() {
                continue;
            }
            // The decoder may have completed an event *and* started a new pending sequence with the
            // same byte (an ESC that flushes prior parked text then opens a fresh escape). Any
            // bytes still pending in the decoder belong to that next, not-yet-complete
            // event — so the span that just completed is `unattributed` minus its
            // pending tail.
            let pending_len = self.query.decoder.pending_bytes().len();
            let span_len = unattributed.len().saturating_sub(pending_len);
            let span: Vec<u8> = unattributed.drain(..span_len).collect();
            self.attribute_span(ids, span, &events, &mut replies);
        }

        // Drained-OS-buffer boundary: release a complete parked trailing text run so a lone
        // keystroke is not held unseen until the next byte (the same flush the Tokio read loop
        // does).
        if self.query.decoder.has_settled_text() {
            let events = self.query.decoder.finish();
            let pending_len = self.query.decoder.pending_bytes().len();
            let span_len = unattributed.len().saturating_sub(pending_len);
            let span: Vec<u8> = unattributed.drain(..span_len).collect();
            self.attribute_span(ids, span, &events, &mut replies);
        }

        // Whatever is still unattributed is a partial sequence the decoder is holding for the next
        // read; carry its raw bytes so its eventual completion is attributed correctly.
        self.query.decoder_carry = unattributed;
        Ok(replies)
    }

    /// Feeds one span's decoded events through the correlator and attributes the span's raw bytes.
    ///
    /// The span is the exact raw bytes that produced `events`. Every completion among `ids` found
    /// in `events` is appended to `replies`; the span is buffered as typeahead only when none of
    /// its events completed one of `ids` — a span that completes even one tracked expectation is
    /// fully consumed as an answer, never partially replayed as input.
    fn attribute_span(
        &mut self,
        ids: &[ExpectationId],
        span: Vec<u8>,
        events: &[Event],
        replies: &mut Vec<(ExpectationId, Reply)>,
    ) {
        let mut span_is_reply = false;
        for event in events {
            match self.query.correlator.feed(event.clone()) {
                Feed::Completed { id: completed, .. } if ids.contains(&completed) => {
                    if let Some(reply) = self.query.correlator.take_reply(completed) {
                        replies.push((completed, reply));
                    }
                    span_is_reply = true;
                }
                // A stray completion outside `ids` has no waiter and is dropped. Passthroughs are
                // unrelated input.
                Feed::Completed { .. } | Feed::Passthrough(_) => {}
            }
        }
        if !span_is_reply {
            self.query.typeahead.extend(span);
        }
    }

    /// Sweeps a leftover query expectation as cancelled.
    ///
    /// A completed synchronous query always clears its own `active_query`, so this is defensive: if
    /// a prior query somehow left one registered, resolving it `Resolution::Cancelled` removes it
    /// before a new one registers, so a stale reply can never misroute the new query (design 03
    /// rule 4). Synchronous and idempotent.
    fn sweep_active_query(&mut self) {
        if let Some(id) = self.query.active_query.take() {
            self.query.correlator.resolve(id, Resolution::Cancelled);
        }
    }
}

/// Waits for `fd` to become readable within `budget` using `rustix::event::poll`.
///
/// Returns `true` when the descriptor is readable and `false` when the budget elapsed first. This
/// is the runtime-neutral wait the synchronous query loop needs: no reactor, no signal handler,
/// just `poll(2)` on the session's own fd (the same seam [`TerminalSession::as_fd`] exposes).
///
/// # Errors
///
/// Returns a read error when `poll(2)` itself fails.
#[cfg(unix)]
fn poll_readable(fd: std::os::fd::BorrowedFd<'_>, budget: Duration) -> terminal::Result<bool> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};

    // `Timespec` fields are signed; the budget is non-negative, so the whole seconds saturate into
    // `i64` and the sub-second nanoseconds widen from `u32` losslessly.
    let timeout = Timespec {
        tv_sec: i64::try_from(budget.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: budget.subsec_nanos().into(),
    };
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    // `poll` returns the number of ready descriptors; 0 means the timeout elapsed. We poll exactly
    // one fd, so any positive count is that fd becoming readable.
    let ready = poll(&mut fds, Some(&timeout))
        .map_err(|errno| terminal::Error::read_terminal(std::io::Error::from(errno)))?;
    Ok(ready > 0)
}

/// Returns whether `error` is the "terminal closed before answering" case
/// [`read_and_match`](TerminalSession::read_and_match) raises on a zero-length read.
///
/// [`probe_capabilities`](TerminalSession::probe_capabilities) treats this the same as a timeout
/// (FM-C4: unknown, not an error) — every other read error still propagates.
#[cfg(unix)]
fn is_eof_error(error: &terminal::Error) -> bool {
    matches!(
        error,
        terminal::Error::ReadTerminal { source } if source.kind() == std::io::ErrorKind::UnexpectedEof
    )
}

/// Builds the error for the impossible "wrong reply type completed a typed query" case.
///
/// The correlator only completes an `Expectation::CursorPosition` with a `Reply::CursorPosition`
/// and an `Expectation::TerminalStatus` with a `Reply::TerminalStatus`, so this never fires; it
/// exists so the typed helpers stay total without an `unreachable!`.
#[cfg(unix)]
fn unexpected_reply(_reply: &Reply) -> terminal::Error {
    terminal::Error::read_terminal(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "query completed with an unexpected reply type",
    ))
}

impl<D: TerminalDevice> Drop for TerminalSession<D> {
    fn drop(&mut self) {
        _ = self.leave();
    }
}

/// Returns whether a reported size is usable rather than a known degenerate value.
fn size_is_usable(size: TerminalSize) -> bool {
    let columns = size.columns();
    let rows = size.rows();
    columns != 0 && rows != 0 && columns != u16::MAX && rows != u16::MAX
}

/// Reads a terminal size from the `COLUMNS` and `LINES` environment variables.
fn environment_size() -> Option<TerminalSize> {
    let columns = std::env::var("COLUMNS").ok()?.parse().ok()?;
    let rows = std::env::var("LINES").ok()?.parse().ok()?;
    let size = TerminalSize::new(columns, rows);
    size_is_usable(size).then_some(size)
}

/// Opens the best-effort device for the emergency restore path.
///
/// The emergency path gets its own file description so its non-blocking flag never affects the
/// session's reads. When the terminal path cannot be reopened, a duplicate of the session device
/// is the fallback; its writes may block, bounded by the emergency retry policy.
#[cfg(unix)]
fn emergency_device(terminal: &Terminal) -> terminal::Result<std::fs::File> {
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

    let reopened = std::fs::OpenOptions::new()
        .write(true)
        .open(terminal.path())
        .and_then(|device| {
            let flags = fcntl_getfl(&device)?;
            fcntl_setfl(&device, flags | OFlags::NONBLOCK)?;
            Ok(device)
        });

    match reopened {
        Ok(device) => Ok(device),
        Err(_) => terminal.try_clone_device(),
    }
}

/// Duplicates the console handles and captures the modes for the emergency restore path (Windows).
///
/// The panic-safe [`RestoreHandle`] needs its own handles so it can write the teardown blob and
/// reset the console modes without borrowing the session's device: an output dup to `WriteFile`
/// the blob and an input dup whose mode it resets. The captured input mode, output mode, and output
/// codepage are the same values the device's cooked-mode restore puts back — the emergency path
/// restores exactly what was live at open, never synthesized defaults.
///
/// # Errors
///
/// Returns an error when either console handle cannot be duplicated.
#[cfg(windows)]
fn emergency_console(
    terminal: &Terminal,
) -> terminal::Result<(
    std::os::windows::io::OwnedHandle,
    std::os::windows::io::OwnedHandle,
    ConsoleModeRestore,
)> {
    let output = terminal.try_clone_output_handle()?;
    let input = terminal.try_clone_input_handle()?;
    let modes = ConsoleModeRestore::new(
        terminal.original_input_mode(),
        terminal.original_output_mode(),
        terminal.original_output_codepage(),
    );
    Ok((output, input, modes))
}

#[cfg(all(test, unix))]
mod tests {
    use crate::TerminalSession;
    use crate::commands::osc::{self, ClipboardSelection};
    use crate::policy::{Policy, PolicyGate};
    use crate::terminal::{Error, FakeDevice};

    #[test]
    fn new_session_starts_restricted() {
        let (device, _terminal) = FakeDevice::open().expect("open fake device");
        let session = TerminalSession::from_device(device).expect("start fake session");

        assert_eq!(session.policy(), Policy::restricted());
    }

    #[test]
    fn set_clipboard_denied_writes_zero_bytes() {
        let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
        let mut session = TerminalSession::from_device(device).expect("start fake session");

        // A hand-built policy with clipboard write off must deny the write.
        session.set_policy(Policy {
            clipboard_write: false,
            ..Policy::restricted()
        });

        let result = session.set_clipboard(ClipboardSelection::Clipboard, b"secret");

        assert!(
            matches!(
                result,
                Err(Error::PolicyDenied {
                    gate: PolicyGate::ClipboardWrite
                })
            ),
            "expected PolicyDenied, got {result:?}",
        );

        // Raw mode is a device mode, not bytes, so a denied write leaves the output empty.
        assert_eq!(
            fake_terminal.output().expect("output"),
            Vec::<u8>::new(),
            "a denied clipboard write must not emit any bytes",
        );
    }

    #[test]
    fn set_clipboard_allowed_writes_exact_command_bytes_and_chains() {
        let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
        // Default (restricted) session allows clipboard write.
        let mut session = TerminalSession::from_device(device).expect("start fake session");

        session
            .set_clipboard(ClipboardSelection::Clipboard, b"Hello")
            .expect("clipboard write allowed")
            .flush()
            .expect("flush");

        // The exact bytes are the command builder's own encoding — the session gate adds no
        // framing.
        let mut expected = Vec::new();
        osc::set_clipboard(ClipboardSelection::Clipboard, b"Hello").encode(&mut expected);
        assert_eq!(fake_terminal.output().expect("output"), expected);
    }

    #[test]
    fn push_kitty_keyboard_writes_push_now_and_pops_on_leave() {
        use crate::KittyKeyboardFlags;
        use crate::commands::terminal;

        let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
        let mut session = TerminalSession::from_device(device).expect("start fake session");

        let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES;
        session
            .push_kitty_keyboard(flags)
            .expect("push kitty flags")
            .flush()
            .expect("flush");

        // The set-only primitive writes exactly the push bytes — no query round-trip, no framing.
        let mut expected_push = Vec::new();
        terminal::push_kitty_keyboard_flags(flags).encode(&mut expected_push);
        assert_eq!(fake_terminal.output().expect("output"), expected_push);

        // Teardown pops the pushed level the ledger recorded.
        session.leave().expect("leave");
        let mut expected_pop = Vec::new();
        terminal::pop_kitty_keyboard_flags().encode(&mut expected_pop);
        assert_eq!(
            fake_terminal.output().expect("output after leave"),
            expected_pop
        );
    }

    #[test]
    fn set_clipboard_uses_policy_after_widening() {
        let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
        let mut session = TerminalSession::from_device(device)
            .expect("start fake session")
            .with_policy(Policy {
                clipboard_write: false,
                ..Policy::restricted()
            });

        assert!(
            session
                .set_clipboard(ClipboardSelection::Primary, b"x")
                .is_err()
        );
        assert_eq!(fake_terminal.output().expect("output"), Vec::<u8>::new());

        // Widening the policy lets the same write through.
        session.set_policy(Policy::trusted());
        session
            .set_clipboard(ClipboardSelection::Primary, b"x")
            .expect("write allowed after widening")
            .flush()
            .expect("flush");

        let mut expected = Vec::new();
        osc::set_clipboard(ClipboardSelection::Primary, b"x").encode(&mut expected);
        assert_eq!(fake_terminal.output().expect("output"), expected);
    }

    #[test]
    fn policy_denied_error_display_names_the_gate() {
        let error = Error::PolicyDenied {
            gate: PolicyGate::ClipboardWrite,
        };
        let message = error.to_string();
        assert!(!message.is_empty());
        assert!(
            message.contains("clipboard write"),
            "error message should name the gate: {message}",
        );
    }

    // --- Synchronous query driver (review-02 §2) -------------------------------------------------
    //
    // These drive the sans-io correlator with no Tokio, over a headless `FakeDevice` socket pair.
    // The fake terminal's reply is fed before the blocking query polls, so the pre-queued bytes are
    // already readable when the query waits — no thread, no pseudoterminal.

    use std::time::Duration;

    use crate::ProtocolPosition;
    use crate::report::TerminalStatus;

    #[test]
    fn sync_cursor_query_round_trips_over_a_fake_device() {
        // The whole point of the sync driver: a headless fake terminal drives the real correlator
        // with a hand-rolled poll/read/decode loop (R-TST-1). Write `CSI 6 n`, get row 12 / col 34.
        let (device, mut terminal) = FakeDevice::open().expect("open fake device");
        let mut session = TerminalSession::from_device(device).expect("start fake session");

        // Queue the reply so it is readable the moment the query polls.
        terminal
            .feed_input(b"\x1b[12;34R")
            .expect("feed cursor position report");

        let report = session
            .request_cursor_position(Duration::from_secs(1))
            .expect("cursor query succeeds")
            .expect("a reply arrives");

        assert_eq!(report.position(), ProtocolPosition::new(12, 34));
        assert_eq!(report.row(), 12);
        assert_eq!(report.column(), 34);

        // The request the driver wrote is exactly the DSR cursor-position probe.
        assert_eq!(terminal.output().expect("request bytes"), b"\x1b[6n");
    }

    #[test]
    fn sync_terminal_status_query_round_trips_over_a_fake_device() {
        // The second helper composes over the identical machinery: `CSI 5 n` in, `CSI 0 n` reply.
        let (device, mut terminal) = FakeDevice::open().expect("open fake device");
        let mut session = TerminalSession::from_device(device).expect("start fake session");

        terminal
            .feed_input(b"\x1b[0n")
            .expect("feed terminal status report");

        let report = session
            .request_terminal_status(Duration::from_secs(1))
            .expect("terminal status query succeeds")
            .expect("a reply arrives");

        assert_eq!(report.status(), TerminalStatus::Ready);
        assert_eq!(terminal.output().expect("request bytes"), b"\x1b[5n");
    }

    #[test]
    fn sync_cursor_query_times_out_to_none_without_hanging() {
        // No reply is ever fed: the query must return Ok(None) once the short budget elapses — the
        // FM-C4 unknown case, not an error and not a hang.
        let (device, mut terminal) = FakeDevice::open().expect("open fake device");
        let mut session = TerminalSession::from_device(device).expect("start fake session");

        let outcome = session
            .request_cursor_position(Duration::from_millis(50))
            .expect("a silent terminal is not an error");

        assert!(outcome.is_none(), "no reply must resolve to None");
        // The request still went out even though nothing answered it.
        assert_eq!(terminal.output().expect("request bytes"), b"\x1b[6n");
    }

    #[test]
    fn sync_query_preserves_typeahead_for_a_later_read() {
        // A keystroke the user typed ahead arrives before the reply. It must NOT be swallowed by
        // the query: after the query completes, `read_input` still delivers that keystroke
        // as ordinary input, in arrival order (FM-Q1).
        let (device, mut terminal) = FakeDevice::open().expect("open fake device");
        let mut session = TerminalSession::from_device(device).expect("start fake session");

        // The typeahead 'x' lands first (its own read), then the cursor-position reply.
        terminal.feed_input(b"x").expect("feed typeahead keystroke");
        // Let the typeahead settle as its own OS read before the reply, so the driver buffers it as
        // typeahead and matches the reply in a separate read.
        std::thread::sleep(Duration::from_millis(10));
        terminal
            .feed_input(b"\x1b[7;9R")
            .expect("feed cursor position report");

        let report = session
            .request_cursor_position(Duration::from_secs(1))
            .expect("cursor query succeeds")
            .expect("a reply arrives past the typeahead");
        assert_eq!(report.position(), ProtocolPosition::new(7, 9));

        // The unrelated keystroke survived: a later read still sees it, and nothing else.
        let mut buffer = [0u8; 16];
        let input = session
            .read_input(&mut buffer)
            .expect("read buffered typeahead");
        assert_eq!(input.as_bytes(), b"x", "typeahead must survive the query");
    }

    #[test]
    fn sync_query_separates_typeahead_from_a_reply_in_the_same_read() {
        // The harder case: typeahead and the reply arrive coalesced in one OS read. Byte-accurate
        // attribution must peel the reply's bytes off and keep the surrounding keystrokes as
        // typeahead — 'a' before the reply and 'b' after it (FM-Q1).
        let (device, mut terminal) = FakeDevice::open().expect("open fake device");
        let mut session = TerminalSession::from_device(device).expect("start fake session");

        terminal
            .feed_input(b"a\x1b[7;9Rb")
            .expect("feed typeahead+reply+typeahead in one burst");

        let report = session
            .request_cursor_position(Duration::from_secs(1))
            .expect("cursor query succeeds")
            .expect("the reply is peeled out of the coalesced read");
        assert_eq!(report.position(), ProtocolPosition::new(7, 9));

        // Both surrounding keystrokes survived, in order, with the reply's bytes removed.
        let mut buffer = [0u8; 16];
        let input = session
            .read_input(&mut buffer)
            .expect("read buffered typeahead");
        assert_eq!(
            input.as_bytes(),
            b"ab",
            "typeahead around the reply must survive with the reply removed",
        );
    }

    // --- Synchronous capability probe bundle (H, OQ-1 revisit) ----------------------------------
    //
    // Mirrors the Tokio driver's `tokio_probe_*` tests in tests/tokio_session.rs exactly: same
    // scripted replies, same assertions. No peer task is needed here — the fake device's queued
    // bytes are already readable the moment the blocking probe polls, same as the single-query
    // tests above.

    use crate::caps::{Evidence, Rgb};

    #[test]
    fn sync_probe_answers_a_subset_and_da1_fence_resolves_the_rest_fast() {
        let (device, mut terminal) = FakeDevice::open().expect("open fake device");
        let mut session = TerminalSession::from_device(device).expect("start fake session");

        // Answer only: mode 2026 set, OSC 11 background, and DA1 (the fence). Everything else
        // silent. A deliberately generous budget: the DA1 fence, not the clock, must end the
        // probe.
        terminal
            .feed_input(b"\x1b[?2026;1$y\x1b]11;rgb:1a1a/2b2b/3c3c\x1b\\\x1b[?1;2c")
            .expect("feed subset answers + DA1 fence");

        #[expect(
            clippy::disallowed_methods,
            reason = "the test measures wall-clock elapsed time, not a driver deadline"
        )]
        let started = std::time::Instant::now();
        let caps = session
            .probe_capabilities(Duration::from_secs(30))
            .expect("probe returns capabilities");
        let elapsed = started.elapsed();

        // The whole bundle went out in one write, DA1 last as the fence.
        let bundle = terminal.output().expect("request bytes");
        assert!(
            bundle.ends_with(b"\x1b[c"),
            "DA1 is written last as the fence, got {bundle:?}"
        );
        assert!(
            bundle.windows(4).any(|w| w == b"\x1b[>q"),
            "XTVERSION queried"
        );
        assert!(
            bundle.windows(9).any(|w| w == b"\x1b[?2026$p"),
            "mode 2026 queried"
        );

        assert_eq!(
            caps.synchronized_output.value_copied(),
            Some(true),
            "mode 2026 reported set"
        );
        assert_eq!(
            caps.synchronized_output.evidence(),
            &Evidence::Probed { via: "DECRQM 2026" },
            "mode 2026 finding is Probed"
        );
        assert_eq!(
            caps.background_color.value_copied(),
            Some(Rgb::new(0x1a, 0x2b, 0x3c)),
            "OSC 11 background parsed"
        );
        assert_eq!(
            caps.background_color.evidence(),
            &Evidence::Probed { via: "OSC 11" },
            "OSC 11 finding is Probed"
        );
        assert!(
            caps.primary_device_attributes.is_some(),
            "DA1 fence arrived"
        );
        // Every unanswered field is None (unknown, not unsupported) with Unknown evidence.
        assert_eq!(caps.grapheme_clustering.value_copied(), None);
        assert_eq!(caps.grapheme_clustering.evidence(), &Evidence::Unknown);
        assert_eq!(caps.in_band_resize.value_copied(), None);
        assert_eq!(caps.bracketed_paste.value_copied(), None);
        assert_eq!(caps.kitty_keyboard.value(), None);
        assert_eq!(caps.identity.version, None);
        assert_eq!(caps.foreground_color.value_copied(), None);

        // The DA1 fence, not the timeout, ended the probe.
        assert!(
            elapsed < Duration::from_secs(5),
            "the DA1 fence must end the probe fast, took {elapsed:?}"
        );
    }

    #[test]
    fn sync_probe_silent_terminal_returns_all_unknown_and_typeahead_survives() {
        // A fully silent terminal, but typeahead queued alongside. The probe must return an
        // all-unknown Capabilities after one timeout (no hang), and the typeahead must survive to
        // a later `read_input` — a probe never eats typeahead (FM-Q1).
        let (device, mut terminal) = FakeDevice::open().expect("open fake device");
        let mut session = TerminalSession::from_device(device).expect("start fake session");

        terminal.feed_input(b"hi").expect("feed typeahead");

        let caps = session
            .probe_capabilities(Duration::from_millis(150))
            .expect("silent terminal is not an error");

        assert!(
            caps.is_all_unknown(),
            "a silent terminal answers nothing: every field is None, got {caps:?}"
        );
        assert_eq!(caps.synchronized_output.evidence(), &Evidence::Unknown);
        assert_eq!(caps.kitty_keyboard.evidence(), &Evidence::Unknown);
        assert_eq!(caps.foreground_color.evidence(), &Evidence::Unknown);
        assert_eq!(caps.background_color.evidence(), &Evidence::Unknown);

        // The typeahead queued during the probe survives to a later read, in order.
        let mut buffer = [0u8; 16];
        let input = session
            .read_input(&mut buffer)
            .expect("read buffered typeahead");
        assert_eq!(input.as_bytes(), b"hi", "typeahead must survive the probe");
    }

    #[test]
    fn sync_probe_two_decrqm_modes_do_not_cross_complete() {
        // FM-Q10: the bundle carries concurrent DECRQM expectations for 2026 and 2027. Answer BOTH
        // with their correct modes (2026 set, 2027 reset). Each field must be set from its own
        // answer with no cross-completion.
        let (device, mut terminal) = FakeDevice::open().expect("open fake device");
        let mut session = TerminalSession::from_device(device).expect("start fake session");

        terminal
            .feed_input(b"\x1b[?2026;1$y\x1b[?2027;2$y\x1b[?1;2c")
            .expect("feed both DECRQM answers + fence");

        let caps = session
            .probe_capabilities(Duration::from_secs(30))
            .expect("probe returns capabilities");

        assert_eq!(
            caps.synchronized_output.value_copied(),
            Some(true),
            "mode 2026 answered SET -> Some(true)"
        );
        assert_eq!(
            caps.synchronized_output.evidence(),
            &Evidence::Probed { via: "DECRQM 2026" },
            "mode 2026 finding names its own query, not 2027's"
        );
        assert_eq!(
            caps.grapheme_clustering.value_copied(),
            Some(false),
            "mode 2027 answered RESET -> Some(false), not cross-completed with 2026's answer"
        );
        assert_eq!(
            caps.grapheme_clustering.evidence(),
            &Evidence::Probed { via: "DECRQM 2027" },
            "mode 2027 finding names its own query, not 2026's"
        );
    }
}
