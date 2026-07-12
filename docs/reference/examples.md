# Checked-In Examples

qwertty keeps small runnable examples in `examples/` to prove public behavior with real code.

This page is the durable index for those examples. Use it when you want to find the right starting
point without scanning the repository tree.

## Command Encoding And Basic Sessions

- `build_status_line.rs`: build ordered output bytes with `CommandBuffer`, cursor movement, and
  text writes without opening a terminal.
- `styled_text.rs`: build ordered SGR styling bytes with `CommandBuffer` — bold, named and
  truecolor colors, an underline substyle, and underline color — then reset with
  `commands::style::reset_all`, all without opening a terminal.
- `osc_families.rs`: build ordered OSC command bytes with `CommandBuffer` — a sanitized window
  title (`commands::osc::set_title`, stripping an embedded bidi-override character), an OSC 8
  hyperlink open/close pair, and an OSC 52 clipboard write (`commands::osc::set_clipboard`,
  documented as an exfiltration surface a session must policy-gate) — all without opening a
  terminal.
- `scroll_region.rs`: build a synchronized-output frame (`commands::screen::begin_synchronized_update`/
  `end_synchronized_update`, mode 2026) around a scroll-region insert-and-scroll sequence
  (`set_scroll_region`, `insert_lines`, `scroll_up`, `reset_scroll_region`, DECSTBM/IL/SU) with
  `CommandBuffer`, all without opening a terminal — and documents why neither mode 2026 nor DECSTBM
  emission is gated at this layer.
- `kitty_graphics.rs`: build the kitty graphics protocol command bytes
  (`commands::graphics::kitty::transmit_and_display`, `place`, `delete_image`, `delete_all_images`)
  and print them escaped — encode-only, without opening a terminal, with capability and transmission
  policy documented as session-layer obligations above the encode helpers.
- `iterm2_inline_image.rs`: build the iTerm2 inline-image command bytes
  (`commands::graphics::iterm2::inline_image` and `inline_image_sized` with a `Dimension`) and print
  them escaped — the OSC 1337 `File` inline form, encode-only, without opening a terminal.
- `session_status.rs`: open a synchronous `TerminalSession`, write ordered output, flush
  explicitly, and leave cleanly.
- `raw_mode.rs`: open the current terminal, enter raw mode through session ownership, and restore
  cooked mode on leave.
- `alternate_screen.rs`: enter the alternate screen (`enter_alternate_screen`, `CSI ? 1049 h` plus
  an explicit clear) and hide the cursor (`hide_cursor`), write a frame, then `leave` to restore the
  primary screen and show the cursor again through the mode ledger.
- `panic_safe_restore.rs`: install a panic hook with `RestoreHandle` so a panic restores the
  terminal before the backtrace prints.
- `session_cycles.rs`: cycle the re-entrant session enter/leave lifecycle headless, the way a
  line editor hands the terminal back between prompts.
- `read_input_bytes.rs`: read raw terminal bytes through the synchronous session boundary.
- `clipboard_policy.rs`: gate an OSC 52 clipboard write behind the session security policy
  (`TerminalSession::set_clipboard`) headless over a `FakeDevice` — a `trusted()` policy allows the
  write, a hand-built restricted policy with clipboard write off denies it, and the denial is a
  typed `Error::PolicyDenied` naming the gate.
- `fake_device.rs`: drive the `TerminalDevice` trait headless with a `FakeDevice` pair, scripting
  input and asserting output without opening a terminal.

## Input Decoding And Reports

- `parse_cursor_position_report.rs`: tokenize a reply through `SyntaxParser` and parse a
  cursor-position report from the CSI token with `report::CursorPositionReport`.
- `decode_syntax_tokens.rs`: feed OSC-8 hyperlink and CSI corpus lines through `SyntaxParser` and
  inspect the lossless `SyntaxToken` families.
- `decode_key_events.rs`: feed input through `SemanticDecoder` and inspect the typed `Event`
  vocabulary — `KeyEvent` values for keys and lossless `Event::Syntax` passthrough for unmapped
  tokens.

## One-Shot Query Without An Async Runtime

