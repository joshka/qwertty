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
//! [`Command`] values. They do not track state locally; the session that writes them owns cleanup
//! and policy around real terminal output.
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

/// Requests the current cursor position.
///
/// This encodes the ECMA-48 Device Status Report request `CSI 6 n`, emitted as `b"\x1b[6n"`.
/// Terminals commonly answer with a cursor position report in the form `CSI row ; column R`.
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a
/// response, route query responses, or filter unrelated input.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::cursor;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(cursor::request_position());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[6n");
/// ```
#[must_use]
pub fn request_position() -> Command {
    escape::csi("6", 'n')
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

/// A DECSCUSR cursor shape.
///
/// This is the shape argument for [`set_shape`], encoding DEC's "Set Cursor Style" control (`CSI
/// Ps SP q`). `Default` requests the terminal profile's own default shape (`Ps` = 0), distinct from
/// [`reset_shape`], which is the same bytes under a name that documents intent at a call site.
///
/// The enum is `#[non_exhaustive]`: DECSCUSR has no vendor extension today, but future terminal
/// families may define additional `Ps` values.
///
/// See [Cursor Shape](crate::docs#cursor-shape) for the protocol details and the restore caveat
/// (FM-L3): no single reset value matches every terminal profile's prior shape, so an application
/// that changes the cursor shape should restore its own remembered shape explicitly rather than
/// rely on one universal reset.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum CursorShape {
    /// The terminal profile's default shape (`Ps` = 0).
    Default,
    /// Blinking block (`Ps` = 1).
    BlinkingBlock,
    /// Steady block (`Ps` = 2).
    SteadyBlock,
    /// Blinking underline (`Ps` = 3).
    BlinkingUnderline,
    /// Steady underline (`Ps` = 4).
    SteadyUnderline,
    /// Blinking bar (`Ps` = 5).
    BlinkingBar,
    /// Steady bar (`Ps` = 6).
    SteadyBar,
}

impl CursorShape {
    /// Returns the DECSCUSR `Ps` parameter for this shape (0 through 6).
    #[must_use]
    const fn parameter(self) -> u8 {
        match self {
            Self::Default => 0,
            Self::BlinkingBlock => 1,
            Self::SteadyBlock => 2,
            Self::BlinkingUnderline => 3,
            Self::SteadyUnderline => 4,
            Self::BlinkingBar => 5,
            Self::SteadyBar => 6,
        }
    }
}

/// Sets the cursor shape.
///
/// This encodes DEC's "Set Cursor Style" control, DECSCUSR: `CSI Ps SP q`, where `Ps` is the
/// shape's DECSCUSR number (0 through 6, `CursorShape::Default` through `CursorShape::SteadyBar`).
/// For example, `set_shape(CursorShape::SteadyBar)` emits `b"\x1b[6 q"`.
///
/// DECSCUSR support and its default appearance vary by terminal profile. See [Cursor
/// Shape](crate::docs#cursor-shape) for the restore caveat (FM-L3): no single `Ps` value is a
/// universal reset to what the shape was before this call, because "default" means the terminal
/// profile's own default, not necessarily the shape a prior `set_shape` call changed away from.
/// Application code that changes the shape should restore its own remembered shape explicitly.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::cursor::{self, CursorShape};
///
/// let mut frame = CommandBuffer::new();
/// frame.command(cursor::set_shape(CursorShape::SteadyBar));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[6 q");
/// ```
#[must_use]
pub fn set_shape(shape: CursorShape) -> Command {
    escape::csi(format!("{} ", shape.parameter()), 'q')
}

/// Resets the cursor shape to the terminal profile's default.
///
/// This encodes `CSI 0 SP q`, emitted as `b"\x1b[0 q"` — the same bytes as
/// `set_shape(CursorShape::Default)`, offered under a name that documents restore intent at call
/// sites.
///
/// Per FM-L3 (helix#10089, libvaxis#10/#98), no single DECSCUSR value is a universal reset: `Ps` =
/// 0 asks for "the terminal profile's default," which is not guaranteed to match whatever shape
/// was active before an application changed it (the user's own profile default, a previous
/// application's leaked shape, or another value entirely). Session code that tracks the shape it
/// set should restore that specific shape explicitly instead of relying on this reset alone; see
/// [Cursor Shape](crate::docs#cursor-shape).
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::cursor;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(cursor::reset_shape());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[0 q");
/// ```
#[must_use]
pub fn reset_shape() -> Command {
    set_shape(CursorShape::Default)
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
    fn hide_bytes() {
        let command = hide();
        assert_bytes(&command, b"\x1b[?25l");
        assert_round_trips_as_csi(&command, b'l');
    }

    #[test]
    fn show_bytes() {
        let command = show();
        assert_bytes(&command, b"\x1b[?25h");
        assert_round_trips_as_csi(&command, b'h');
    }

    #[test]
    fn set_shape_bytes_cover_every_variant() {
        let cases = [
            (CursorShape::Default, b"\x1b[0 q".as_slice()),
            (CursorShape::BlinkingBlock, b"\x1b[1 q".as_slice()),
            (CursorShape::SteadyBlock, b"\x1b[2 q".as_slice()),
            (CursorShape::BlinkingUnderline, b"\x1b[3 q".as_slice()),
            (CursorShape::SteadyUnderline, b"\x1b[4 q".as_slice()),
            (CursorShape::BlinkingBar, b"\x1b[5 q".as_slice()),
            (CursorShape::SteadyBar, b"\x1b[6 q".as_slice()),
        ];
        for (shape, expected) in cases {
            let command = set_shape(shape);
            assert_bytes(&command, expected);
            assert_round_trips_as_csi(&command, b'q');
        }
    }

    #[test]
    fn reset_shape_matches_set_shape_default() {
        assert_eq!(reset_shape(), set_shape(CursorShape::Default));
        assert_bytes(&reset_shape(), b"\x1b[0 q");
    }
}
