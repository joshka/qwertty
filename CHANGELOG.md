# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This changelog is maintained by hand. qwertty does not use conventional commits, so release-plz
detects versions from the public API diff (via cargo-semver-checks) and does not generate these
entries.

## [Unreleased]

## [0.1.3] - 2026-07-12

### Added

- Windows console support. `Terminal`, `TerminalSession`, and (behind the `tokio` feature)
  `TokioTerminalSession` now own a live Windows console, sharing the Unix decoder, command encoders,
  query correlator, and capability model over a VT-based backend (Windows 10 1809+; no legacy-console
  path). The async session reads through a cancellation-safe worker thread that waits on the console
  input handle. Resize arrives in-band as `Event::Resize`; `RestoreHandle` restores console modes and
  the output codepage from a panic hook; `signals()` reports console Ctrl events; `run_detached`
  hands the console to a child. `suspend`/`resume` and `resize_stream` return `Error::Unsupported` on
  Windows (no job control; resize is in-band). See the platform support reference.
- Decode support for win32-input-mode (`CSI … _`) key sequences, including key-release events,
  positional modifiers, and surrogate pairs. Enabling the mode stays a policy-gated opt-in.
- A keybinding-portability reference page documenting the legacy key collisions and the
  kitty/win32-input enhancement ladder.
- A generated conformance reference (the "caniuse + MDN for terminals" view): a compact support
  summary is now part of the crate docs (the `docs::conformance` page), backed by a committed tree
  under `docs/reference/generated/` — the full support matrix plus a page per sequence family with
  citations, fixtures, and the per-target conformance verdicts. It is rendered by
  `qdb generate reference` from the live-capture results and CI-freshness-checked.

### Changed

- The `unsafe_code` lint moved from `forbid` to `deny` so the `#[cfg(windows)]` console FFI modules
  can opt in; the Unix and platform-neutral layers remain free of `unsafe` (ADR 0021).
- Raised dependency version floors to what the code actually requires: `tokio` ≥ 1.37
  (`AsyncFd::try_new`) and `rustix` ≥ 1.1 (`Pid::as_raw_pid`). The CI minimal-versions check now
  builds against these floors as a required gate. Consumers already resolving newer versions are
  unaffected.

## [0.1.2] - 2026-07-12

Maintenance release — internal CI, test, and packaging fixes only. No library code or public API
changes since 0.1.1.

### Fixed

- Corrected the CHANGELOG version links to point at the real `qwertty-v*` release tags.
- Restored CI to green across all platforms: gated two Unix-only `ModeLedger` unit tests so the
  Windows build compiles, pinned the fuzz job to the host target, fixed a pty-teardown race in the
  one-shot residue integration test, and skipped the Unix-only doctests on the Windows job.

## [0.1.1] - 2026-07-10

Documentation-only patch release; no library code or public API changes.

### Documentation

- Link the Tokio session types (`TokioTerminalSession`, `ResizeStream`, `SignalStream`,
  `TerminalAcquisition`) from the crate-root introduction so they render as links on docs.rs rather
  than as unlinked code.
- Add a "Why qwertty" reference page (`qwertty::docs::why_qwertty`) comparing qwertty to crossterm,
  termwiz, termion, and termina, and stating where it deliberately does less.

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

[Unreleased]: https://github.com/joshka/qwertty/compare/qwertty-v0.1.3...HEAD
[0.1.3]: https://github.com/joshka/qwertty/releases/tag/qwertty-v0.1.3
[0.1.2]: https://github.com/joshka/qwertty/releases/tag/qwertty-v0.1.2
[0.1.1]: https://github.com/joshka/qwertty/releases/tag/qwertty-v0.1.1
[0.1.0]: https://crates.io/crates/qwertty/0.1.0