- `oneshot_background.rs`: ask the terminal one question with no async runtime and default features
  (no `tokio`). Opens a `TerminalSession`, writes a single `CSI 6 n` cursor-position probe, waits
  for the session fd to become readable with `rustix::event::poll` under a bounded 150 ms budget,
  reads the reply, and parses it through the sans-io `SyntaxParser` and
  `report::CursorPositionReport`. A non-answering terminal times out cleanly and reports the
  *unknown* case rather than hanging; a `RestoreHandle` panic hook plus drop-time `leave` guarantee
  cooked-mode restoration on every exit path. This is a second, no-async consumer of the same
  decode core the async session uses, driven by a hand-rolled synchronous poll loop.
- `sync_cursor_query.rs`: the same round-trip through the typed convenience
  `TerminalSession::request_cursor_position` instead of a hand-rolled loop. One call registers the
  query with the sans-io correlator, writes `CSI 6 n`, and drives a blocking poll/read/decode loop
  until the reply completes, returning `Ok(Some(report))` on an answer, `Ok(None)` on timeout (the
  *unknown* case), and `Err(..)` only on a genuine I/O failure. Typeahead the user sent before the
  terminal answered survives for the next `read_input`. This drives the *same* correlator the async
  session uses, with no Tokio required for the synchronous path. Reach for `oneshot_background.rs`
  when you want the raw loop; reach for this when you want the typed helper.

## Tokio Session Basics

- `tokio_terminal_queries.rs`: open a Tokio session, issue live terminal-status and
  cursor-position queries, write ordered output, and leave explicitly.
- `tokio_input_events.rs`: read decoded `Event` values through
  `TokioTerminalSession::next_event`.
