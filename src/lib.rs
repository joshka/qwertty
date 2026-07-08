//! Own the terminal on Unix, with explicit ownership, ordered output, and race-free queries.
//!
//! qwertty is a library for building terminal applications. It takes ownership of the terminal on
//! your behalf — raw mode, ordered output, and guaranteed restoration — and gives you typed layers
//! over the raw byte stream in both directions: you build output as typed [`Command`] values, a
//! session writes them in call order and restores the terminal on exit (including from a panic),
//! input decodes losslessly into typed [`Event`] values, and terminal queries such as the cursor
//! position are matched to their replies without mistaking a keystroke for an answer.
//!
//! qwertty is async-first. A runtime-neutral, side-effect-free core — encoding, the input decoder,
#![cfg_attr(
    feature = "tokio",
    doc = "and the query correlator — is driven either by the asynchronous [`TokioTerminalSession`] (the"
)]
#![cfg_attr(
    not(feature = "tokio"),
    doc = "and the query correlator — is driven either by the asynchronous `TokioTerminalSession` (the"
)]
//! `tokio` feature) or by the blocking [`TerminalSession`], so the same decode and query logic
//! backs both. Live terminal ownership is Unix-only; the encode and decode layers compile
//! everywhere.
//!
//! # The model
//!
//! Work flows through a few layers, each a typed view of the byte stream:
//!
//! - **Encode.** The [`commands`] modules build [`Command`] byte sequences (SGR styling, cursor and
//!   screen control, OSC, terminal mode changes); a [`CommandBuffer`] collects them in order.
//! - **Own.** A session enters raw mode through a mode ledger, writes output in call order, and
//!   undoes exactly what it enabled on `leave`, on drop, or from a panic hook (the unix-only
#![cfg_attr(
    feature = "tokio",
    doc = "  [`RestoreHandle`]). [`TerminalSession`] is synchronous; [`TokioTerminalSession`] is its async"
)]
#![cfg_attr(
    not(feature = "tokio"),
    doc = "  [`RestoreHandle`]). [`TerminalSession`] is synchronous; `TokioTerminalSession` is its async"
)]
//!   counterpart.
//! - **Decode.** A [`SyntaxParser`] turns input bytes into lossless [`SyntaxToken`] spans, and a
//!   [`SemanticDecoder`] maps those to typed [`Event`] values — [`KeyEvent`], mouse, focus, paste,
//!   resize — passing anything it does not recognize through unchanged.
//! - **Query.** A session writes a request and a correlator pairs the reply to it, surviving
//!   interleaved typeahead, so the [`report`] parsers (such as [`CursorPositionReport`]) return the
//!   answer to the right question. Capability probing and the security [`Policy`] build on this
//!   path.
//!
//! # Feature flags
//!
//! - **default** (no features): encoding, the synchronous [`TerminalSession`] (raw mode, ordered
//!   output, blocking cursor-position and terminal-status queries), the input decoders, and the
//!   [`report`] parsers.
#![cfg_attr(
    feature = "tokio",
    doc = "- **`tokio`**: adds [`TokioTerminalSession`], which drives the same sans-io core over Tokio"
)]
#![cfg_attr(
    not(feature = "tokio"),
    doc = "- **`tokio`**: adds `TokioTerminalSession`, which drives the same sans-io core over Tokio"
)]
//!   readiness — decoded [`Event`] delivery, live queries, capability probing, suspend/resume,
//!   `$EDITOR` handoff, and signal and resize streams.
//!
//! # Where to start
//!
//! - Build output with [`Command`], [`CommandBuffer`], and the [`commands`] modules.
#![cfg_attr(
    feature = "tokio",
    doc = "- Own the terminal with [`TerminalSession`] (or [`TokioTerminalSession`] under the `tokio`"
)]
#![cfg_attr(
    not(feature = "tokio"),
    doc = "- Own the terminal with [`TerminalSession`] (or `TokioTerminalSession` under the `tokio`"
)]
//!   feature); both guarantee cleanup.
//! - Read input through the [`SemanticDecoder`] and the [`Event`] / [`KeyEvent`] vocabulary, with
//!   [`SyntaxParser`] and [`SyntaxToken`] as the lossless layer beneath.
//! - Query and gate with the [`report`] parsers, the capability model in [`caps`], and the
//!   [`Policy`] / [`PolicyGate`] security gate.
//! - Test headlessly by driving a session over the [`TerminalDevice`] boundary (`FakeDevice` on
//!   Unix), no real terminal required.
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
//! That builds bytes in memory without opening a terminal. Opening a live terminal is a session;
//! the `examples/` directory has runnable programs for that — `session_status`,
//! `tokio_terminal_queries`, and `panic_safe_restore` among them. Terminal protocol terms used by
//! the command helpers are introduced in the [terminal control
//! reference](crate::docs::terminal_control).
#![cfg_attr(feature = "tokio", doc = "# The `tokio` feature")]
#![cfg_attr(feature = "tokio", doc = "")]
#![cfg_attr(
    feature = "tokio",
    doc = "With `tokio` enabled, [`TokioTerminalSession`] is the async session owner that drives the same"
)]
#![cfg_attr(
    feature = "tokio",
    doc = "sans-io core as the synchronous [`TerminalSession`]: it delivers decoded [`Event`] values,"
)]
#![cfg_attr(
    feature = "tokio",
    doc = "answers live queries, probes terminal capabilities, and adds [`ResizeStream`] and [`SignalStream`]"
)]
#![cfg_attr(
    feature = "tokio",
    doc = "for `SIGWINCH` and job-control signals, plus [`TerminalAcquisition`] reporting how the"
)]
#![cfg_attr(feature = "tokio", doc = "controlling terminal was reached.")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// docs.rs builds with `--cfg docsrs` (see `[package.metadata.docs.rs]` in Cargo.toml) and enables
// the nightly-only `doc_cfg` feature there so gated items (for example `TokioTerminalSession` under
// the `tokio` feature) show an automatic "Available on feature tokio only" badge. `doc_cfg` covers
// both explicit `#[doc(cfg(..))]` and automatic cfg inference (the older `doc_auto_cfg` feature was
// merged into it). Gated on `docsrs` so this never affects a normal stable build: `doc_cfg` is
// unstable, and enabling a nightly-only feature unconditionally would break `cargo doc`/`cargo
// build` on stable.
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod caps;
mod command;
pub mod commands;
// The correlator is sans-io core, deliberately not feature-gated: design 04 keeps it
// runtime-independent so both consumers can drive it: the Tokio session and the synchronous,
// no-Tokio query driver on `TerminalSession` (review-02 §2). Both are Unix-gated, so on Unix the
// correlator now always has a consumer regardless of the `tokio` feature. It is dead only on
// non-Unix targets, where neither driver exists (no live terminal, no `poll` readiness seam).
#[cfg_attr(
    not(unix),
    expect(
        dead_code,
        reason = "sans-io correlator (design 04); its drivers (Tokio + sync query) are Unix-only"
    )
)]
pub(crate) mod correlate;
pub mod docs;
mod escape;
pub mod event;
mod input;
pub mod policy;
pub mod report;
mod session;
mod syntax;
mod terminal;
#[cfg(all(feature = "tokio", unix))]
mod tokio_session;

