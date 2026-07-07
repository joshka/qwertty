//! Terminal-level command helpers.
//!
//! These helpers encode controls that ask about or affect the terminal as a whole instead of a
//! cursor, screen, or text attribute subdomain.

use crate::{Command, KittyKeyboardFlags, escape};

/// Requests terminal status.
///
/// This encodes the ECMA-48 Device Status Report request `CSI 5 n`, emitted as `b"\x1b[5n"`.
/// Terminals commonly answer with `CSI 0 n` for ready or `CSI 3 n` for malfunction.
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a
/// response, route query responses, or filter unrelated input.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::request_status());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[5n");
/// ```
#[must_use]
pub fn request_status() -> Command {
    escape::csi("5", 'n')
}

/// Pushes kitty keyboard progressive-enhancement flags onto the terminal's flags stack.
///
/// This encodes `CSI > flags u`, which asks the terminal to turn on the requested reporting
/// behaviours and remembers the previous set on a stack so it can be restored. A terminal enables
/// only the subset it supports, so a caller should follow this with [`query_kitty_keyboard_flags`]
/// to learn what was granted (verify-after-push, design 06). Pushing the empty set emits
/// `CSI > 0 u`.
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a
/// response, or record ledger state.
///
/// # Example
///
/// ```
/// use qwertty::commands::terminal;
/// use qwertty::{CommandBuffer, KittyKeyboardFlags};
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::push_kitty_keyboard_flags(
///     KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES,
/// ));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[>1u");
/// ```
#[must_use]
pub fn push_kitty_keyboard_flags(flags: KittyKeyboardFlags) -> Command {
    escape::csi(format!(">{}", flags.bits()), 'u')
}

/// Pops one entry off the terminal's kitty keyboard flags stack.
///
/// This encodes `CSI < 1 u`, restoring the flags in effect before the matching
/// [`push_kitty_keyboard_flags`]. It is the exact undo of a single push and is what the session
/// ledger replays on `leave`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::pop_kitty_keyboard_flags());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[<1u");
/// ```
#[must_use]
pub fn pop_kitty_keyboard_flags() -> Command {
    escape::csi("<1", 'u')
}

/// Queries the terminal's current kitty keyboard flags.
///
/// This encodes `CSI ? u`. The terminal answers with `CSI ? flags u`, the currently active
/// progressive-enhancement flags — the *granted* set after a push. This is the query half of
/// verify-after-push (design 06).
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::query_kitty_keyboard_flags());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?u");
/// ```
#[must_use]
pub fn query_kitty_keyboard_flags() -> Command {
    escape::csi("?", 'u')
}
