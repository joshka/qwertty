//! Terminal session lifecycle.
//!
//! A session is the first application-facing owner above the low-level terminal device. It enters
//! raw mode, preserves output ordering, reads raw input bytes, exposes explicit flushing, and gives
//! callers an explicit leave path for terminal-mode cleanup errors.
//!
//! Every reversible state change a session makes is recorded in an internal mode ledger with the
//! action that undoes it. All exit paths replay that one ledger: orderly leave, drop, and (on
//! Unix) the panic-safe [`RestoreHandle`] returned by [`TerminalSession::restore_handle`].

mod ledger;
#[cfg(unix)]
mod restore;

#[cfg(unix)]
pub use restore::RestoreHandle;

use crate::session::ledger::{ModeKind, ModeLedger, UndoAction};
use crate::{Command, DeviceMode, InputBytes, Terminal, TerminalSize, terminal};

/// An active terminal session.
///
/// `TerminalSession` owns a [`Terminal`] for application output. Creating a session enters raw mode
/// so later input and query layers can receive terminal bytes directly. Call
/// [`TerminalSession::leave`] during orderly shutdown so terminal-mode restoration errors can be
/// handled.
///
/// The session records every reversible state change in a mode ledger and replays it in reverse
/// enablement order exactly once, on whichever exit path runs first: [`TerminalSession::leave`],
/// drop, or the panic-safe [`RestoreHandle`] on Unix. Dropping a session without calling `leave`
/// therefore still restores the terminal, but drop-time failures cannot be reported.
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
pub struct TerminalSession {
    terminal: Terminal,
    ledger: ModeLedger,
    #[cfg(unix)]
    restore: RestoreHandle,
    #[cfg(not(unix))]
    left: bool,
}

impl TerminalSession {
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
        let restore = RestoreHandle::new(emergency_device(&terminal)?, terminal.cooked_mode());

        terminal.set_raw_mode()?;

        let mut ledger = ModeLedger::new();
        ledger.record(ModeKind::Raw, UndoAction::SetMode(DeviceMode::Cooked));

        #[cfg(unix)]
        restore.publish_blob(&ledger.protocol_undo_bytes());

        Ok(Self {
            terminal,
            ledger,
            #[cfg(unix)]
            restore,
            #[cfg(not(unix))]
            left: false,
        })
    }

    /// Returns a panic-safe restore handle for this session.
    ///
    /// The handle stays valid without borrowing the session, so it can live inside a panic hook.
    /// See [`RestoreHandle`] for the hook pattern and what the emergency path covers.
    #[cfg(unix)]
    #[must_use]
    pub fn restore_handle(&self) -> RestoreHandle {
        self.restore.clone()
    }

    /// Returns the current terminal size.
    ///
    /// The result is a snapshot. This method does not subscribe to future resize events.
    ///
    /// # Errors
    ///
    /// Returns an error when the operating system cannot read the terminal size.
    pub fn size(&self) -> terminal::Result<TerminalSize> {
        self.terminal().size()
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
        self.terminal_mut().write_all(bytes.as_ref())?;
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

        let len = self.terminal_mut().read(buffer)?;
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
        self.terminal_mut().flush()?;
        Ok(self)
    }

    /// Leaves the session and restores the terminal.
    ///
    /// This is the orderly cleanup path. It replays the session's mode ledger in reverse
    /// enablement order, attempts every step even after a failure, flushes, and reports the first
    /// error. Today the ledger holds raw-mode restoration; alternate screen, cursor visibility,
    /// mouse mode, paste mode, and vendor protocol cleanup join it in later slices.
    ///
    /// If the panic-safe restore handle already restored the terminal, `leave` does nothing and
    /// returns success.
    ///
    /// Call [`TerminalSession::flush`] before `leave` when output visibility matters.
    ///
    /// # Errors
    ///
    /// Returns the first error encountered while restoring terminal state.
    pub fn leave(mut self) -> terminal::Result<()> {
        #[cfg(unix)]
        if !self.restore.mark_restored() {
            return Ok(());
        }
        #[cfg(not(unix))]
        {
            self.left = true;
        }

        self.replay_ledger()
    }

    /// Replays the mode ledger in reverse enablement order, reporting the first error.
    fn replay_ledger(&mut self) -> terminal::Result<()> {
        let mut first_error = None;
        for undo in self.ledger.drain_reversed() {
            let result = match undo {
                UndoAction::WriteBytes(bytes) => self.terminal.write_all(&bytes),
                UndoAction::SetMode(DeviceMode::Cooked) => self.terminal.set_cooked_mode(),
                UndoAction::SetMode(DeviceMode::Raw) => self.terminal.set_raw_mode(),
            };
            if let Err(error) = result
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }

        if let Err(error) = self.terminal.flush()
            && first_error.is_none()
        {
            first_error = Some(error);
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.restore.mark_restored() {
            _ = self.replay_ledger();
        }
        #[cfg(not(unix))]
        if !self.left {
            _ = self.replay_ledger();
        }
    }
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
