# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This changelog is maintained by hand. qwertty does not use conventional commits, so release-plz
detects versions from the public API diff (via cargo-semver-checks) and does not generate these
entries.

## [Unreleased]

## [0.1.0] - 2026-07-07

Initial release of qwertty, a Unix-first Rust library for building terminal applications that need
explicit terminal ownership, ordered output, input handling, and policy-aware terminal features.

### Added

- Runtime-neutral command encoding and protocol parsing in memory: `CommandBuffer`, the `commands`
  vocabulary, and cursor-position / terminal-status report parsing.
- Synchronous, re-entrant terminal session (`TerminalSession`) over a Unix terminal device layer,
  with a mode ledger that restores every entered mode on `leave` and a panic-safe `RestoreHandle`
  that restores cooked mode from a panic hook.
- Security policy (`Policy`, `PolicyGate`) gating clipboard, notification, file-transfer, and mux
  passthrough features behind `restricted`, `interactive`, and `trusted` presets, with a gated
  `set_clipboard` on both sessions and a typed `Error::PolicyDenied`.
- Capability model (`Capabilities`) reporting synchronized output, grapheme clustering, in-band
  resize, bracketed paste, kitty keyboard flags, terminal identity, and env-inferred hyperlink and
  truecolor support, each carrying evidence of how it was learned (probed, inferred, or unknown).
- Synchronous, no-async-runtime blocking terminal queries
  (`TerminalSession::request_cursor_position` / `request_terminal_status`) and the Tokio async query
  path, both driven by the same sans-io correlator with identical timeout, cancellation,
  preserved-input, wrong-report, and unmatched-input contracts.
- Optional Tokio-backed async session (`TokioTerminalSession`) behind the `tokio` feature on Unix,
  including decoded `Event` delivery, mouse/focus/paste/resize reporting modes, kitty keyboard
  verify-after-push, capability probing, suspend/resume around `SIGTSTP`/`SIGCONT`, `$EDITOR`-style
  handoff (`run_detached`), a typed terminal signals stream (`SignalStream`), a `SIGWINCH` resize
  fallback stream (`ResizeStream`), lone-Escape flush timing control, and terminal-acquisition
  observability.
- Total lossless input syntax layer and a semantic decoder that maps input to typed `Event` values,
  classifying UTF-8 text/control/key input across chunks and preserving complete CSI/OSC/DCS syntax.
- A curated, machine-validated sequence database (`db/`) of 375 terminal control sequences across 16
  protocol families (ECMA-48, DEC, xterm, kitty, iTerm2, OSC, and vendor DCS), each with citations
  and byte-exact fixtures, plus the `qdb` tool that validates the database and renders a live-capture
  conformance matrix (`db/caniuse.md`) from tmux and betamax (libghostty) captures.
- Checked-in examples and docs.rs reference pages (`qwertty::docs`) for the public workflows above.
- Dual `MIT OR Apache-2.0` licensing.

[Unreleased]: https://github.com/joshka/qwertty/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/joshka/qwertty/releases/tag/v0.1.0
