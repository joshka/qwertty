# ADR 0007: Stateful Input Decoder

## Status

Accepted

## Context

qwertty can read raw input bytes, classify ASCII controls, decode complete UTF-8 text within one
input chunk, and parse a small documented set of Escape-prefixed arrow keys. Chunk-local
classification keeps bytes lossless, but terminal reads can split multi-byte UTF-8 scalar values
and Escape-prefixed key sequences across reads.

The next useful step is to buffer those incomplete boundary cases without turning the input layer
into query routing, paste policy, timing policy, or a broad Control Sequence Introducer parser.

## Decision

Add `InputDecoder` as the first stateful input owner.

`InputDecoder` owns:

- buffered incomplete UTF-8 scalar values across decode calls;
- buffered incomplete documented Escape-prefixed key sequences across decode calls;
- `pending_bytes` so callers can inspect the exact buffered bytes;
- `finish` so callers can explicitly return remaining buffered bytes as `InputEvent::Undecoded`.

The decoder returns the existing `InputEvent` values. Unsupported Escape-prefixed input and invalid
UTF-8 remain lossless undecoded bytes.

`finish` does not classify a pending Escape byte as `ControlInput::Escape`. Distinguishing a
standalone Escape key from the start of a longer sequence needs timing or application policy, which
belongs to a later layer.

## Consequences

- Callers can decode common text and arrow-key input even when terminal reads split those bytes.
- The public API now has an explicit owner for cross-read buffering instead of hiding state inside
  `InputBytes`.
- Incomplete input can be flushed without losing bytes or inventing timing behavior.
- Query response routing, paste ambiguity, mouse, focus, keyboard enhancement, graphics,
  clipboard, and vendor protocols remain outside this slice.

## Alternatives Considered

### Make `InputBytes::events` Stateful

`InputBytes` is a value type for one terminal read. Giving it hidden cross-read state would make the
API harder to reason about and would blur the ownership boundary between raw reads and decoding.

### Classify Pending Escape On Finish

Treating a pending Escape byte as `ControlInput::Escape` on finish is convenient, but it silently
chooses an ambiguity policy. Returning undecoded bytes keeps the policy visible to the caller.

### Add A General CSI Parser

A broader parser will be needed later, but parameter parsing, final-byte handling, and terminal
query response routing need their own tests and policy decisions.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Terminal input reference](../reference/terminal-input.md)
- [Issue #33: Add stateful input decoder](https://github.com/joshka/qwertty/issues/33)
