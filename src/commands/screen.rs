//! Screen command helpers.
//!
//! These helpers encode common screen and line erase operations. They only build command bytes;
//! they do not know the current cursor position, scrollback behavior, terminal size, or emulator
//! support. Real terminal ownership and cleanup belong to later session layers.
//!
//! These helpers use ECMA-48 erase controls. The terms ECMA-48, CSI, ED, and EL are introduced in
//! [Command Anatomy](crate::docs#command-anatomy), [Erase In
//! Display](crate::docs#erase-in-display), and [Erase In Line](crate::docs#erase-in-line).
//!
//! ```
//! use qwertty::CommandBuffer;
//! use qwertty::commands::screen;
//!
//! let mut frame = CommandBuffer::new();
//! frame.command(screen::clear()).text("Ready");
//!
//! assert_eq!(frame.as_bytes(), b"\x1b[2JReady");
//! ```

use crate::{Command, escape};

/// Clears the active display.
///
/// This encodes ECMA-48 ED, "Erase in Display", with mode `2`, "erase the complete display".
/// qwertty emits `CSI 2 J` as `b"\x1b[2J"`. See
/// [Erase In Display](crate::docs#erase-in-display) for the protocol terms.
///
/// This affects the terminal display but does not move the cursor.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::clear());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[2J");
/// ```
#[must_use]
pub fn clear() -> Command {
    escape::csi("2", 'J')
}

/// Erases the active line.
///
/// This encodes ECMA-48 EL, "Erase in Line", with mode `2`, "erase the complete line".
/// qwertty emits `CSI 2 K` as `b"\x1b[2K"`. See
/// [Erase In Line](crate::docs#erase-in-line) for the protocol terms.
///
/// This affects the active line but does not move the cursor.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::erase_line());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[2K");
/// ```
#[must_use]
pub fn erase_line() -> Command {
    escape::csi("2", 'K')
}
