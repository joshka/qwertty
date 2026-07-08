//! Terminal session lifecycle.
//!
//! A session is the first application-facing owner above the low-level terminal device. It enters
//! raw mode, preserves output ordering, reads raw input bytes, exposes explicit flushing, and gives
//! callers an explicit leave path for terminal-mode cleanup errors.
//!
//! Every reversible state change a session makes is recorded in an internal mode ledger with the
//! actions that apply and undo it. All lifecycle paths replay that one ledger:
//! [`TerminalSession::enter`] applies it, and orderly [`TerminalSession::leave`], drop, and (on
//! Unix) the panic-safe [`RestoreHandle`] undo it in reverse enablement order.

mod kitty;
mod ledger;
#[cfg(unix)]
mod restore;

pub use kitty::{KittyKeyboardFlags, KittyKeyboardGrant};
#[cfg(unix)]
pub use restore::RestoreHandle;

use crate::commands::terminal::MouseMode;
use crate::policy::{Policy, PolicyGate};
use crate::session::ledger::{ModeKind, ModeLedger, StateAction};
use crate::{
    Command, DeviceMode, InputBytes, Terminal, TerminalDevice, TerminalSize, commands, terminal,
};

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
/// The first session API is runtime-neutral and writes through the synchronous terminal-device
/// boundary. Input is exposed as raw bytes; async input, query routing, and runtime-owned I/O
/// belong to later session slices.
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
    #[cfg(unix)]
    restore: Option<RestoreHandle>,
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

        let mut session = Self {
            device: terminal,
            ledger: ModeLedger::new(),
            entered: false,
            policy: Policy::default(),
            #[cfg(unix)]
            restore,
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
    #[cfg(unix)]
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
            #[cfg(unix)]
            restore: None,
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

        #[cfg(unix)]
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

        #[cfg(unix)]
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
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot read input.
    pub fn read_input(&mut self, buffer: &mut [u8]) -> terminal::Result<InputBytes> {
        if buffer.is_empty() {
            return Ok(InputBytes::default());
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

    /// Returns a shared reference to the owned device.
    ///
    /// A driver that registers the same device's descriptor with a runtime reactor (the Tokio
    /// session) uses this to reach the device the session owns — for its pollable fd and its path —
    /// without taking it away from the session's mode ledger and restore paths.
    #[cfg_attr(
        not(all(feature = "tokio", unix)),
        expect(
            dead_code,
            reason = "tokio+unix async-driver helper; absent from other builds"
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
        not(all(feature = "tokio", unix)),
        expect(
            dead_code,
            reason = "tokio+unix async-driver helper; absent from other builds"
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
        #[cfg(unix)]
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
        self.record_mode(
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
        self.record_mode(
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
        self.record_mode(
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
        self.record_mode(
            ModeKind::InBandResize,
            &commands::terminal::enable_in_band_resize(),
            &commands::terminal::disable_in_band_resize(),
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
        self.record_mode_entry(
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
        self.record_mode(
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

    /// Records a byte-based mode entry, writing its enable bytes now and refreshing the emergency
    /// blob so its reset bytes are covered even before the next `enter`.
    fn record_mode(
        &mut self,
        kind: ModeKind,
        enable: &Command,
        disable: &Command,
    ) -> terminal::Result<&mut Self> {
        let mut apply = Vec::new();
        enable.encode(&mut apply);

        // Apply now so the mode is active for the caller's next read.
        self.device.write_all(&apply)?;
        self.record_mode_entry(kind, apply, disable);
        Ok(self)
    }

    /// Records a byte-based mode entry in the ledger and refreshes the emergency blob, **without**
    /// writing the enable bytes.
    ///
    /// The sync path writes through the device before calling this; the Tokio driver writes the
    /// enable bytes through its own readiness path and then calls this so the ledger and emergency
    /// blob learn the entry without a second, unordered write. `apply` is the already-encoded
    /// enable bytes (replayed by a later `enter`); `disable` is encoded here for the undo
    /// action.
    fn record_mode_entry(&mut self, kind: ModeKind, apply: Vec<u8>, disable: &Command) {
        let mut undo = Vec::new();
        disable.encode(&mut undo);
        self.ledger.record(
            kind,
            StateAction::WriteBytes(apply),
            StateAction::WriteBytes(undo),
        );

        // Refresh the emergency blob so a panic between now and the next `enter` still resets this
        // mode from the ledger's byte-based undo.
        #[cfg(unix)]
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
        not(all(feature = "tokio", unix)),
        expect(
            dead_code,
            reason = "tokio+unix async-driver helper; absent from other builds"
        )
    )]
    pub(crate) fn record_mouse_enabled(&mut self, mode: MouseMode) {
        let mut apply = Vec::new();
        commands::terminal::enable_mouse(mode).encode(&mut apply);
        self.record_mode_entry(
            ModeKind::Mouse,
            apply,
            &commands::terminal::disable_mouse(mode),
        );
    }

    /// Records an already-written focus-events enable in the ledger (Tokio driver path).
    #[cfg_attr(
        not(all(feature = "tokio", unix)),
        expect(
            dead_code,
            reason = "tokio+unix async-driver helper; absent from other builds"
        )
    )]
    pub(crate) fn record_focus_events_enabled(&mut self) {
        let mut apply = Vec::new();
        commands::terminal::enable_focus_events().encode(&mut apply);
        self.record_mode_entry(
            ModeKind::Focus,
            apply,
            &commands::terminal::disable_focus_events(),
        );
    }

    /// Records an already-written bracketed-paste enable in the ledger (Tokio driver path).
    #[cfg_attr(
        not(all(feature = "tokio", unix)),
        expect(
            dead_code,
            reason = "tokio+unix async-driver helper; absent from other builds"
        )
    )]
    pub(crate) fn record_bracketed_paste_enabled(&mut self) {
        let mut apply = Vec::new();
        commands::terminal::enable_bracketed_paste().encode(&mut apply);
        self.record_mode_entry(
            ModeKind::BracketedPaste,
            apply,
            &commands::terminal::disable_bracketed_paste(),
        );
    }

    /// Records an already-written in-band resize enable in the ledger (Tokio driver path).
    #[cfg_attr(
        not(all(feature = "tokio", unix)),
        expect(
            dead_code,
            reason = "tokio+unix async-driver helper; absent from other builds"
        )
    )]
    pub(crate) fn record_in_band_resize_enabled(&mut self) {
        let mut apply = Vec::new();
        commands::terminal::enable_in_band_resize().encode(&mut apply);
        self.record_mode_entry(
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
        not(all(feature = "tokio", unix)),
        expect(
            dead_code,
            reason = "tokio+unix async-driver helper; absent from other builds"
        )
    )]
    pub(crate) fn record_alternate_screen_entered(&mut self) {
        let mut apply = Vec::new();
        commands::screen::enter_alternate_screen().encode(&mut apply);
        commands::screen::clear().encode(&mut apply);
        self.record_mode_entry(
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
        not(all(feature = "tokio", unix)),
        expect(
            dead_code,
            reason = "tokio+unix async-driver helper; absent from other builds"
        )
    )]
    pub(crate) fn record_cursor_hidden(&mut self) {
        let mut apply = Vec::new();
        commands::cursor::hide().encode(&mut apply);
        self.record_mode_entry(ModeKind::CursorVisibility, apply, &commands::cursor::show());
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
}
