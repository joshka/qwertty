//! In-process fake terminal device for headless tests.

use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::terminal::{DeviceMode, Error, Result, TerminalDevice, TerminalSize};

const DEFAULT_SIZE: TerminalSize = TerminalSize::new(80, 24);

/// A fake terminal device backed by an in-process socket pair.
///
/// `FakeDevice` implements [`TerminalDevice`] without opening a pseudoterminal or touching
/// operating-system terminal state. The paired [`FakeTerminal`] plays the terminal emulator role:
/// it feeds input bytes and observes output bytes, so session logic and downstream tests run
/// headless in ordinary unit tests.
///
/// The device side is backed by a real file descriptor, so readiness-driven drivers can register
/// it exactly like a live terminal.
///
/// # Example
///
/// ```
/// use qwertty::{DeviceMode, FakeDevice, TerminalDevice};
///
/// # fn main() -> qwertty::Result<()> {
/// let (mut device, mut terminal) = FakeDevice::open()?;
///
/// device.set_mode(DeviceMode::Raw)?;
/// device.write_all(b"\x1b[2J")?;
/// device.flush()?;
///
/// terminal.feed_input(b"hello")?;
/// let mut buffer = [0; 8];
/// let read = device.read(&mut buffer)?;
///
/// assert_eq!(&buffer[..read], b"hello");
/// assert_eq!(terminal.output()?, b"\x1b[2J");
/// assert_eq!(terminal.modes(), [DeviceMode::Raw]);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct FakeDevice {
    stream: UnixStream,
    state: Arc<Mutex<FakeState>>,
}

/// The terminal emulator side of a [`FakeDevice`] pair.
///
/// Tests use this half to script what the pretend terminal sends and to assert on what the
/// application side wrote. See [`FakeDevice`] for a round-trip example.
#[derive(Debug)]
pub struct FakeTerminal {
    stream: UnixStream,
    state: Arc<Mutex<FakeState>>,
}

#[derive(Debug)]
struct FakeState {
    size: TerminalSize,
    modes: Vec<DeviceMode>,
}

impl FakeDevice {
    /// Opens a connected fake device and fake terminal pair.
    ///
    /// The reported size starts at 80x24 columns by rows. Change it with
    /// [`FakeTerminal::set_size`].
    ///
    /// # Errors
    ///
    /// Returns an error when the socket pair cannot be created.
    pub fn open() -> Result<(Self, FakeTerminal)> {
        let (device_stream, terminal_stream) = UnixStream::pair().map_err(Error::open_terminal)?;
        terminal_stream
            .set_nonblocking(true)
            .map_err(Error::open_terminal)?;

        let state = Arc::new(Mutex::new(FakeState {
            size: DEFAULT_SIZE,
            modes: Vec::new(),
        }));

        let device = Self {
            stream: device_stream,
            state: Arc::clone(&state),
        };
        let terminal = FakeTerminal {
            stream: terminal_stream,
            state,
        };

        Ok((device, terminal))
    }
}

impl TerminalDevice for FakeDevice {
    /// Records the requested mode instead of changing operating-system state.
    ///
    /// # Errors
    ///
    /// This implementation does not fail.
    fn set_mode(&mut self, mode: DeviceMode) -> Result<()> {
        lock(&self.state).modes.push(mode);
        Ok(())
    }

    fn size(&self) -> Result<TerminalSize> {
        Ok(lock(&self.state).size)
    }

    /// Reads bytes fed by the paired [`FakeTerminal`].
    ///
    /// Like a live terminal read, this blocks until input is available, so tests should feed
    /// input before reading.
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        self.stream.read(buffer).map_err(Error::read_terminal)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.stream.write_all(bytes).map_err(Error::write_terminal)
    }

    fn flush(&mut self) -> Result<()> {
        self.stream.flush().map_err(Error::write_terminal)
    }

    fn as_fd(&self) -> Option<BorrowedFd<'_>> {
        Some(self.stream.as_fd())
    }
}

impl FakeTerminal {
    /// Feeds input bytes for the device side to read.
    ///
    /// # Errors
    ///
    /// Returns an error when the bytes cannot be written to the pair.
    pub fn feed_input(&mut self, bytes: &[u8]) -> Result<()> {
        self.stream.write_all(bytes).map_err(Error::write_terminal)
    }

    /// Drains and returns the bytes the device side has written so far.
    ///
    /// This does not wait for output: it returns whatever has already arrived, which may be
    /// empty.
    ///
    /// # Errors
    ///
    /// Returns an error when reading from the pair fails.
    pub fn output(&mut self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => output.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(Error::read_terminal(error)),
            }
        }
        Ok(output)
    }

    /// Sets the terminal size the device side reports.
    pub fn set_size(&mut self, size: TerminalSize) {
        lock(&self.state).size = size;
    }

    /// Returns the device modes requested so far, in request order.
    #[must_use]
    pub fn modes(&self) -> Vec<DeviceMode> {
        lock(&self.state).modes.clone()
    }
}

fn lock(state: &Mutex<FakeState>) -> MutexGuard<'_, FakeState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_output_and_input() {
        let (mut device, mut terminal) = FakeDevice::open().expect("open fake device");

        device.write_all(b"\x1b[2J").expect("write");
        device.flush().expect("flush");
        terminal.feed_input(b"hi").expect("feed");

        let mut buffer = [0; 4];
        let read = device.read(&mut buffer).expect("read");

        assert_eq!(&buffer[..read], b"hi");
        assert_eq!(terminal.output().expect("output"), b"\x1b[2J");
    }

    #[test]
    fn records_mode_requests_in_order() {
        let (mut device, terminal) = FakeDevice::open().expect("open fake device");

        device.set_mode(DeviceMode::Raw).expect("raw");
        device.set_mode(DeviceMode::Cooked).expect("cooked");

        assert_eq!(terminal.modes(), [DeviceMode::Raw, DeviceMode::Cooked]);
    }

    #[test]
    fn reports_scripted_size() {
        let (device, mut terminal) = FakeDevice::open().expect("open fake device");

        assert_eq!(device.size().expect("size"), TerminalSize::new(80, 24));

        terminal.set_size(TerminalSize::new(120, 40));

        assert_eq!(device.size().expect("size"), TerminalSize::new(120, 40));
    }

    #[test]
    fn output_is_empty_before_any_write() {
        let (_device, mut terminal) = FakeDevice::open().expect("open fake device");

        assert_eq!(terminal.output().expect("output"), Vec::<u8>::new());
    }

    #[test]
    fn exposes_a_pollable_descriptor() {
        let (device, _terminal) = FakeDevice::open().expect("open fake device");

        assert!(device.as_fd().is_some());
    }
}
