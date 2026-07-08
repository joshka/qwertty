//! User-facing protocol and library reference material.
//!
//! This module keeps important reference documentation on docs.rs instead of requiring readers to
//! leave the crate documentation for protocol context.

#![doc = include_str!("../docs/reference/examples.md")]
#![doc = include_str!("../docs/reference/platform-support.md")]
#![doc = include_str!("../docs/reference/release-blocking-examples.md")]
#![doc = include_str!("../docs/reference/release-checklist.md")]
#![doc = include_str!("../docs/reference/release-readiness.md")]
#![doc = include_str!("../docs/reference/terminal-control.md")]
// The two live query-helper examples from the control reference use `TokioTerminalSession`, so they
// live in a `tokio`-gated companion include instead of inline `rust` fences that a default-feature
// doctest run cannot compile. Under `--all-features` this page is included and its doctests
// compile.
#![cfg_attr(feature = "tokio", doc = include_str!("../docs/reference/terminal-control-tokio.md"))]
#![doc = include_str!("../docs/reference/terminal-device.md")]
#![doc = include_str!("../docs/reference/terminal-session.md")]
// The Tokio-only tail of the session reference (async boundary, live query helpers) lives in a
// separate include gated behind the `tokio` feature: its `rust` fences use `TokioTerminalSession`,
// which only exists with that feature, so a default-feature doctest run must not compile them. The
// runtime-neutral body above stays ungated so default builds keep those docs.
#![cfg_attr(feature = "tokio", doc = include_str!("../docs/reference/terminal-session-tokio.md"))]
#![doc = include_str!("../docs/reference/terminal-input.md")]
// Tokio-heavy page: every `rust` fence drives `TokioTerminalSession`, so the whole include is gated
// behind the `tokio` feature. A default build legitimately lacks the Tokio session, so its docs
// need not describe it; under `--all-features` the page is included and its doctests still
// compile+run.
#![cfg_attr(feature = "tokio", doc = include_str!("../docs/reference/tokio-input-ownership.md"))]
#![doc = include_str!("../docs/reference/capability-model.md")]
