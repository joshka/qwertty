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

/// Begins a synchronized-output frame.
///
/// This encodes the xterm/Contour private mode 2026 set: `CSI ? 2026 h`, emitted as
/// `b"\x1b[?2026h"`. It asks a supporting terminal to buffer subsequent output and paint it
/// atomically once [`end_synchronized_update`] closes the frame, avoiding the partial-frame flash
/// a full redraw can otherwise produce mid-paint.
///
/// **Caller contract.** This helper only builds the begin bytes; it does not know whether the
/// terminal understands mode 2026. Per R-OUT-3 and FM-V4 (codex#24543: mode-2026-adjacent bytes
/// leaking raw onto terminals that do not support them), emission of this pair must be
/// detection-gated by the caller — probe for mode 2026 support (DECRQM or an equivalent capability
/// check) before writing these bytes to a real terminal, the way `TokioTerminalSession`'s capture
/// probing does for other capabilities. This module has no session or device, so it cannot gate
/// anything itself; a later session slice owns applying that gate. Pair every `begin` with a
/// matching [`end_synchronized_update`] so a terminal never gets left mid-frame — wrap one full
/// frame per pair, never leave a begin unmatched across an error path.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::begin_synchronized_update());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?2026h");
/// ```
#[must_use]
pub fn begin_synchronized_update() -> Command {
    escape::csi("?2026", 'h')
}

/// Ends a synchronized-output frame.
///
/// This encodes the xterm/Contour private mode 2026 reset: `CSI ? 2026 l`, emitted as
/// `b"\x1b[?2026l"`. Use this as the closing pair for [`begin_synchronized_update`]; a supporting
/// terminal paints everything buffered since the matching begin as one atomic update.
///
/// Subject to the same detection-gating caller contract as [`begin_synchronized_update`] (FM-V4):
/// this helper builds bytes only and does not verify mode 2026 support itself.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::end_synchronized_update());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?2026l");
/// ```
#[must_use]
pub fn end_synchronized_update() -> Command {
    escape::csi("?2026", 'l')
}

/// Sets the vertical scroll region (DECSTBM).
///
/// This encodes ECMA-48/DEC "Set Top and Bottom Margins": `CSI top ; bottom r`, with `top` and
/// `bottom` as 1-based protocol row numbers. For example, `set_scroll_region(2, 10)` emits
/// `b"\x1b[2;10r"`, confining subsequent scrolling (including [`scroll_up`], [`scroll_down`], and
/// wrap-driven scrolling at the bottom margin) to rows 2 through 10.
///
/// **Caller contract — this primitive is not portable (FM-V2).** DECSTBM is the core
/// inline-viewport primitive ratatui's `insert_before`-shaped consumers need (R-OUT-6), but codex's
/// tui2 postmortem found it drops or duplicates scrollback lines on some hosts, and xterm.js-based
/// terminals (notably VS Code's integrated terminal) permanently drop scrollback when a scroll
/// region is set (codex#27644). This helper only builds the bytes; it has no device, session, or
/// capability model, so it cannot refuse to emit on a host known to be unsafe. Per R-OUT-6, callers
/// should gate emission of scroll-region commands on the `inline_insertion_safe` capability a later
/// session/capability slice adds (backed by the conformance matrix's per-terminal
/// scroll-region/clear semantics), rather than assuming DECSTBM is safe everywhere it is accepted
/// syntactically.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::set_scroll_region(2, 10));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[2;10r");
/// ```
#[must_use]
pub fn set_scroll_region(top: u16, bottom: u16) -> Command {
    escape::csi(format!("{top};{bottom}"), 'r')
}

/// Resets the scroll region to the full viewport (DECSTBM with no parameters).
///
/// This encodes `CSI r`, emitted as `b"\x1b[r"`. Use this as the cleanup pair for
/// [`set_scroll_region`]: an omitted `Pt`/`Pb` pair resets the top and bottom margins to the first
/// and last row of the terminal.
///
/// Subject to the same `inline_insertion_safe` gating caveat as [`set_scroll_region`] (FM-V2):
/// this helper builds bytes only.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::reset_scroll_region());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[r");
/// ```
#[must_use]
pub fn reset_scroll_region() -> Command {
    escape::csi("", 'r')
}

/// Scrolls the active scroll region up (SU).
///
/// This encodes ECMA-48 "Scroll Up": `CSI n S`, emitted as `b"\x1b[nS"`. Content within the active
/// scroll region ([`set_scroll_region`], or the whole viewport when no region is set) moves up by
/// `n` lines; `n` blank lines are introduced at the bottom of the region. `n` is written even when
/// it equals the ECMA-48 default of 1, so `scroll_up(1)` emits `b"\x1b[1S"` rather than the
/// parameter-omitted form.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::scroll_up(3));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[3S");
/// ```
#[must_use]
pub fn scroll_up(n: u16) -> Command {
    escape::csi(n.to_string(), 'S')
}

