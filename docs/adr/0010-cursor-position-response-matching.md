# ADR 0010: Cursor Position Response Matching

## Status

Accepted

## Context

qwertty can emit cursor position query bytes and parse cursor position reports from complete CSI
input. The next query-routing boundary is preserving unrelated input while separating a known
response from decoded events.

Full terminal query routing still needs request ownership, live reads, async runtime decisions,
timeouts, cancellation, and policy for unrelated events. Those concerns should not be hidden inside
the first matcher.

## Decision

Add `CursorPositionReport::match_events` as a deterministic event-level matcher.

The matcher consumes decoded `InputEvent` values, separates the first valid `CursorPositionReport`,
and returns every other event to the caller through `CursorPositionReportMatch`.

The matcher does not write to a terminal, read from a terminal, block, time out, prove request
origin, or depend on an async runtime.

## Consequences

- Cursor position reports can be separated from ordinary input without swallowing unrelated events.
- Tests can prove matching semantics before live terminal ownership or async routing is introduced.
- Duplicate reports after the first match remain visible to the caller.
- Malformed or unrelated CSI input remains visible instead of becoming a false match.

## Alternatives Considered

### Match Inside `InputDecoder`

`InputDecoder` owns byte-to-event classification. Matching query responses there would mix parser
state with request policy too early.

### Add A General Query Router

A general router is the product direction, but it needs request IDs, timeout behavior, unrelated
input delivery, cancellation, and runtime ownership. This slice keeps those decisions separate.

### Return Only The Report

Returning only the report would be smaller, but it would lose unrelated input. Preserving those
events is the core behavior query routing needs to guarantee.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Terminal control reference](../reference/terminal-control.md)
- [Terminal input reference](../reference/terminal-input.md)
- [Issue #45: Add cursor position query response matching](https://github.com/joshka/qwertty/issues/45)
