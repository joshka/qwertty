# Architecture

qwertty starts with a small crate and will split package boundaries only when the split improves
ownership, stability, dependency isolation, or audience clarity.

## Planned Layers

- `qwertty`: user-facing facade and practical entry points.
- Terminal device layer: opening the current terminal, raw mode, size, and IO boundaries.
- Session layer: terminal ownership, ordered output, explicit flushing, cleanup, and explicit leave
  behavior.
- Input layer: raw input bytes first, then parsed events and query response routing.
- Protocol layer: runtime-neutral command, event, query, and syntax types.
- Testkit layer: deterministic tests for terminal behavior and protocol fixtures.

## Boundary Rule

A module becomes a crate only when it has an independent audience, dependency set, stability
policy, or ownership model. Tiny protocol surfaces should begin as modules or planned work.

## Layer Boundary

The terminal device layer should stay below the session layer. It owns the live terminal handle,
raw/cooked mode transition, terminal size lookup, and byte-oriented write/flush boundary.

It should not own application lifecycle policy yet. Session setup, alternate screen, ordered frame
cleanup, feature cleanup, input parsing, query routing, and async event loops belong to later
slices unless the implementation issue records a narrower reason to move one of those boundaries.

The first session layer owns raw-mode entry, ordered output writes, explicit flushing, and explicit
leave cleanup. It does not yet own input parsing, query routing, alternate screen policy, feature
cleanup, or async runtime integration.

The input layer owns raw bytes read from a terminal session, basic text/control classification,
complete UTF-8 text, a tiny documented Escape parser for common arrow keys, and a small stateful
decoder for incomplete UTF-8 and Control Sequence Introducer input split across chunks. It can
preserve complete CSI syntax, parse cursor position reports, and match those reports from decoded
events without assigning broader query, key, paste, mouse, focus, response, or vendor protocol
meaning. Those interpretations belong to later parser and policy slices.

## Async Runtime Boundary

The first async public surface is `TokioTerminalSession`, a Tokio-specific session owner behind an
optional `tokio` Cargo feature. The feature is disabled by default so command, protocol, terminal
device, and runtime-neutral session users do not compile Tokio unless they opt in.

The Tokio session owner uses runtime-backed terminal reads and writes, feeds reads through
`InputDecoder`, preserves unrelated decoded input in its internal event queue, and documents
cancellation at the event-delivery boundary. It is not a thin async wrapper around the synchronous
`TerminalSession` methods.

Runtime-agnostic async traits are deferred until a concrete Tokio implementation proves behavior
that another runtime can share without adding unnecessary abstraction.

## Query Routing Boundary

Live query routing starts inside `TokioTerminalSession`. The session owns terminal writes, flushes,
runtime-backed reads, decoder state, unrelated decoded input, query timeouts, and cancellation
semantics for the first async query helpers.

Public query APIs should remain narrow typed session methods until qwertty has enough query shapes
to prove a broader abstraction. A private session-owned router may collect pending query state and
shared matching mechanics, but a public generic router, concurrent query registry, capability
probing surface, and runtime-agnostic query trait are deferred.

Unrelated decoded input is always part of the session event stream. Query helpers that read events
before their response must keep those events queued for later `TokioTerminalSession::next_event`
calls.

## Design Rule

Public APIs are conservative until examples prove the shape. Durable choices about crate
boundaries, terminal ownership, parser architecture, query routing, policy, and release scope
belong in ADRs.

See [ADR 0013: Platform Support Policy](adr/0013-platform-support-policy.md) for the current
Unix-first support boundary and the rule for widening platform claims.
See [ADR 0014: Crate and Module Split Policy](adr/0014-crate-and-module-split-policy.md) for the
rule for keeping work inside `qwertty` and the criteria for widening package boundaries.
See [ADR 0015: Versioning and Compatibility Policy](adr/0015-versioning-and-compatibility-policy.md)
for the current pre-release compatibility posture and the rule for claiming stronger stability.
See [ADR 0016: Dependency Policy](adr/0016-dependency-policy.md) for the rule for dependency
additions, optional features, and dependency upgrades.