/// Scrolls the active scroll region down (SD).
///
/// This encodes ECMA-48 "Scroll Down": `CSI n T`, emitted as `b"\x1b[nT"`. Content within the
/// active scroll region ([`set_scroll_region`], or the whole viewport when no region is set) moves
/// down by `n` lines; `n` blank lines are introduced at the top of the region. `n` is written even
/// when it equals the ECMA-48 default of 1, so `scroll_down(1)` emits `b"\x1b[1T"` rather than the
/// parameter-omitted form.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::scroll_down(3));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[3T");
/// ```
#[must_use]
pub fn scroll_down(n: u16) -> Command {
    escape::csi(n.to_string(), 'T')
}

/// Inserts blank lines at the cursor (IL).
///
/// This encodes ECMA-48 "Insert Line": `CSI n L`, emitted as `b"\x1b[nL"`. `n` blank lines are
/// inserted at the cursor's line within the active scrolling area; lines at and below the cursor
/// shift down, and lines shifted past the bottom margin are discarded. `n` is written even when it
/// equals the ECMA-48 default of 1, so `insert_lines(1)` emits `b"\x1b[1L"` rather than the
/// parameter-omitted form.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::insert_lines(3));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[3L");
/// ```
#[must_use]
pub fn insert_lines(n: u16) -> Command {
    escape::csi(n.to_string(), 'L')
}

/// Deletes lines at the cursor (DL).
///
/// This encodes ECMA-48 "Delete Line": `CSI n M`, emitted as `b"\x1b[nM"`. `n` lines starting at
/// the cursor's line are deleted within the active scrolling area; lines below shift up, and blank
/// lines are introduced at the bottom margin. `n` is written even when it equals the ECMA-48
/// default of 1, so `delete_lines(1)` emits `b"\x1b[1M"` rather than the parameter-omitted form.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::screen;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(screen::delete_lines(3));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[3M");
/// ```
#[must_use]
pub fn delete_lines(n: u16) -> Command {
    escape::csi(n.to_string(), 'M')
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

    #[test]
    fn begin_synchronized_update_bytes() {
        let command = begin_synchronized_update();
        assert_bytes(&command, b"\x1b[?2026h");
        assert_round_trips_as_csi(&command, b'h');
    }

    #[test]
    fn end_synchronized_update_bytes() {
        let command = end_synchronized_update();
        assert_bytes(&command, b"\x1b[?2026l");
        assert_round_trips_as_csi(&command, b'l');
    }

    #[test]
    fn set_scroll_region_bytes() {
        let command = set_scroll_region(2, 10);
        assert_bytes(&command, b"\x1b[2;10r");
        assert_round_trips_as_csi(&command, b'r');
    }

    #[test]
    fn set_scroll_region_full_viewport() {
        // Edge case: top=1 is the ECMA-48 default but qwertty still writes it explicitly, the
        // same policy as the n=1 scroll/insert/delete helpers below.
        let command = set_scroll_region(1, 24);
        assert_bytes(&command, b"\x1b[1;24r");
        assert_round_trips_as_csi(&command, b'r');
    }

    #[test]
    fn reset_scroll_region_bytes() {
        let command = reset_scroll_region();
        assert_bytes(&command, b"\x1b[r");
        assert_round_trips_as_csi(&command, b'r');
    }

    #[test]
    fn scroll_up_bytes() {
        let command = scroll_up(3);
        assert_bytes(&command, b"\x1b[3S");
        assert_round_trips_as_csi(&command, b'S');
    }

    #[test]
    fn scroll_up_default_form() {
        // n=1 is the ECMA-48 default; qwertty still writes it explicitly rather than omitting the
        // parameter, matching the documented n=1 behavior.
        let command = scroll_up(1);
        assert_bytes(&command, b"\x1b[1S");
        assert_round_trips_as_csi(&command, b'S');
    }

    #[test]
    fn scroll_down_bytes() {
        let command = scroll_down(3);
        assert_bytes(&command, b"\x1b[3T");
        assert_round_trips_as_csi(&command, b'T');
    }

    #[test]
    fn scroll_down_default_form() {
        let command = scroll_down(1);
        assert_bytes(&command, b"\x1b[1T");
        assert_round_trips_as_csi(&command, b'T');
    }

    #[test]
    fn insert_lines_bytes() {
        let command = insert_lines(3);
        assert_bytes(&command, b"\x1b[3L");
        assert_round_trips_as_csi(&command, b'L');
    }

    #[test]
    fn insert_lines_default_form() {
        let command = insert_lines(1);
        assert_bytes(&command, b"\x1b[1L");
        assert_round_trips_as_csi(&command, b'L');
    }

    #[test]
    fn delete_lines_bytes() {
        let command = delete_lines(3);
        assert_bytes(&command, b"\x1b[3M");
        assert_round_trips_as_csi(&command, b'M');
    }

    #[test]
    fn delete_lines_default_form() {
        let command = delete_lines(1);
        assert_bytes(&command, b"\x1b[1M");
        assert_round_trips_as_csi(&command, b'M');
    }
}
