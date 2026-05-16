//! Unix terminal device implementation.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use rustix::termios::{OptionalActions, Termios, tcgetattr, tcgetwinsize, tcsetattr};

use crate::terminal::{Error, Result, TerminalSize};

const DEV_TTY: &str = "/dev/tty";

/// An open Unix terminal device.
///
/// `Terminal` captures the original terminal mode when it opens the device. Call
/// [`Terminal::set_cooked_mode`] during orderly shutdown so restoration errors can be handled.
/// Drop also attempts best-effort restoration, but drop-time failures cannot be reported.
#[derive(Debug)]
pub struct Terminal {
    device: File,
    original_mode: Termios,
    path: PathBuf,
}

impl Terminal {
    /// Opens the current controlling terminal.
    ///
    /// This opens `/dev/tty`, which addresses the process controlling terminal instead of wrapping
    /// process stdin or stdout.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal cannot be opened or its current mode cannot be captured.
    pub fn open() -> Result<Self> {
        Self::open_path(DEV_TTY)
    }

    /// Opens a specific terminal device path.
    ///
    /// This is mainly useful for tests, embedding environments, and advanced callers that have
    /// already resolved the terminal device they want to own.
    ///
    /// # Errors
    ///
    /// Returns an error when the path cannot be opened as a terminal device or its current mode
    /// cannot be captured.
    pub fn open_path(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let device = open_read_write(&path).map_err(Error::open_terminal)?;
        let original_mode = tcgetattr(&device)
            .map_err(io::Error::from)
            .map_err(Error::get_terminal_mode)?;

        Ok(Self {
            device,
            original_mode,
            path,
        })
    }

    /// Returns the path used to open the terminal device.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the current terminal size.
    ///
    /// The result is a snapshot. This method does not subscribe to future resize events.
    ///
    /// # Errors
    ///
    /// Returns an error when the operating system cannot read the terminal size.
    pub fn size(&self) -> Result<TerminalSize> {
        let size = tcgetwinsize(&self.device)
            .map_err(io::Error::from)
            .map_err(Error::get_terminal_size)?;

        Ok(TerminalSize::new(size.ws_col, size.ws_row))
    }

    /// Enters raw mode.
    ///
    /// Raw mode is derived from the terminal mode captured by [`Terminal::open`] or
    /// [`Terminal::open_path`]. It disables canonical input processing and local echo so later
    /// input code can receive terminal bytes directly.
    ///
    /// The mode is applied with `TCSAFLUSH`, so unread canonical input may be discarded while
    /// entering raw mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal mode cannot be applied.
    pub fn set_raw_mode(&self) -> Result<()> {
        let mut raw = self.original_mode.clone();
        raw.make_raw();
        tcsetattr(&self.device, OptionalActions::Flush, &raw)
            .map_err(io::Error::from)
            .map_err(Error::set_terminal_mode)
    }

    /// Restores the terminal mode captured when this terminal was opened.
    ///
    /// Use this explicit restoration path during orderly shutdown. Drop-time restoration is only a
    /// last line of defense because it cannot report errors to the caller.
    ///
    /// # Errors
    ///
    /// Returns an error when the captured terminal mode cannot be restored.
    pub fn set_cooked_mode(&self) -> Result<()> {
        tcsetattr(&self.device, OptionalActions::Now, &self.original_mode)
            .map_err(io::Error::from)
            .map_err(Error::set_terminal_mode)
    }

    /// Writes all bytes to the terminal device.
    ///
    /// This method does not inspect or escape the bytes. Command bytes and text bytes are written
    /// exactly as provided.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the operating system cannot write all bytes.
    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.device.write_all(bytes).map_err(Error::write_terminal)
    }

    /// Reads bytes from the terminal device.
    ///
    /// This method reads raw bytes from the terminal device without parsing or decoding them. In
    /// raw mode, ordinary keys, control bytes, Escape-prefixed sequences, query responses, paste
    /// payloads, and vendor protocol bytes all pass through this same boundary.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the operating system cannot read terminal input.
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        self.device.read(buffer).map_err(Error::read_terminal)
    }

    /// Flushes buffered terminal output.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the operating system cannot flush the device.
    pub fn flush(&mut self) -> Result<()> {
        self.device.flush().map_err(Error::write_terminal)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        _ = self.set_cooked_mode();
    }
}

impl Write for Terminal {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.device.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.device.flush()
    }
}

fn open_read_write(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}