- `input_event_viewer.rs`: the one input example that runs on **both Unix and Windows** — a
  continuous viewer that enables mouse/focus/paste, probes kitty keyboard, and prints every decoded
  `Event` until Ctrl-C. It is the interactive driver for the
  [Windows validation runbook](https://github.com/joshka/qwertty/blob/main/docs/development/windows-validation.md)
  (eyeballing live keys, IME/CJK input, mouse, and resize across the terminal matrix).
- `kitty_keyboard.rs`: request kitty keyboard progressive-enhancement flags with verify-after-push
  (`TokioTerminalSession::request_kitty_keyboard`), inspect the granted subset, and decode rich key
  events including releases and modifiers; the session pops the granted flags on `leave`.
- `probe_capabilities.rs`: probe the terminal with one DA1-fenced query bundle
  (`TokioTerminalSession::probe_capabilities`) and print each finding as yes/no/unknown alongside
  its evidence (probed/inferred/unknown) plus the derived `TerminalIdentity` — a single write, one
  deadline, with an unknown finding meaning *unknown, not unsupported*. See
  [Capabilities](crate::docs::capabilities) for the `Finding`/`Evidence`/identity/env-heuristic
  model.
- `synchronized_frame.rs`: probe for synchronized output, then draw a capability-gated frame with
  `TokioTerminalSession::synchronized` — it emits the mode-2026 wrap only when the probe answered
  supported, and runs the same frame un-batched otherwise, never the 2026 bytes into a terminal
  that did not answer. See [Terminal Control](crate::docs::terminal_control) for the gating rule.
- `mouse_and_paste.rs`: enable SGR mouse (`enable_mouse`), focus (`enable_focus_events`), and
  bracketed paste (`enable_bracketed_paste`), then print the decoded `Event::Mouse`, `Event::Focus`,
  and `Event::Paste` values — scroll events uncoalesced, paste line endings normalized and control
  bytes flagged; the session resets all three modes on `leave`.
- `resize_events.rs`: enable in-band resize (`enable_in_band_resize`, mode 2048) and `select!` its
  coalesced `Event::Resize` delivery against the `SIGWINCH` fallback stream (`resize_stream`) — a
  resize storm collapses to one event carrying the final geometry, while scroll and mouse events
  never coalesce; the session resets mode 2048 on `leave`.
- `suspend_resume.rs`: suspend to the shell on a key (`TokioTerminalSession::suspend` — restore the
  terminal, disarm the panic-safe handle, and `SIGTSTP` the process group) and resume on `SIGCONT`
  (`resume` — re-enter raw mode and recorded modes with a bounded retry, re-assert the readiness
  fd's non-blocking flag, optionally `tcflush` stale input via the `flush_input` parameter, and
  queue a synthetic `Event::Resize`); qwertty installs no signal handler, so the app owns the
  `SIGTSTP`/`SIGCONT` wiring.
- `editor_handoff.rs`: hand the terminal to `$EDITOR` (a pager, a subshell — any synchronous child)
  and reclaim it with `TokioTerminalSession::run_detached` — restore a clean blocking terminal and
  disarm the panic-safe handle before the child, then re-enter raw mode (resyncing termios the child
  may have left cooked), re-register async readiness on the same fd, and queue a synthetic
  `Event::Resize` after; the closure is a synchronous `FnOnce` whose return value (the child's
  `ExitStatus`) is returned from `run_detached`.
- `signal_handling.rs`: `select!` the opt-in terminal-signals stream (`TokioTerminalSession::signals`
  — yielding typed `TerminalSignal::Suspend`/`Continue`/`Terminate`/`Interrupt` for
  `SIGTSTP`/`SIGCONT`/`SIGTERM`/`SIGINT`) alongside `next_event` and the `SIGWINCH` `resize_stream`,
  responding with `suspend` on `Suspend`, `resume` on `Continue`, and a graceful exit on
  `Terminate`/`Interrupt`; qwertty installs no handler and never auto-acts — the stream only reports,
  the app owns the response, and `SIGWINCH` stays with `resize_stream`.
- `tokio_query_error_handling.rs`: handle live query success, `Error::QueryTimeout`, and
  `Error::ReadTerminal` explicitly.
- `verify_queries.rs`: real-emulator verification smoke — run once per terminal application to
  check live query answers, typeahead survival, and a clean exit with your own eyes.

## Query Cancellation

- `tokio_query_cancellation.rs`: cancel a live cursor-position query and keep using the same Tokio
  session.
- `tokio_terminal_status_cancellation.rs`: cancel a live terminal-status query and keep using the
  same Tokio session.

## Query Timeouts And Follow-Up Input

- `tokio_late_query_reply.rs`: let a terminal-status query time out and then treat a late reply as
  ordinary decoded input.
- `tokio_wrong_report_query.rs`: let a cursor-position query time out and then treat a
  terminal-status report as ordinary decoded input.
- `tokio_unmatched_query_input.rs`: let a cursor-position query time out and then treat unmatched
  query-shaped CSI as ordinary decoded input.
- `tokio_preserved_unrelated_input.rs`: let a cursor-position query wait while preserving
  unrelated input for a later `next_event`.

## Terminal-Status Query Routing

- `tokio_terminal_status_preserved_input.rs`: let a terminal-status query wait while preserving
  unrelated input for a later `next_event`.
- `tokio_terminal_status_wrong_report.rs`: let a terminal-status query wait while leaving a
  cursor-position report visible through ordinary decoded input.
- `tokio_terminal_status_unmatched_query_input.rs`: let a terminal-status query wait while leaving
  unmatched query-shaped CSI visible through ordinary decoded input.

## Choosing An Example

- Start with `sync_cursor_query.rs` when you have a single question for the terminal, want the
  typed convenience, and do not want an async runtime — one call, one bounded wait, one answer, on
  default features.
- Start with `oneshot_background.rs` when you want the same no-runtime query but hand-rolled — the
  raw `poll`/`read_input`/parse loop spelled out, for when you need to shape the loop yourself.
- Start with `tokio_terminal_queries.rs` when you want the smallest end-to-end Tokio ownership
  example.
- Start with `tokio_input_events.rs` when you need decoded event delivery.
- Start with `resize_events.rs` when you need resize handling — in-band (mode 2048) with the
  `SIGWINCH` fallback.
- Start with `signal_handling.rs` when you need job-control and lifecycle signals — the opt-in
  `signals` stream for `SIGTSTP`/`SIGCONT`/`SIGTERM`/`SIGINT`, wired to `suspend`/`resume`/exit.
- Start with `tokio_query_error_handling.rs` when the main question is timeout or read-failure
  handling.
- Start with the cancellation, late-reply, wrong-report, unmatched-input, and preserved-input
  examples when you need one specific query-routing contract in isolation.
