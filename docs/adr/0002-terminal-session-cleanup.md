# ADR 0002: Terminal Session Cleanup

## Status

Accepted

## Context

The terminal device layer can open a terminal, enter raw mode, write bytes, flush, and restore the
captured cooked mode. Application code needs a higher-level owner that starts a terminal session,
keeps output ordering visible in the API, and gives callers an explicit cleanup path.

This slice should not pull in input parsing, query routing, alternate screen policy, mouse or paste
mode policy, vendor protocol cleanup, or a runtime dependency before those behaviors have their own
issues and examples.

There is also an async-first product constraint. qwertty should grow toward async terminal
ownership, but an async method that only wraps synchronous file I/O would imply a runtime story the
library has not proven yet.

## Decision

Add `TerminalSession` as a small owner above `Terminal`.

The first session lifecycle owns:

- opening or accepting a terminal device;
- entering raw mode when the session starts;
- writing command bytes, raw bytes, and text bytes in method-call order;
- explicit flushing;
- explicit `leave` that reports cooked-mode restoration errors;
- best-effort drop fallback through the underlying terminal.

The first session lifecycle does not own:

- input parsing;
- query routing;
- alternate screen setup or teardown;
- cursor visibility cleanup;
- mouse, paste, graphics, clipboard, or vendor extension policy;
- async runtime integration.

The first public session API stays runtime-neutral. Async terminal I/O remains a first-class product
goal and should enter when the library owns runtime-backed reads, writes, events, or query
responses.

## Consequences

- Applications get one owner for raw mode, ordered output, flushing, and explicit leave cleanup.
- Drop remains a last line of defense, not the primary cleanup API.
- Output examples can use a session without introducing input or protocol policy.
- The API avoids a fake async surface over synchronous file operations.
- The next async PR must document its runtime dependency, feature shape, and ownership model.

## Alternatives Considered

### Make `leave` Flush Output

Implicit flushing would make simple examples shorter, but it hides an output ordering decision in
cleanup. The first session API keeps flush explicit so users decide when visible output matters.

### Add Async Output Methods Immediately

Async methods would advertise the product direction earlier, but methods backed only by synchronous
file writes could block while being polled and would not prove a useful runtime boundary.

### Let Drop Be The Main Cleanup API

Drop-time cleanup is convenient, but it cannot report restoration errors. Terminal applications need
an explicit shutdown path that can fail loudly during orderly exits.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Terminal session reference](../reference/terminal-session.md)
- [Issue #13: Add terminal session lifecycle](https://github.com/joshka/qwertty/issues/13)
