//! User-facing terminal command helpers.
//!
//! The helpers are grouped by user intent and return the shared [`Command`](crate::Command) byte
//! envelope. They do not open a terminal, mutate session state, enforce policy, or assume a
//! specific emulator.
//!
//! Use this module when application code knows the terminal action it wants, such as moving the
//! cursor or clearing the screen. The exact protocol bytes remain behind small helper functions so
//! callers do not need to remember whether a common action is ECMA-48, DEC, xterm, or another
//! terminal family.
//!
//! ```
//! use qwertty::{CommandBuffer, ProtocolPosition, commands};
//!
//! let mut frame = CommandBuffer::new();
//! frame
//!     .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))
//!     .command(commands::screen::clear())
//!     .text("Ready");
//!
//! assert_eq!(frame.as_bytes(), b"\x1b[1;1H\x1b[2JReady");
//! ```

pub mod cursor;
pub mod graphics;
pub mod osc;
pub mod screen;
pub mod style;
pub mod terminal;
