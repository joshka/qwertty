# ADR 0001: Async-First Terminal I/O Boundary

## Status

Accepted

## Context

qwertty should be an async-first terminal library. The main reason to choose it over smaller
terminal helper crates is that it will own terminal output ordering, input bytes, terminal events,
queries, capability policy, and cleanup in async applications.

The first implementation slices still need a small terminal-device boundary. Opening a terminal,
capturing its original mode, entering raw mode, restoring cooked mode, querying size, and writing
bytes are operating-system concerns. Those operations can be tested without committing every later
session, parser, or event-loop decision.

That creates a staging risk. If the device layer is only synchronous, the library can look like a
synchronous terminal utility with async added later. If the device layer introduces async too early,
it can harden runtime dependencies and input ownership before the session and event model are
designed.

## Decision

qwertty is async-first, but its terminal-device layer may expose a small synchronous core surface as
the foundation for terminal ownership and tests.

The synchronous device surface should own:

- opening the current terminal;
- opening a specific terminal path where the platform supports it;
- capturing the original terminal mode;
- entering raw mode;
- restoring cooked mode explicitly;
- best-effort drop-time terminal-mode restoration;
- querying terminal size;
- byte-oriented write and flush.

Async terminal I/O is a first-class product goal, not an optional afterthought. It should enter the
public API as soon as the session or input slices need async ownership of terminal reads, writes,
events, or query responses.

The terminal-device implementation should not hide or preclude async. It should keep ownership
boundaries, error types, terminal size types, and byte I/O semantics compatible with an async
session layer.

The first async implementation may be runtime-specific and feature-gated. Tokio is the likely first
runtime integration because it is the practical baseline for async Rust applications, but the exact
public shape belongs to the slice that introduces async reads, writes, or event routing.

## Consequences

- Issue-sized work can start with a runtime-neutral device core and PTY-backed tests.
- qwertty's public direction remains async-first even if the first device PR is synchronous.
- Async reader/writer APIs should not appear accidentally as helpers inside the device layer.
- A PR that introduces async terminal I/O must document its runtime dependency, feature flag shape,
  ownership model, and relationship to session cleanup.
- The session layer remains responsible for application lifecycle behavior: alternate screen,
  ordered cleanup, feature cleanup, input parsing, query routing, and event-loop policy.
- The device layer remains responsible for operating-system terminal state, not emulator protocol
  state.

## Alternatives Considered

### Make The Device Layer Fully Async Immediately

This would make the async-first product direction visible from the first live-terminal API. It
also risks coupling the low-level device boundary to a runtime, input ownership model, and Windows
readiness strategy before the session layer proves the shape.

### Keep Async Entirely Out Of The Device Design

This would keep the first device PR small and runtime-neutral, but it makes the library's purpose
less clear and can lead to APIs that are awkward to adapt to async sessions later.

### Hide Async Behind Runtime-Agnostic Traits

This could leave room for multiple runtimes, but it adds abstraction before the library has enough
public examples to prove the trait shape. The project should not add that indirection until it
reduces real caller complexity.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Issue #3: Add terminal device layer](https://github.com/joshka/qwertty/issues/3)
