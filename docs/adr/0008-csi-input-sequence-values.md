# ADR 0008: CSI Input Sequence Values

## Status

Accepted

## Context

qwertty can read raw input bytes, classify controls and UTF-8 text, parse a small arrow-key set,
and buffer incomplete UTF-8 or Escape-prefixed input across chunks with `InputDecoder`.

The next parser boundary needs to make complete Control Sequence Introducer input visible without
turning it into terminal query routing, key policy, paste handling, mouse handling, or capability
policy. Cursor position reports, device status reports, keyboard enhancement responses, and vendor
extensions all use CSI-shaped bytes, but interpreting those bytes requires later protocol-specific
work.

## Decision

Add `CsiInput` as a small public value for complete 7-bit CSI input syntax.

`CsiInput` preserves:

- the original bytes;
- parameter bytes;
- leading private marker parameter bytes;
- intermediate bytes;
- the final byte.

`InputEvent::Csi` carries complete CSI input that is not one of qwertty's documented arrow keys.
The arrow-key sequences `ESC [ A`, `ESC [ B`, `ESC [ C`, and `ESC [ D` continue to produce
`InputEvent::Key`.

`InputDecoder` buffers incomplete CSI input across chunks and `finish` returns any remaining
buffered CSI bytes as undecoded input. Unsupported non-CSI Escape-prefixed input remains
`InputEvent::Undecoded`.

## Consequences

- Callers can distinguish complete CSI input from opaque undecoded bytes.
- Later query-routing and richer input parsing can build on a lossless syntax value.
- qwertty still does not claim to interpret CSI meanings, query responses, mouse events, keyboard
  enhancement reports, or vendor extensions.
- Existing arrow-key behavior remains stable.

## Alternatives Considered

### Keep Complete CSI Undecoded

This would preserve bytes, but it would force later query-routing and parser slices to repeat the
same byte-shape detection before they can make progress.

### Interpret Known Reports Immediately

Cursor position and device status reports are common, but interpreting them requires request and
response routing policy. That belongs in a later slice.

### Replace Arrow Keys With CSI Values

Arrow keys are already a documented public event. Replacing them with generic CSI values would make
the API less useful and would break the intentionally small key input surface.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Terminal control reference](../reference/terminal-control.md)
- [Terminal input reference](../reference/terminal-input.md)
- [Issue #37: Add CSI input sequence values](https://github.com/joshka/qwertty/issues/37)
