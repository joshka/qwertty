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

/// Enters the alternate screen buffer.
///
/// This encodes the xterm private mode 1049 set: `CSI ? 1049 h`, emitted as `b"\x1b[?1049h"`. It
/// switches to the alternate screen buffer and saves the cursor position, the widely supported
/// combination for full-screen applications that want their output confined to a buffer the
/// terminal discards on exit, leaving the caller's prior scrollback content untouched.
///
/// This helper only builds the enter bytes. It does not clear the alternate buffer, track whether
/// the terminal is currently on the alternate screen, or arrange cleanup. Session code pairs this
/// with an explicit clear and with [`leave_alternate_screen`] as a ledger entry — see
/// [Alternate Screen](crate::docs#alternate-screen) for why the explicit clear after entry
/// matters.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::enter_alternate_screen());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?1049h");
/// ```
#[must_use]
pub fn enter_alternate_screen() -> Command {
    escape::csi("?1049", 'h')
}

/// Leaves the alternate screen buffer.
///
/// This encodes the xterm private mode 1049 reset: `CSI ? 1049 l`, emitted as `b"\x1b[?1049l"`. It
/// switches back to the primary screen buffer and restores the cursor position saved by
/// [`enter_alternate_screen`].
///
/// Use this as the cleanup pair for [`enter_alternate_screen`].
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::leave_alternate_screen());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?1049l");
/// ```
#[must_use]
pub fn leave_alternate_screen() -> Command {
    escape::csi("?1049", 'l')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SyntaxParser, SyntaxToken};

    /// Encodes `command` and asserts the bytes match exactly.
    fn assert_bytes(command: &Command, expected: &[u8]) {
        let mut bytes = Vec::new();
        command.encode(&mut bytes);
        assert_eq!(bytes, expected);
    }

    /// Asserts that `command`'s bytes parse back through `SyntaxParser` as exactly one CSI token
    /// with the given final byte, proving the emitted bytes are well-formed.
    fn assert_round_trips_as_csi(command: &Command, final_byte: u8) {
        let mut bytes = Vec::new();
        command.encode(&mut bytes);

        let mut parser = SyntaxParser::new();
        let mut tokens = parser.feed(&bytes);
        tokens.extend(parser.finish());

        assert_eq!(tokens.len(), 1, "expected exactly one token from {bytes:?}");
        let SyntaxToken::Csi(csi) = &tokens[0] else {
            panic!("expected a CSI token from {bytes:?}, got {:?}", tokens[0]);
        };
        assert_eq!(csi.params().final_byte(), final_byte);
        assert_eq!(csi.as_bytes(), bytes.as_slice());
    }

    #[test]
    fn clear_bytes() {
        let command = clear();
        assert_bytes(&command, b"\x1b[2J");
        assert_round_trips_as_csi(&command, b'J');
    }

    #[test]
    fn erase_line_bytes() {
        let command = erase_line();
        assert_bytes(&command, b"\x1b[2K");
        assert_round_trips_as_csi(&command, b'K');
    }

    #[test]
    fn enter_alternate_screen_bytes() {
        let command = enter_alternate_screen();
        assert_bytes(&command, b"\x1b[?1049h");
        assert_round_trips_as_csi(&command, b'h');
    }

    #[test]
    fn leave_alternate_screen_bytes() {
        let command = leave_alternate_screen();
        assert_bytes(&command, b"\x1b[?1049l");
        assert_round_trips_as_csi(&command, b'l');
    }
}
