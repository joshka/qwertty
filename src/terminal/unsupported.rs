//! Unsupported terminal device implementation.

use std::path::{Path, PathBuf};

use crate::terminal::{DeviceMode, Error, Result, TerminalDevice, TerminalSize};

const PLATFORM: &str = "this platform";

/// Terminal device placeholder for platforms without a live implementation yet.
#[derive(Debug)]
pub struct Terminal {
    path: PathBuf,
}

impl Terminal {
    /// Opens the current terminal.
    ///
    /// # Errors
    ///
    /// Always returns [`Error::Unsupported`] on this platform.
    pub fn open() -> Result<Self> {
        Err(Error::unsupported("open terminal device", PLATFORM))
    }

    /// Opens a specific terminal path.
    ///
    /// # Errors
    ///
    /// Always returns [`Error::Unsupported`] on this platform.
    pub fn open_path(path: impl Into<PathBuf>) -> Result<Self> {
        let _ = path.into();
        Err(Error::unsupported("open terminal device path", PLATFORM))
    }

    /// Returns the path used to open the terminal device.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the current terminal size.
    ///
    /// # Errors
    ///
    /// Always returns [`Error::Unsupported`] on this platform.
    pub fn size(&self) -> Result<TerminalSize> {
        Err(Error::unsupported("query terminal size", PLATFORM))
    }

    /// Enters raw mode.
    ///
    /// # Errors
    ///
    /// Always returns [`Error::Unsupported`] on this platform.
    pub fn set_raw_mode(&self) -> Result<()> {
        Err(Error::unsupported("enter raw mode", PLATFORM))
    }

    /// Restores cooked mode.
    ///
    /// # Errors
    ///
    /// Always returns [`Error::Unsupported`] on this platform.
    pub fn set_cooked_mode(&self) -> Result<()> {
        Err(Error::unsupported("restore cooked mode", PLATFORM))
    }

    /// Writes all bytes to the terminal device.
    ///
    /// # Errors
    ///
    /// Always returns [`std::io::ErrorKind::Unsupported`] on this platform.
    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        let _ = bytes;
        Err(Error::unsupported("write terminal output", PLATFORM))
    }

    /// Reads bytes from the terminal device.
    ///
    /// # Errors
    ///
    /// Always returns [`Error::Unsupported`] on this platform.
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        let _ = buffer;
        Err(Error::unsupported("read terminal input", PLATFORM))
    }

    /// Flushes buffered terminal output.
    ///
    /// # Errors
    ///
    /// Always returns [`std::io::ErrorKind::Unsupported`] on this platform.
    pub fn flush(&mut self) -> Result<()> {
        Err(Error::unsupported("flush terminal output", PLATFORM))
    }
}

impl TerminalDevice for Terminal {
    fn set_mode(&mut self, mode: DeviceMode) -> Result<()> {
        let _ = mode;
        Err(Error::unsupported("set terminal mode", PLATFORM))
    }

    fn size(&self) -> Result<TerminalSize> {
        Self::size(self)
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        Self::read(self, buffer)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        Self::write_all(self, bytes)
    }

    fn flush(&mut self) -> Result<()> {
        Self::flush(self)
    }
}
