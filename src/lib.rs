//! Encode terminal output without opening a terminal.
//!
//! qwertty is growing in small slices. The current public surface can build the bytes a terminal
//! would receive and, on Unix, open a terminal device for explicit byte output, raw-mode
//! management, a small terminal session lifecycle, raw terminal input bytes, and basic terminal
//! input events with a stateful decoder for split chunks, complete CSI input syntax, and cursor
//! position and terminal status report parsing. With the optional `tokio` feature on Unix, it also
//! exposes a Tokio-backed session owner for runtime-backed reads, writes, decoded input events, and
//! explicit cleanup, including live cursor position and terminal status queries.
//!
//! The main types are:
//!
//! - [`Command`], a small envelope for encoded terminal command bytes.
//! - [`CommandBuffer`], an ordered byte buffer for commands, raw bytes, and text.
//! - [`ProtocolPosition`], a one-based terminal protocol coordinate.
//! - [`Terminal`], a low-level terminal device owner.
//! - [`TerminalSession`], an application-facing owner for raw mode, ordered output, flushing, and
//!   explicit leave cleanup.
//! - [`InputBytes`], raw terminal input bytes read through a session.
//! - [`CsiInput`], lossless syntax for complete Control Sequence Introducer input.
//! - [`CursorPositionReport`], parsed `CSI row ; column R` cursor position reports.
//! - [`CursorPositionReportMatch`], event-level cursor position report matching.
//! - [`TerminalStatusReport`], parsed `CSI 0 n` and `CSI 3 n` terminal status reports.
//! - [`InputDecoder`], stateful classification for input split across byte chunks.
//! - [`InputEvent`], basic classification for simple text, control, key, and undecoded input.
//! - [`TerminalSize`], terminal dimensions reported by the operating system.
//! - `TokioTerminalSession`, a Tokio-backed session owner available with the `tokio` feature.
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
mod input;
mod session;
mod terminal;
#[cfg(all(feature = "tokio", unix))]
mod tokio_session;

pub use command::{Command, CommandBuffer, ProtocolPosition};
pub use input::{
    ControlInput, CsiInput, CursorPositionReport, CursorPositionReportMatch, InputBytes,
    InputDecoder, InputEvent, KeyInput, TerminalStatus, TerminalStatusReport,
    TerminalStatusReportMatch,
};
pub use session::TerminalSession;
pub use terminal::{Error, Result, Terminal, TerminalSize};
#[cfg(all(feature = "tokio", unix))]
pub use tokio_session::TokioTerminalSession;
