//! Terminal-level command helpers.
//!
//! These helpers encode controls that ask about or affect the terminal as a whole instead of a
//! cursor, screen, or text attribute subdomain.

use crate::{Command, escape};

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
