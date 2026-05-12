//! Cursor command helpers.
//!
//! Cursor positioning commands use terminal protocol coordinates: row 1, column 1 is the top-left
//! cell in the active terminal coordinate system. Use [`ProtocolPosition`] at this boundary rather
//! than passing zero-based layout indexes directly.
//!
//! The positioning helper in this module uses ECMA-48 control functions. The terms ECMA-48, CSI,
//! and CUP are introduced in [Command Anatomy](crate::docs#command-anatomy) and
//! [Cursor Position](crate::docs#cursor-position).
//!
//! Cursor visibility and save/restore are terminal state changes represented as encoded
//! [`Command`] values. They do not track state locally; future session code is responsible for
//! cleanup and policy around real terminal output.
//!
//! ```
//! use qwertty::commands::cursor;
//! use qwertty::{CommandBuffer, ProtocolPosition};
//!
//! let mut frame = CommandBuffer::new();
//! frame
//!     .command(cursor::save())
//!     .command(cursor::move_to(ProtocolPosition::ORIGIN))
//!     .text("top")
//!     .command(cursor::restore());
//!
//! assert_eq!(frame.as_bytes(), b"\x1b7\x1b[1;1Htop\x1b8");
//! ```

use crate::{Command, ProtocolPosition, escape};

/// Moves the cursor to a one-based protocol position.
///
/// This encodes ECMA-48 CUP, "Cursor Position". CUP is written as
/// `CSI row ; column H`, where CSI is emitted as `ESC [` by qwertty.
/// See [Cursor Position](crate::docs#cursor-position) for the protocol terms.
///
/// `ProtocolPosition::new(3, 5)` emits `b"\x1b[3;5H"`.
///
/// # Example
///
/// ```
/// use qwertty::commands::cursor;
/// use qwertty::{CommandBuffer, ProtocolPosition};
///
/// let mut frame = CommandBuffer::new();
/// frame.command(cursor::move_to(ProtocolPosition::new(3, 5)));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[3;5H");
/// ```
#[must_use]
pub fn move_to(position: ProtocolPosition) -> Command {
    escape::csi(format!("{};{}", position.row(), position.column()), 'H')
}

/// Hides the cursor.
///
/// This encodes the commonly supported xterm/DEC private cursor-visibility mode reset:
/// `CSI ? 25 l`, emitted as `b"\x1b[?25l"`.
///
/// See [Cursor Visibility](crate::docs#cursor-visibility) for the protocol details.
///
/// Hiding the cursor changes terminal state. Code that writes this command to a real terminal
/// should arrange to show the cursor again during cleanup.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::cursor;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(cursor::hide());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?25l");
/// ```
#[must_use]
pub fn hide() -> Command {
    escape::csi("?25", 'l')
}

/// Shows the cursor.
///
/// This encodes the commonly supported xterm/DEC private cursor-visibility mode set:
/// `CSI ? 25 h`, emitted as `b"\x1b[?25h"`.
///
/// Use this as the cleanup pair for [`hide`].
/// See [Cursor Visibility](crate::docs#cursor-visibility) for the protocol details.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::cursor;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(cursor::show());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?25h");
/// ```
#[must_use]
pub fn show() -> Command {
    escape::csi("?25", 'h')
}

/// Saves the current cursor position.
///
/// This emits the DEC save-cursor sequence `ESC 7`, written as `b"\x1b7"`.
///
/// See [Cursor Save And Restore](crate::docs#cursor-save-and-restore) for the protocol details.
///
/// Save/restore support is widespread, but the saved position is terminal state. Prefer using the
/// pair within a narrow output frame.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::cursor;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(cursor::save());
///
/// assert_eq!(frame.as_bytes(), b"\x1b7");
/// ```
#[must_use]
pub fn save() -> Command {
    escape::escape(b'7')
}

/// Restores the previously saved cursor position.
///
/// This emits the DEC restore-cursor sequence `ESC 8`, written as `b"\x1b8"`.
///
/// See [Cursor Save And Restore](crate::docs#cursor-save-and-restore) for the protocol details.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::cursor;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(cursor::restore());
///
/// assert_eq!(frame.as_bytes(), b"\x1b8");
/// ```
#[must_use]
pub fn restore() -> Command {
    escape::escape(b'8')
}
