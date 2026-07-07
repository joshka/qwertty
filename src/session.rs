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

mod ledger;
#[cfg(unix)]
mod restore;

#[cfg(unix)]
pub use restore::RestoreHandle;

use crate::session::ledger::{ModeKind, ModeLedger, StateAction};
use crate::{Command, DeviceMode, InputBytes, Terminal, TerminalDevice, TerminalSize, terminal};

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
    #[allow(
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
    /// first error. Today the ledger holds raw-mode restoration; alternate screen, cursor
    /// visibility, mouse mode, paste mode, and vendor protocol cleanup join it in later slices.
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

    /// Records the terminal state every session applies on entry.
    fn record_initial_state(&mut self) {
        self.ledger.record(
            ModeKind::Raw,
            StateAction::SetMode(DeviceMode::Raw),
            StateAction::SetMode(DeviceMode::Cooked),
        );
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
