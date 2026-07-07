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
- `session_status.rs`: open a synchronous `TerminalSession`, write ordered output, flush
  explicitly, and leave cleanly.
- `raw_mode.rs`: open the current terminal, enter raw mode through session ownership, and restore
  cooked mode on leave.
- `panic_safe_restore.rs`: install a panic hook with `RestoreHandle` so a panic restores the
  terminal before the backtrace prints.
- `session_cycles.rs`: cycle the re-entrant session enter/leave lifecycle headless, the way a
  line editor hands the terminal back between prompts.
- `read_input_bytes.rs`: read raw terminal bytes through the synchronous session boundary.
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

## Tokio Session Basics

- `tokio_terminal_queries.rs`: open a Tokio session, issue live terminal-status and
  cursor-position queries, write ordered output, and leave explicitly.
- `tokio_input_events.rs`: read decoded `Event` values through
  `TokioTerminalSession::next_event`.
- `kitty_keyboard.rs`: request kitty keyboard progressive-enhancement flags with verify-after-push
  (`TokioTerminalSession::request_kitty_keyboard`), inspect the granted subset, and decode rich key
  events including releases and modifiers; the session pops the granted flags on `leave`.
- `mouse_and_paste.rs`: enable SGR mouse (`enable_mouse`), focus (`enable_focus_events`), and
  bracketed paste (`enable_bracketed_paste`), then print the decoded `Event::Mouse`, `Event::Focus`,
  and `Event::Paste` values — scroll events uncoalesced, paste line endings normalized and control
  bytes flagged; the session resets all three modes on `leave`.
- `resize_events.rs`: enable in-band resize (`enable_in_band_resize`, mode 2048) and `select!` its
  coalesced `Event::Resize` delivery against the `SIGWINCH` fallback stream (`resize_stream`) — a
  resize storm collapses to one event carrying the final geometry, while scroll and mouse events
  never coalesce; the session resets mode 2048 on `leave`.
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

- Start with `tokio_terminal_queries.rs` when you want the smallest end-to-end Tokio ownership
  example.
- Start with `tokio_input_events.rs` when you need decoded event delivery.
- Start with `resize_events.rs` when you need resize handling — in-band (mode 2048) with the
  `SIGWINCH` fallback.
- Start with `tokio_query_error_handling.rs` when the main question is timeout or read-failure
  handling.
- Start with the cancellation, late-reply, wrong-report, unmatched-input, and preserved-input
  examples when you need one specific query-routing contract in isolation.
