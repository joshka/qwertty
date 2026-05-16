//! Terminal session lifecycle.
//!
//! A session is the first application-facing owner above the low-level terminal device. It enters
//! raw mode, preserves output ordering, exposes explicit flushing, and gives callers an explicit
//! leave path for terminal-mode cleanup errors.

use crate::{Command, Terminal, TerminalSize, terminal};

/// An active terminal session.
///
/// `TerminalSession` owns a [`Terminal`] for application output. Creating a session enters raw mode
/// so later input and query layers can receive terminal bytes directly. Call
/// [`TerminalSession::leave`] during orderly shutdown so terminal-mode restoration errors can be
/// handled.
///
/// Dropping a session without calling [`TerminalSession::leave`] still relies on the underlying
/// [`Terminal`] drop fallback to restore cooked mode, but drop-time failures cannot be reported.
///
/// The first session API is runtime-neutral and writes through the synchronous terminal-device
/// boundary. Async input, query routing, and runtime-owned I/O belong to later session slices.
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
    /// Returns an error when raw mode cannot be entered.
    pub fn from_terminal(terminal: Terminal) -> terminal::Result<Self> {
        terminal.set_raw_mode()?;
        Ok(Self { terminal })
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

    /// Leaves the session and restores cooked mode.
    ///
    /// This is the orderly cleanup path. It reports terminal-mode restoration errors to the
    /// caller. It does not add protocol cleanup beyond raw-mode restoration: alternate screen,
    /// cursor visibility, mouse mode, paste mode, graphics, clipboard, and vendor protocol cleanup
    /// belong to later policy-aware session slices.
    ///
    /// Call [`TerminalSession::flush`] before `leave` when output visibility matters. Drop still
    /// attempts best-effort cooked-mode restoration through the underlying terminal, but drop-time
    /// failures cannot be returned.
    ///
    /// # Errors
    ///
    /// Returns an error when cooked mode cannot be restored.
    pub fn leave(self) -> terminal::Result<()> {
        self.terminal.set_cooked_mode()
    }

    fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }
}
