# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This changelog is maintained by hand. qwertty does not use conventional commits, so release-plz
detects versions from the public API diff (via cargo-semver-checks) and does not generate these
entries.

## [Unreleased]

### Added

- A [failure-modes reference page](https://docs.rs/qwertty/latest/qwertty/docs/failure_modes/)
  (`docs::failure_modes`): the public catalogue of the ways terminal I/O goes wrong — interleaved
  query replies and typeahead, wrong-reply cross-completion, ambiguous silence, late replies, the
  lone-Escape prefix problem, sequences split across reads, paste, restore leaks, async-over-
  blocking races, and capability guesswork — each with the mechanism qwertty uses against it.
- `Capabilities::iterm2_images`: identity-keyed iTerm2 inline-image support. The protocol has no
  support query, so the finding is inferred (never probed) from the resolved terminal identity —
  known-true under iTerm2 and WezTerm (which speaks the protocol too), honestly unknown for every
  other identity, and re-derived when an XTVERSION reply improves on the environment's identity (a
  multiplexer answering XTVERSION for itself downgrades it back to unknown). The inference is
  public as `caps::infer_iterm2_images`. Both `probe_capabilities` drivers now seed their
  env-inferred findings through one shared constructor so they cannot drift.
- Capability-gated session emits for iTerm2 inline images: `inline_iterm2_image` /
  `inline_iterm2_image_sized` on both `TerminalSession` and `TokioTerminalSession` refuse with the
  typed `Error::CapabilityUnverified` — writing nothing — unless the finding affirms support
  (R-CAP-4). No policy gate: the bytes are inline and open no resource, unlike the kitty
  resource-naming transmissions. The `iterm2_inline_image.rs` example now runs this full
  identity-gated flow end to end.

## [0.1.4] - 2026-07-13

### Added

- `TerminalSession::probe_capabilities`: the DA1-fenced capability probe bundle (XTVERSION, kitty
  keyboard flags, OSC 10/11, and the DEC private mode queries for synchronized output/grapheme
  clustering/in-band resize/bracketed paste), blocking with no async runtime. The synchronous
  mirror of `TokioTerminalSession::probe_capabilities`, sharing its bundle contents and
  reply-to-field mapping so the two drivers can never drift apart.
- Dumb terminals are now detected and never probed (R-QRY-5). Both `probe_capabilities` drivers
  check the environment for `TERM=dumb` and the Linux console (`TERM=linux`) before writing a
  single byte — a terminal that does not parse escape sequences would echo the probe as garbage —
  and return immediately with every probe-backed finding honestly `Evidence::Unknown` and the
  reason recorded on the new `Capabilities::probe_skip` field (`Some(ProbeSkip::TermDumb)` /
  `Some(ProbeSkip::LinuxConsole)`; `None` when the probe actually ran). The guard itself is public
  as `caps::probe_skip_from_env` for callers composing their own query flow. Previously this was
  documented as a caller duty.
- The rest of the kitty graphics protocol surface on top of the 0.1.3 encoders: `query_support`
  (the spec's canonical `a=q` probe), id-carrying `transmit` with raw-format dimensions
  (`ImageSize`), `place_with` placement options (`Placement`: placement id, column/row scaling,
  z-index), the data-freeing `delete_image_and_data` / `delete_all_images_and_data` /
  `delete_placement` forms, the resource-naming `transmit_file` / `transmit_temp_file` /
  `transmit_shared_memory` builders, and protocol-correct chunking of every payload past the
  4096-byte bound (previously oversized payloads were emitted as one over-long escape). Terminal
  acknowledgements decode as the new `report::KittyGraphicsReport`, matched by the query
  correlator on the echoed image id.
- Graphics capability probing: both `probe_capabilities` drivers now send the kitty graphics
  `a=q` query plus the XTWINOPS text-area (`CSI 14 t`) and cell-size (`CSI 16 t`) pixel-geometry
  queries behind the same DA1 fence, populating `Capabilities::kitty_graphics`,
  `text_area_pixels`, and `cell_size`. Zero-dimension geometry answers stay unknown — never a
  fabricated default. New report parsers `TextAreaPixelsReport` and `CellSizeReport`; new command
  builders `commands::terminal::request_text_area_pixels` / `request_cell_size`.
- Policy-gated session emits for the resource-naming kitty transmission modes
  (`TerminalSession::transmit_kitty_file` / `transmit_kitty_temp_file` /
  `transmit_kitty_shared_memory`): the escape names a file or IPC object the terminal itself
  opens — a local-file-read primitive — so the emit consults the existing file-transfer policy
  gate (denied under the default `Policy::restricted()`) and requires a known-true capability
  finding. A refused capability check is the new typed `Error::CapabilityUnverified`; inline
  (direct) transmission carries app-owned bytes and is capability-gated only. Placed images are
  app-owned content: not ledgered, not auto-cleared, ids caller-owned.
- The graphics reference page (`docs::graphics`) now covers the capability-provenance table, the
  resource-naming policy split, and the pixel-geometry honesty rule; the `kitty_graphics.rs`
  example runs the full gated flow (probe, transmit, place, decode the acknowledgement, delete).

## [0.1.3] - 2026-07-12

### Added

- iTerm2 inline-image command encoders under `commands::graphics::iterm2`: `inline_image` and
  `inline_image_sized` (with a `Dimension` of cells, pixels, percent, or auto) build the OSC 1337
  `File` inline form (also spoken by WezTerm). Encode-only and inline-bytes-only, like the kitty
  encoders — no file path, no capability check, no policy; iTerm2 support is identity-keyed at the
  session layer, a later slice.
- `width_of(&str, &Capabilities) -> usize`, terminal-aware string column-width measurement. Sums a
  static `unicode-width` baseline per grapheme cluster, overriding the clusters real terminals render
  off-baseline (ZWJ emoji, skin-tone modifiers, regional flags, VS16) from a per-terminal deviation
  table measured from live conformance and keyed on the terminal's identity and observed mode-2027
  state — never enabling 2027, only observing it. Unknown terminals fall back to the baseline. Adds
  the `unicode-width` and `unicode-segmentation` dependencies. See the new string-width reference page.
- Kitty graphics protocol command encoders under `commands::graphics::kitty`: `transmit_and_display`
  (send an image and show it), `place` (show a transmitted image by id), `delete_image`, and
  `delete_all_images`, plus a `Format` (`Rgb`/`Rgba`/`Png`) for the pixel format. Like every
  `commands::` helper these build raw bytes only — the inline transmission form, with no capability
  check and no policy; support gating and the file/temp/shared-memory transmission policy are
  session-layer concerns for a later slice. See the new graphics reference page.
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

[Unreleased]: https://github.com/joshka/qwertty/compare/qwertty-v0.1.4...HEAD
[0.1.4]: https://github.com/joshka/qwertty/releases/tag/qwertty-v0.1.4
[0.1.3]: https://github.com/joshka/qwertty/releases/tag/qwertty-v0.1.3
[0.1.2]: https://github.com/joshka/qwertty/releases/tag/qwertty-v0.1.2
[0.1.1]: https://github.com/joshka/qwertty/releases/tag/qwertty-v0.1.1
[0.1.0]: https://crates.io/crates/qwertty/0.1.0
