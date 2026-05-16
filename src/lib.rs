//! Encode terminal output without opening a terminal.
//!
//! qwertty is growing in small slices. The current public surface can build the bytes a terminal
//! would receive and, on Unix, open a terminal device for explicit byte output and raw-mode
//! management. It does not own async input, query routing, or session cleanup yet.
//!
//! The main types are:
//!
//! - [`Command`], a small envelope for encoded terminal command bytes.
//! - [`CommandBuffer`], an ordered byte buffer for commands, raw bytes, and text.
//! - [`ProtocolPosition`], a one-based terminal protocol coordinate.
//! - [`Terminal`], a low-level terminal device owner.
//! - [`TerminalSize`], terminal dimensions reported by the operating system.
//! - [`commands`], user-intent helpers that return [`Command`].
//!
//! # Example
//!
//! ```
//! use qwertty::{CommandBuffer, ProtocolPosition, commands};
//!
//! let mut output = CommandBuffer::new();
//! output
//!     .command(commands::screen::clear())
//!     .command(commands::cursor::move_to(ProtocolPosition::new(3, 5)))
//!     .text("Ready");
//!
//! assert_eq!(output.as_bytes(), b"\x1b[2J\x1b[3;5HReady");
//! ```
//!
//! Terminal protocol terms used by the first command helpers are introduced in the
//! [terminal control reference](crate::docs).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod command;
pub mod commands;
pub mod docs;
mod escape;
mod terminal;

pub use command::{Command, CommandBuffer, ProtocolPosition};
pub use terminal::{Error, Result, Terminal, TerminalSize};
