# ADR 0012: Terminal Query Routing Boundary

## Status

Accepted

## Context

qwertty can now emit a cursor position Device Status Report request, parse cursor position reports,
match the first report from decoded input events, own a Tokio-backed terminal session, and perform
one live cursor position query with timeout-bounded behavior.

That vertical path proves the smallest useful request/response workflow. The next boundary is where
query-routing state should live as qwertty grows beyond one helper. Query routing must preserve
unrelated decoded input, keep cancellation behavior understandable, and avoid forcing users to
assemble low-level terminal reads, writes, parser state, and timeout policy themselves.

At the same time, a broad public router is still early. qwertty has one live query shape and one
async runtime owner. Public abstractions for generic query registration, concurrent request routing,
capability probing, and non-Tokio runtimes would commit names and contracts before the library has
enough behavior to generalize.

## Decision

Keep query routing state inside the Tokio session owner for the next implementation slice.

`TokioTerminalSession` remains the public owner for live async query helpers. Public query APIs
should be narrow typed methods first, such as `request_cursor_position`, rather than a public
generic router. Internally, the Tokio session may extract a small query-routing component that owns
pending query state, decoded event buffering, response matching, timeout handling, and unrelated
event preservation.

The first internal router should support one pending live query at a time. Multiple simultaneous
queries are out of scope until qwertty has more query shapes and a concrete response
disambiguation story.

Unrelated decoded input remains part of the session event stream. Events read before a matching
query response must be queued so later `TokioTerminalSession::next_event` calls can observe them in
their original order.

Timeouts are query-level errors. A timeout should return `Error::QueryTimeout` with an operation
name and the caller-provided duration, while keeping already decoded unrelated events available.

Cancellation is defined at the session boundary. Canceling a pending query future while it is
waiting for terminal readiness must leave the session usable. Any bytes, decoder state, or decoded
events already owned by qwertty must remain available to later session calls.

Do not add a runtime-agnostic async query trait yet. A shared trait can be considered after the
Tokio owner has more than one query helper and the shared behavior is concrete.

The next implementation issue is
[Extract Tokio query routing state](https://github.com/joshka/qwertty/issues/62).

## Consequences

- Users keep a simple session-owned API for live terminal queries.
- The implementation can remove duplicated query loop mechanics without exposing a broad router.
- Unrelated input preservation remains a required query-routing behavior.
- Timeout and cancellation behavior stay documented at the public session boundary.
- The library can add more typed query helpers before deciding whether a public router exists.
- Concurrent live queries, capability probing, and non-Tokio query traits remain later work.

## Alternatives Considered

### Add A Public Query Router Type

A public router would make the product direction visible, but it would require stable decisions
about registration, event streams, concurrent queries, cancellation, timeout ownership, and runtime
integration before qwertty has enough query behavior to prove the shape.

### Keep Each Query As A Separate Method With Duplicated Loops

This keeps the public surface small, but it would make every new live query repeat the same
buffering, timeout, cancellation, and unrelated-event rules. An internal router gives those rules a
single owner without committing to public generic routing yet.

### Add Runtime-Agnostic Async Query Traits Now

Traits would make multi-runtime support look available before the behavior is proven. Tokio remains
the only runtime-backed owner, so a trait would mostly freeze abstractions around one
implementation.

### Support Multiple Simultaneous Queries First

Concurrent query routing is useful eventually, but response disambiguation is protocol-specific.
Starting with one pending query keeps correctness local while qwertty builds more typed query
behavior.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Terminal session reference](../reference/terminal-session.md)
- [Terminal input reference](../reference/terminal-input.md)
- [Terminal control reference](../reference/terminal-control.md)
- [ADR 0009: Cursor Position Query Report](0009-cursor-position-query-report.md)
- [ADR 0010: Cursor Position Response Matching](0010-cursor-position-response-matching.md)
- [ADR 0011: Tokio Async Runtime Boundary](0011-tokio-async-runtime-boundary.md)
- [Issue #59: Decide terminal query routing boundary](https://github.com/joshka/qwertty/issues/59)
