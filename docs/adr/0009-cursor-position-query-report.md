# ADR 0009: Cursor Position Query Report

## Status

Accepted

## Context

qwertty can emit cursor movement commands, represent one-based protocol positions, decode input
events, buffer incomplete input across reads, and preserve complete CSI input syntax. The next
query-facing step should interpret one well-known terminal report without introducing the full
request/response owner that async query routing will need.

Cursor position reporting is a narrow first boundary. The request bytes are `CSI 6 n`, and terminals
commonly answer with `CSI row ; column R`.

## Decision

Add `commands::cursor::request_position` to emit `CSI 6 n`.

Add `CursorPositionReport` to parse `CSI row ; column R` from a complete `CsiInput` value and
return the one-based `ProtocolPosition` reported by the terminal.

This slice does not write the query to a live terminal, wait for a response, match responses to
requests, route unrelated input, add timeouts, or add an async runtime dependency.

## Consequences

- Users can see the exact cursor position query bytes qwertty emits.
- Users can parse one concrete query-shaped CSI report into a typed value.
- The library gains a small foundation for later query-routing work without committing to a runtime
  or request owner yet.
- Malformed, unrelated, private, or unsupported CSI input remains a non-match instead of a lossy
  parse error.

## Alternatives Considered

### Add Full Query Routing Now

Full routing needs request ownership, response matching, unrelated input preservation, timeout
policy, cancellation, and eventually async runtime boundaries. That is too much for this slice.

### Only Add The Request Command

Emitting `CSI 6 n` is useful, but without the matching report parser it would leave users to parse
the first query response by hand.

### Parse More Device Status Reports

Other DSR forms exist, but cursor position is enough to prove the query/report boundary while
keeping the public contract small.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Terminal control reference](../reference/terminal-control.md)
- [Terminal input reference](../reference/terminal-input.md)
- [Issue #41: Add cursor position query report parsing](https://github.com/joshka/qwertty/issues/41)
