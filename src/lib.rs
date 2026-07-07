//! Encode terminal output without opening a terminal.
//!
//! qwertty is growing in small slices. The current public surface can build the bytes a terminal
//! would receive and, on Unix, open a terminal device for explicit byte output, raw-mode
//! management, a small terminal session lifecycle, and raw terminal input bytes decoded through a
//! total, lossless syntax layer and a semantic layer that maps its tokens to typed [`Event`]
//! values, with typed cursor position and terminal status report parsing over that syntax layer.
//! With the optional `tokio` feature on Unix, it also exposes a Tokio-backed session owner that
//! drives the sans-io core (device, [`SemanticDecoder`], and query correlator) for runtime-backed
//! reads, writes, decoded [`Event`] delivery, and explicit cleanup, including live cursor position
//! and terminal status queries.
//!
//! The main types are:
//!
//! - [`Command`], a small envelope for encoded terminal command bytes.
//! - [`CommandBuffer`], an ordered byte buffer for commands, raw bytes, and text.
//! - [`ProtocolPosition`], a one-based terminal protocol coordinate.
//! - [`Terminal`], a low-level terminal device owner.
//! - [`TerminalDevice`], the substitutable device boundary session logic writes through.
//! - [`DeviceMode`], the raw or cooked mode selected through a device.
//! - `FakeDevice` and `FakeTerminal`, an in-process device pair for headless tests on Unix.
//! - [`TerminalSession`], an application-facing owner for raw mode, ordered output, flushing, and
//!   explicit leave cleanup.
//! - [`KittyKeyboardFlags`] and [`KittyKeyboardGrant`], the caller-chosen kitty keyboard
//!   progressive-enhancement request set and the granted result of the verify-after-push handshake.
//! - `RestoreHandle`, a panic-safe emergency terminal-restore handle on Unix.
//! - [`InputBytes`], raw terminal input bytes read through a session.
//! - [`CursorPositionReport`], parsed `CSI row ; column R` cursor position reports.
//! - [`TerminalStatusReport`] and [`TerminalStatus`], parsed `CSI 0 n` and `CSI 3 n` terminal
//!   status reports.
//! - [`SyntaxParser`], the total, lossless, bounded, stateful syntax tokenizer over input bytes.
//! - [`SyntaxToken`], one classified byte-span in the syntax layer (text, control, CSI, OSC, DCS,
//!   APC, PM, SOS, escape, or malformed).
//! - [`SemanticDecoder`], the semantic layer over [`SyntaxParser`] that maps tokens to typed
//!   [`Event`] values.
//! - [`Event`], a semantic input event: a [`KeyEvent`] or lossless [`SyntaxToken`] passthrough.
//! - [`KeyEvent`], a kitty-shaped key event with a [`Key`], [`Modifiers`], [`KeyEventKind`], and
//!   optional [`TextPayload`].
//! - [`Key`], [`Modifiers`], [`KeyEventKind`], and [`TextPayload`], the parts of a [`KeyEvent`].
//! - [`TerminalSize`], terminal dimensions reported by the operating system.
//! - `TokioTerminalSession`, a Tokio-backed session owner available with the `tokio` feature.
//! - [`commands`], user-intent helpers that return [`Command`].
//! - [`report`], the module home of the typed terminal reports parsed from the lossless syntax
//!   layer. [`CursorPositionReport`], [`TerminalStatusReport`], and [`TerminalStatus`] are
//!   re-exported at the crate root for convenience and also reachable as `report::` for a stable
//!   module path (the ghostty-rs encode oracle uses the module path). These are the report parsers
//!   the query correlator consumes.
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
pub(crate) mod correlate;
pub mod docs;
mod escape;
mod event;
mod input;
pub mod report;
mod session;
mod syntax;
mod terminal;
#[cfg(all(feature = "tokio", unix))]
mod tokio_session;

pub use command::{Command, CommandBuffer, ProtocolPosition};
pub use event::{Event, Key, KeyEvent, KeyEventKind, Modifiers, SemanticDecoder, TextPayload};
pub use input::InputBytes;
pub use report::{CursorPositionReport, TerminalStatus, TerminalStatusReport};
#[cfg(unix)]
pub use session::RestoreHandle;
pub use session::{KittyKeyboardFlags, KittyKeyboardGrant, TerminalSession};
pub use syntax::{
    ControlParams, ControlSequence, DEFAULT_PAYLOAD_LIMIT, EscapeSequence, Param, ParamSeparator,
    StringKind, StringSequence, StringTerminator, SyntaxParser, SyntaxToken,
};
pub use terminal::{DeviceMode, Error, Result, Terminal, TerminalDevice, TerminalSize};
#[cfg(unix)]
pub use terminal::{FakeDevice, FakeTerminal};
#[cfg(all(feature = "tokio", unix))]
pub use tokio_session::TokioTerminalSession;
