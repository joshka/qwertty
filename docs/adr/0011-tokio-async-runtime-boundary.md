# ADR 0011: Tokio Async Runtime Boundary

## Status

Accepted

## Context

qwertty has a runtime-neutral command, terminal device, session, input decoder, CSI value, cursor
position report parser, and cursor position response matcher. Those pieces prove terminal
ownership, decoded input, and the first query-shaped response behavior without committing the crate
to an async runtime too early.

The next public boundary is async terminal ownership. qwertty should not add async methods that
only wrap blocking `TerminalSession` reads and writes. It also should not add a runtime-agnostic
trait before the event, cancellation, and query-routing shape is proven by one real runtime.

Rust async terminal I/O is runtime-shaped in practice. File descriptor readiness, task wakeups,
feature flags, cancellation behavior, and test utilities are not interchangeable details. Tokio is
the practical first integration target for async Rust terminal applications.

## Decision

Introduce the first async public surface as a Tokio-specific integration behind an optional
`tokio` Cargo feature. The feature stays disabled by default.

The first async owner should be a separate Tokio session type, tentatively named
`TokioTerminalSession`, instead of adding async-looking methods to the existing runtime-neutral
`TerminalSession`.

The Tokio session owner must own:

- live terminal opening for the current terminal and test-provided paths where the platform
  supports them;
- raw-mode entry and explicit cooked-mode restoration;
- ordered command, raw byte, text, and flush operations through runtime-backed output I/O;
- runtime-backed terminal reads;
- buffering through the existing `InputDecoder`;
- decoded input event delivery while preserving unrelated and undecoded input;
- cancellation behavior at the event-delivery boundary;
- a best-effort drop fallback for terminal-mode restoration.

Cancellation at this boundary means a canceled event read leaves the session usable, and any bytes
or events already buffered by qwertty remain available to later calls. Query-level cancellation,
timeouts, and request ownership remain separate work.

The synchronous `Terminal` and `TerminalSession` APIs remain runtime-neutral. They are still the
small device and session core, not the hidden implementation of async methods that may block while
being polled.

Do not add a runtime-agnostic async trait yet. A trait can be added later if the Tokio owner proves
stable behavior that another runtime can share without forcing users through unnecessary
abstraction.

The first implementation issue is
[Add feature-gated Tokio terminal session owner](https://github.com/joshka/qwertty/issues/52).

## Consequences

- Existing users do not gain a Tokio dependency unless they enable the feature.
- The first async implementation can use Tokio's readiness, wakeup, cancellation, and test
  facilities directly.
- The public API makes runtime ownership explicit instead of presenting a generic abstraction too
  early.
- Query routing can build on a real async owner instead of combining runtime and request-policy
  decisions in one large slice.
- A later multi-runtime API remains possible after qwertty has concrete behavior to abstract.

## Alternatives Considered

### Add Tokio As A Required Dependency

This would make the async-first direction visible immediately, but it would force every user of the
runtime-neutral command, protocol, and synchronous session APIs to compile Tokio.

### Add Async Methods To `TerminalSession`

This would keep the type list small, but it risks hiding blocking file I/O behind async method
names. It would also make it harder to explain which owner is responsible for runtime wakeups and
cancellation.

### Add Runtime-Agnostic Async Traits First

Traits could keep room for multiple runtimes, but they would require decisions about associated
types, streams, cancellation, buffering, and query ownership before qwertty has one complete async
runtime implementation.

### Defer Async Behind Another Runtime-Neutral Owner

Another runtime-neutral owner would delay the actual async integration while adding public API
surface. The project has enough parser and response-matching behavior to introduce the first real
runtime boundary now.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Terminal session reference](../reference/terminal-session.md)
- [Terminal input reference](../reference/terminal-input.md)
- [ADR 0001: Async-First Terminal I/O Boundary](0001-async-first-terminal-io-boundary.md)
- [ADR 0002: Terminal Session Cleanup](0002-terminal-session-cleanup.md)
- [ADR 0003: Terminal Input Runtime Boundary](0003-terminal-input-runtime-boundary.md)
- [ADR 0010: Cursor Position Response Matching](0010-cursor-position-response-matching.md)
- [Issue #49: Decide async runtime boundary](https://github.com/joshka/qwertty/issues/49)
