//! Substitutable terminal device boundary.

use crate::terminal::{Result, TerminalSize};

/// Operating-system terminal mode selected through [`TerminalDevice::set_mode`].
///
/// This type stays a two-state choice on purpose. Richer terminal state, such as the alternate
/// screen or bracketed paste, is protocol bytes written through the device, not a device mode.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeviceMode {
    /// Raw mode: canonical input processing and local echo are disabled so input code receives
    /// terminal bytes directly.
    Raw,
    /// Cooked mode: the mode captured when the device was opened, restored for orderly shutdown.
    Cooked,
}

/// A byte-level terminal device that sessions can own.
///
/// This trait is the seam between session logic and a concrete terminal. The live implementation
/// is [`Terminal`](crate::Terminal). On Unix, `FakeDevice` implements the same trait over an
/// in-process socket pair so sessions and downstream tests run headless, without a
/// pseudoterminal.
///
/// # Contract
///
/// - [`read`](TerminalDevice::read) blocks like a terminal read and returns `Ok(0)` only at end of
///   input. Drivers that need readiness instead of blocking use [`as_fd`](TerminalDevice::as_fd).
/// - [`write_all`](TerminalDevice::write_all) writes bytes exactly as provided, in call order,
///   without escaping or interpretation.
/// - [`set_mode`](TerminalDevice::set_mode) translates the mode to the platform mechanism. Fake
///   implementations may record the request instead of changing operating-system state.
///
/// # Example
///
/// Session-style code writes through the trait so any device fits:
///
/// ```
/// use qwertty::{CommandBuffer, TerminalDevice, commands};
///
/// fn paint(device: &mut impl TerminalDevice) -> qwertty::Result<()> {
///     let mut output = CommandBuffer::new();
///     output.command(commands::screen::clear()).text("Ready");
///     device.write_all(output.as_bytes())?;
///     device.flush()
/// }
/// ```
pub trait TerminalDevice {
    /// Applies a terminal mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the mode cannot be applied.
    fn set_mode(&mut self, mode: DeviceMode) -> Result<()>;

    /// Returns the current terminal size.
    ///
    /// The result is a snapshot, not a subscription to future resize events.
    ///
    /// # Errors
    ///
    /// Returns an error when the size cannot be determined.
    fn size(&self) -> Result<TerminalSize>;

    /// Reads bytes from the terminal.
    ///
    /// # Errors
    ///
    /// Returns an error when the read fails.
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize>;

    /// Writes all bytes to the terminal.
    ///
    /// # Errors
    ///
    /// Returns an error when the bytes cannot all be written.
    fn write_all(&mut self, bytes: &[u8]) -> Result<()>;

    /// Flushes buffered terminal output.
    ///
    /// # Errors
    ///
    /// Returns an error when the flush fails.
    fn flush(&mut self) -> Result<()>;

    /// Returns the readable file descriptor behind this device, when one exists.
    ///
    /// Readiness-driven drivers register this descriptor instead of blocking in
    /// [`read`](TerminalDevice::read). Devices without a pollable descriptor return `None`.
    #[cfg(unix)]
    fn as_fd(&self) -> Option<std::os::fd::BorrowedFd<'_>> {
        None
    }

    /// Returns the console input and output handles behind this device, when it owns a console.
    ///
    /// This is the Windows analogue of [`as_fd`](TerminalDevice::as_fd): a console `HANDLE` is not
    /// a pollable descriptor, so the async driver cannot register it with a reactor, but it *is* a
    /// waitable object the readiness worker duplicates and waits on (ADR 0022 §4). The worker reads
    /// input from the returned input handle and writes output to the returned output handle; a
    /// device that owns no console returns `None` and is rejected the same way an fd-less device is
    /// on Unix. The live [`Terminal`](crate::Terminal) returns `Some`; every other device defaults
    /// to `None`.
    ///
    /// Present only with the `tokio` feature, whose async readiness worker is the sole consumer.
    #[cfg(all(windows, feature = "tokio"))]
    fn as_console_handles(&self) -> Option<crate::terminal::ConsoleHandles<'_>> {
        None
    }
}