pub use caps::{
    Capabilities, DeviceAttributes, Evidence, Finding, Multiplexer, Rgb, TerminalIdentity,
    TerminalProgram,
};
pub use command::{Command, CommandBuffer, ProtocolPosition};
pub use commands::terminal::MouseMode;
pub use event::{
    Event, FocusEvent, FocusState, Key, KeyEvent, KeyEventKind, Modifiers, MouseButton, MouseEvent,
    MouseEventKind, PasteEvent, ResizeEvent, ScrollDirection, SemanticDecoder, TextPayload,
};
pub use input::InputBytes;
pub use policy::{Policy, PolicyGate};
pub use report::{
    CursorPositionReport, DecPrivateModeReport, DecPrivateModeState, OscColorKind, OscColorReport,
    TerminalStatus, TerminalStatusReport, XtVersionReport,
};
#[cfg(unix)]
pub use session::RestoreHandle;
pub use session::{KittyKeyboardFlags, KittyKeyboardGrant, TerminalSession};
pub use syntax::{
    ControlParams, ControlSequence, DEFAULT_PAYLOAD_LIMIT, EscapeSequence, Param, ParamSeparator,
    PasteSequence, StringKind, StringSequence, StringTerminator, SyntaxParser, SyntaxToken,
};
pub use terminal::{DeviceMode, Error, PixelSize, Result, Terminal, TerminalDevice, TerminalSize};
#[cfg(unix)]
pub use terminal::{FakeDevice, FakeTerminal};
#[cfg(all(feature = "tokio", unix))]
pub use tokio_session::{
    ResizeStream, SignalStream, TerminalAcquisition, TerminalSignal, TokioTerminalSession,
};
