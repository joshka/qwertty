//! Terminal device ownership.
//!
//! This module owns the low-level operating-system terminal boundary. It opens a terminal device,
//! captures the original terminal mode, enters raw mode, restores cooked mode, queries terminal
//! size, and writes bytes. It does not parse input, route terminal queries, enter the alternate
//! screen, or clean up emulator protocol state.
//!
//! [`TerminalDevice`] is the substitutable seam over that boundary: [`Terminal`] implements it
//! for a live terminal, and `FakeDevice` implements it in process for headless tests on Unix.

use std::time::Duration;
use std::{error, fmt, io};

mod device;
#[cfg(unix)]
mod fake;
#[cfg(unix)]
mod unix;
#[cfg(not(unix))]
mod unsupported;

pub use device::{DeviceMode, TerminalDevice};
#[cfg(unix)]
pub use fake::{FakeDevice, FakeTerminal};
#[cfg(unix)]
pub use unix::Terminal;
#[cfg(not(unix))]
pub use unsupported::Terminal;

/// Result alias for terminal device operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Terminal dimensions reported by the operating system.
///
/// `columns` and `rows` are measured in terminal cells. This is a snapshot, not a subscription to
/// future resize events. Later session and input layers will own resize event routing.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TerminalSize {
    columns: u16,
    rows: u16,
}

impl TerminalSize {
    /// Creates a terminal size from cell dimensions.
    #[must_use]
    pub const fn new(columns: u16, rows: u16) -> Self {
        Self { columns, rows }
    }

    /// Returns the terminal width in character cells.
    #[must_use]
    pub const fn columns(self) -> u16 {
        self.columns
    }

    /// Returns the terminal height in character cells.
    #[must_use]
    pub const fn rows(self) -> u16 {
        self.rows
    }
}

/// Terminal dimensions measured in pixels.
///
/// This is the optional pixel geometry an in-band resize report (DEC mode 2048) can carry
/// alongside the cell dimensions: a report whose pixel fields are nonzero decodes to a
/// [`ResizeEvent`](crate::event::ResizeEvent) with `Some(PixelSize)`, and one whose pixel fields
/// are zero (or absent) decodes with `None`. The syntax mirrors the wire order width-then-height
/// is *not* used here — the accessors name the axis so callers never have to remember the report's
/// field order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PixelSize {
    width: u16,
    height: u16,
}

impl PixelSize {
    /// Creates a pixel size from width and height in pixels.
    #[must_use]
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }

    /// Returns the terminal width in pixels.
    #[must_use]
    pub const fn width(self) -> u16 {
        self.width
    }

    /// Returns the terminal height in pixels.
    #[must_use]
    pub const fn height(self) -> u16 {
        self.height
    }
}

/// Error returned by terminal device operations.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Opening a terminal device failed.
    OpenTerminal {
        /// Source I/O error.
        source: io::Error,
    },
    /// Reading terminal mode attributes failed.
    GetTerminalMode {
        /// Source I/O error.
        source: io::Error,
    },
    /// Applying terminal mode attributes failed.
    SetTerminalMode {
        /// Source I/O error.
        source: io::Error,
    },
    /// Querying terminal dimensions failed.
    GetTerminalSize {
        /// Source I/O error.
        source: io::Error,
    },
    /// The reported terminal dimensions are a known degenerate value, not a real measurement.
    ///
    /// Piped stdio, some CI environments, and some IDE terminals report zero or `u16::MAX`
    /// dimensions. Callers receiving this error should apply their own default size.
    InvalidTerminalSize {
        /// Reported column count.
        columns: u16,
        /// Reported row count.
        rows: u16,
    },
    /// Writing or flushing terminal output failed.
    WriteTerminal {
        /// Source I/O error.
        source: io::Error,
    },
    /// Reading terminal input failed.
    ReadTerminal {
        /// Source I/O error.
        source: io::Error,
    },
    /// A live terminal query did not receive its expected response before the timeout elapsed.
    QueryTimeout {
        /// Query operation that timed out.
        operation: &'static str,
        /// Timeout used for the query.
        timeout: Duration,
    },
    /// The current platform does not support the requested operation yet.
    Unsupported {
        /// Operation that was requested.
        operation: &'static str,
        /// Platform family that rejected the operation.
        platform: &'static str,
    },
}

impl Error {
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn open_terminal(source: io::Error) -> Self {
        Self::OpenTerminal { source }
    }

    #[cfg(unix)]
    pub(crate) fn get_terminal_mode(source: io::Error) -> Self {
        Self::GetTerminalMode { source }
    }

    #[cfg(unix)]
    pub(crate) fn set_terminal_mode(source: io::Error) -> Self {
        Self::SetTerminalMode { source }
    }

    #[cfg(unix)]
    pub(crate) fn get_terminal_size(source: io::Error) -> Self {
        Self::GetTerminalSize { source }
    }

    #[cfg(unix)]
    pub(crate) fn write_terminal(source: io::Error) -> Self {
        Self::WriteTerminal { source }
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn read_terminal(source: io::Error) -> Self {
        Self::ReadTerminal { source }
    }

    #[cfg(all(feature = "tokio", unix))]
    pub(crate) const fn query_timeout(operation: &'static str, timeout: Duration) -> Self {
        Self::QueryTimeout { operation, timeout }
    }

    #[cfg(any(not(unix), all(feature = "tokio", unix)))]
    pub(crate) const fn unsupported(operation: &'static str, platform: &'static str) -> Self {
        Self::Unsupported {
            operation,
            platform,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenTerminal { .. } => f.write_str("failed to open terminal device"),
            Self::GetTerminalMode { .. } => f.write_str("failed to get terminal mode"),
            Self::SetTerminalMode { .. } => f.write_str("failed to set terminal mode"),
            Self::GetTerminalSize { .. } => f.write_str("failed to get terminal size"),
            Self::InvalidTerminalSize { columns, rows } => {
                write!(f, "terminal reported a degenerate size of {columns}x{rows}")
            }
            Self::WriteTerminal { .. } => f.write_str("failed to write terminal output"),
            Self::ReadTerminal { .. } => f.write_str("failed to read terminal input"),
            Self::QueryTimeout { operation, timeout } => {
                write!(f, "{operation} timed out after {timeout:?}")
            }
            Self::Unsupported {
                operation,
                platform,
            } => {
                write!(f, "{operation} is not supported on {platform}")
            }
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::OpenTerminal { source }
            | Self::GetTerminalMode { source }
            | Self::SetTerminalMode { source }
            | Self::GetTerminalSize { source }
            | Self::WriteTerminal { source }
            | Self::ReadTerminal { source } => Some(source),
            Self::InvalidTerminalSize { .. }
            | Self::QueryTimeout { .. }
            | Self::Unsupported { .. } => None,
        }
    }
}
