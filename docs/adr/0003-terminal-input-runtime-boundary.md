# ADR 0003: Terminal Input Runtime Boundary

## Status

Accepted

## Context

qwertty now has command encoding, a Unix terminal device owner, and a small terminal session owner
for raw mode, ordered output, explicit flushing, and explicit leave cleanup. The next product step
is terminal input ownership.

Input is where the async-first direction matters most. A useful async terminal library must own
runtime-backed reads, event delivery, query response routing, cancellation behavior, and wakeup
semantics. At the same time, adding a runtime dependency before parser and event boundaries are
clear can freeze the wrong public shape.

The first input issue only needs to prove the byte boundary: qwertty can read the raw terminal bytes
that later parser, query, and policy layers will interpret.

## Decision

Add a runtime-neutral input byte boundary first.

The first input slice owns:

- `InputBytes` as an undecoded raw terminal input value;
- `Terminal::read` as the low-level device read boundary;
- `TerminalSession::read_input` as the session-level input method;
- documentation that explains the bytes are not parsed, decoded, or routed.

This slice does not add Tokio, async-std, futures traits, or a Cargo feature. The first runtime
dependency should be introduced when qwertty owns asynchronous reads or event streams as public
behavior, not as a thin wrapper over a blocking read.

## Consequences

- Tests can prove representative raw bytes without a runtime dependency.
- The public API makes input ownership visible while keeping parser and query routing non-goals.
- The next async input PR must document its runtime dependency, feature shape, cancellation
  behavior, ownership model, and relationship to `TerminalSession`.
- Callers that need interpreted keys or query responses still need their own temporary parser until
  qwertty adds those layers.

## Alternatives Considered

### Add Tokio Input Immediately

Tokio is the likely first runtime integration for async terminal applications, but adding it before
event and query ownership are clear would make a dependency decision without enough public behavior
to validate the shape.

### Add A Runtime-Agnostic Async Trait

A trait could defer the runtime choice, but it would add abstraction before the library has enough
input behavior to prove the trait methods, associated types, cancellation model, or buffering
rules.

### Skip Input Until A Full Parser Exists

Waiting for a parser would hide an important ownership boundary. Raw input bytes are useful test
evidence and a small public contract for later parser and query slices.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Terminal input reference](../reference/terminal-input.md)
- [Issue #17: Add terminal input byte events](https://github.com/joshka/qwertty/issues/17)
