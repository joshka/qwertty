//! Reference documentation. Start with a topic:
//!
//! - [Why qwertty](why_qwertty) — what it adds over crossterm, termwiz, termion, and termina, and
//!   where it deliberately does less.
//! - [Terminal control](terminal_control) — building command bytes: cursor, screen, style, and OSC
//!   helpers, plus the live query helpers under the `tokio` feature.
//! - [Terminal device](terminal_device) — the low-level owner of a live terminal: raw/cooked mode,
//!   size, and byte output.
//! - [Terminal session](terminal_session) — the application-facing owner above the device: output
//!   ordering, input modes, security policy, and the alternate-screen/cursor lifecycle, plus the
//!   Tokio async session under the `tokio` feature.
//! - [Terminal input](terminal_input) — decoding raw bytes into syntax tokens and typed key, mouse,
//!   focus, paste, resize, and report events.
#![cfg_attr(
    feature = "tokio",
    doc = "//! - [Tokio input ownership](tokio_input_ownership) — the `tokio`-feature guide to \
           owning terminal reads, live queries, cancellation, and handoff with \
           `TokioTerminalSession`."
)]
#![cfg_attr(
    feature = "tokio",
    doc = "//! - [The async model](async_model) — how the sans-io core, the query correlator, and \
           Tokio readiness make terminal queries race-free by construction."
)]
//! - [Capabilities](capabilities) — the `Finding`/`Evidence` model, terminal identity, and
//!   environment-heuristic inference behind capability probing.
//! - [String width](string_width) — `width_of`, the terminal-aware column-width function: a
//!   `unicode-width` baseline plus a measured per-terminal deviation table for the hard clusters.
//! - [Platform support](platform) — the Unix and Windows terminal backends, where they differ, and
//!   the `Error::Unsupported` boundary on other targets.
//! - [Conformance](conformance) — the live-capture support summary: which control sequences real
//!   terminals actually answer, from the generated "caniuse for terminals" reference.
//! - [Examples](examples) — the durable index of runnable examples shipped with the crate.
//!
//! Concept guides — how a specific terminal feature works:
//!
//! - [Alternate screen](alternate_screen) — the two screen buffers and switching between them.
//! - [Mouse modes](mouse_modes) — the tracking-mode ladder and the SGR coordinate encoding.
//! - [Bracketed paste](bracketed_paste) — telling pasted input apart from typed input.
//! - [Kitty keyboard](kitty_keyboard) — progressive-enhancement key reporting and
//!   verify-after-push.
//! - [Graphics](graphics) — inline images: the protocol landscape, probe-based capability, the
//!   resource-naming policy split, pixel-geometry honesty, and app-owned image lifecycle.
//! - [Keybinding portability](keybinding_portability) — which key combinations a terminal can tell
//!   apart, the legacy collisions, and the kitty/win32-input enhancement ladder.

#[doc = include_str!("../docs/reference/why-qwertty.md")]
pub mod why_qwertty {}

#[doc = include_str!("../docs/reference/terminal-control.md")]
// The two live query-helper examples from the control reference use `TokioTerminalSession`, so they
// live in a `tokio`-gated companion include instead of inline `rust` fences that a default-feature
// doctest run cannot compile. Under `--all-features` this page is appended and its doctests
// compile.
#[cfg_attr(feature = "tokio", doc = include_str!("../docs/reference/terminal-control-tokio.md"))]
pub mod terminal_control {}

#[doc = include_str!("../docs/reference/terminal-device.md")]
pub mod terminal_device {}

#[doc = include_str!("../docs/reference/terminal-session.md")]
// The Tokio-only tail of the session reference (async boundary, live query helpers) lives in a
// separate include gated behind the `tokio` feature: its `rust` fences use `TokioTerminalSession`,
// which only exists with that feature, so a default-feature doctest run must not compile them. The
// runtime-neutral body above stays ungated so default builds keep those docs.
#[cfg_attr(feature = "tokio", doc = include_str!("../docs/reference/terminal-session-tokio.md"))]
pub mod terminal_session {}

#[doc = include_str!("../docs/reference/terminal-input.md")]
pub mod terminal_input {}

// Tokio-heavy page: every `rust` fence drives `TokioTerminalSession`, so the whole module is gated
// behind the `tokio` feature. A default build legitimately lacks the Tokio session, so its docs
// need not exist there; under `--all-features` the module is included and its doctests still
// compile+run.
#[cfg(feature = "tokio")]
#[doc = include_str!("../docs/reference/tokio-input-ownership.md")]
pub mod tokio_input_ownership {}

// The async model page explains the sans-io core, the query correlator, and Tokio-readiness
// driving; its links reference the async types, so the whole module is gated behind the `tokio`
// feature.
#[cfg(feature = "tokio")]
#[doc = include_str!("../docs/reference/async-model.md")]
pub mod async_model {}

#[doc = include_str!("../docs/reference/capability-model.md")]
pub mod capabilities {}

#[doc = include_str!("../docs/reference/string-width.md")]
pub mod string_width {}

#[doc = include_str!("../docs/reference/platform-support.md")]
pub mod platform {}

// The compact conformance summary is generated by `qdb generate reference` from the sequence
// database and the live-capture results (committed under docs/reference/generated/, since docs.rs
// cannot run qdb). Only this summary page is embedded in the crate docs; the full per-sequence
// matrix and per-family pages are repo-viewed. The relative links in the page (matrix.md, family
// pages) resolve on GitHub, not on docs.rs — they point at the committed tree.
#[doc = include_str!("../docs/reference/generated/summary.md")]
pub mod conformance {}

#[doc = include_str!("../docs/reference/examples.md")]
pub mod examples {}

#[doc = include_str!("../docs/reference/alternate-screen.md")]
pub mod alternate_screen {}

#[doc = include_str!("../docs/reference/mouse-modes.md")]
pub mod mouse_modes {}

#[doc = include_str!("../docs/reference/bracketed-paste.md")]
pub mod bracketed_paste {}

#[doc = include_str!("../docs/reference/kitty-keyboard.md")]
pub mod kitty_keyboard {}

#[doc = include_str!("../docs/reference/graphics.md")]
pub mod graphics {}

#[doc = include_str!("../docs/reference/keybinding-portability.md")]
pub mod keybinding_portability {}
