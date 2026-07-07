# Release-Blocking Examples

This page identifies the checked-in examples that are part of the `0.1.0` release evidence for
qwertty.

Use it when deciding whether the first published version still has a coherent public example
surface. The full example index in [Checked-In Examples](crate::docs#checked-in-examples) remains
the discovery map for every shipped example. This page is narrower: it names the examples that
should remain correct, runnable, and aligned with the first release target.

## Why This List Exists

qwertty's first release target is not "every helper has an example." It is a Unix-first terminal
library with:

- runtime-neutral command encoding and protocol parsing in memory;
- synchronous terminal ownership and session lifecycle on Unix;
- optional Tokio-backed async session ownership on Unix;
- decoded input delivery and live query behavior that are already part of the documented async
  contract.

The release-blocking examples are the smallest set that proves those user-facing workflows exist in
checked-in runnable form for `0.1.0`.

## Release-Blocking Examples For `0.1.0`

### `build_status_line.rs`

Why it is release-blocking:

- proves the encode-only path works without a live terminal;
- demonstrates `CommandBuffer`, command ordering, cursor movement, and text writes through the
  intended high-level command API;
- represents the runtime-neutral command-construction surface that belongs in the first release
  target.

### `session_status.rs`

Why it is release-blocking:

- proves the synchronous `TerminalSession` ownership path exists as a real workflow, not only as
  API fragments;
- shows ordered output, explicit flush, and explicit leave in one small example;
- represents the synchronous session lifecycle that is part of the first release target.

### `read_input_bytes.rs`

Why it is release-blocking:

- proves the synchronous session boundary can read raw terminal input bytes;
- keeps the first release honest about the current input layer below richer async event delivery;
- represents the current synchronous input contract without pretending the synchronous API already
  owns higher-level decoded event routing.

### `tokio_terminal_queries.rs`

Why it is release-blocking:

- proves the optional Tokio session owner is part of the first release product, not a sidecar;
- shows ordered async output, explicit flush, and both live query helpers in one end-to-end
  workflow;
- represents the async ownership and live-query surface that makes qwertty an async-first terminal
  library in practice.

### `tokio_input_events.rs`

Why it is release-blocking:

- proves decoded `Event` delivery through `TokioTerminalSession::next_event`;
- keeps the first release target honest about owning async input events, not just query helpers and
  output writes;
- represents the decoded async event stream that users should expect from the Tokio session owner.

### `kitty_keyboard.rs`

Why it is release-blocking:

- proves the kitty keyboard verify-after-push workflow end to end:
  `TokioTerminalSession::request_kitty_keyboard`, the granted-subset result, and rich key-event
  decoding (releases, modifiers, associated text);
- keeps the first release honest about owning the terminal's most-requested input enhancement, with
  the granted-vs-requested and unknown-vs-unsupported distinctions users must act on;
- represents the progressive-enhancement lifecycle (push, verify, ledger-recorded teardown) that no
  other example covers.

### `tokio_query_error_handling.rs`

Why it is release-blocking:

- proves the documented query success, timeout, and read-failure paths in runnable form;
- keeps the first release honest about the operational query contract instead of treating errors as
  an undocumented edge;
- represents the part of the async query surface most applications need to handle directly.

## Important But Not Release-Blocking By Themselves

The remaining checked-in examples still matter. They explain narrower contracts such as
cancellation, late replies, wrong-report handling, unmatched query-shaped input, and preserved
unrelated input.

Those examples support the release surface, but they are not the minimal blocking set for `0.1.0`.
Their underlying behavior is still release-relevant through the user-facing references, PTY-backed
tests, and validation gate.

## Review Rule

If one of the release-blocking examples stops matching the documented `0.1.0` product surface,
release work should treat that as a blocker. Either the example, the docs, or the release target is
wrong, and the mismatch should be resolved before publication.

## Related References

- [Checked-In Examples](crate::docs#checked-in-examples)
- [Release Checklist](crate::docs#release-checklist)
- [Release Readiness](crate::docs#release-readiness)
- [Tokio Input Ownership And Query Handoff](crate::docs#tokio-input-ownership-and-query-handoff)
