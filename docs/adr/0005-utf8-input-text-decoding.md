# ADR 0005: UTF-8 Input Text Decoding

## Status

Accepted

## Context

qwertty can read raw input bytes and classify printable single-byte ASCII, ASCII controls, and
undecoded byte chunks. Non-ASCII input still needs a clear boundary. UTF-8 text decoding is useful,
but terminal reads can split multi-byte characters across chunks, and the same byte stream also
carries Escape-prefixed keys, query responses, paste payloads, mouse reports, focus reports, and
vendor protocols.

This slice should make complete UTF-8 text usable without introducing a stateful parser or claiming
ownership of buffering across reads.

## Decision

Decode complete UTF-8 text inside `InputBytes::events`.

The UTF-8 decoding slice owns:

- complete UTF-8 scalar values within one `InputBytes` chunk as `InputEvent::Text`;
- incomplete UTF-8 sequences as `InputEvent::Undecoded`;
- invalid UTF-8 sequences as `InputEvent::Undecoded`;
- documentation that chunk-local decoding does not buffer across terminal reads.

This slice does not add an `InputDecoder` yet. A stateful decoder should appear when qwertty owns
buffering across terminal reads, Escape parsing, query response routing, or paste ambiguity.

## Consequences

- Callers can handle ordinary Unicode text when the terminal reports complete UTF-8 in one read.
- Incomplete and invalid bytes remain lossless and visible.
- The public API stays small: `InputBytes::events` remains the classification entry point.
- Future parser work can introduce buffering without being constrained by an accidental stateful
  shape from this slice.

## Alternatives Considered

### Add `InputDecoder` Immediately

A stateful decoder is likely needed later, but adding it now would force buffering, lifetime, reset,
and error-policy decisions before Escape parsing and query routing are designed.

### Keep All Non-ASCII Bytes Undecoded

This keeps the boundary very small, but it makes common text input unnecessarily hard for callers
and tests once `InputEvent::Text` already exists.

### Replace Invalid UTF-8

Replacement characters are convenient for display, but they lose the original bytes. qwertty should
preserve bytes until policy code explicitly decides how to recover from invalid input.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Terminal input reference](../reference/terminal-input.md)
- [Issue #25: Add UTF-8 input text decoding](https://github.com/joshka/qwertty/issues/25)
