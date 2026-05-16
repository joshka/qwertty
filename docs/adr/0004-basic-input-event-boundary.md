# ADR 0004: Basic Input Event Boundary

## Status

Accepted

## Context

qwertty can read raw input bytes through `TerminalSession::read_input` and preserve those bytes as
`InputBytes`. The next step is to make simple input usable without claiming that qwertty has a full
terminal parser.

Terminal input is ambiguous. The same stream carries printable text, C0 controls, Escape-prefixed
key sequences, Control Sequence Introducer messages, paste payloads, mouse reports, focus reports,
terminal query responses, and vendor protocol data. Classifying too much too early would create
incorrect public promises and make later parser/query routing harder to change.

## Decision

Add `InputEvent` as a minimal classification layer above `InputBytes`.

The first event classifier owns:

- printable single-byte ASCII text as `InputEvent::Text`;
- ASCII C0 controls and Delete as `InputEvent::Control`;
- Escape-prefixed input, non-ASCII bytes, UTF-8 sequences, query responses, paste, mouse, focus, and
  vendor protocol bytes as `InputEvent::Undecoded`.

Escape by itself is a control byte. Escape followed by more bytes is left undecoded so qwertty does
not pretend to parse keys or protocol messages before parser and query-routing slices exist.

## Consequences

- Callers can handle simple text and control input without writing byte tests themselves.
- Undecoded bytes remain visible and lossless for application-specific handling.
- The parser boundary stays honest: qwertty has a classifier, not a full terminal input parser.
- Later parser and query-routing work can build from deterministic fixtures without preserving an
  accidental event shape for complex sequences.

## Alternatives Considered

### Decode UTF-8 Text Immediately

UTF-8 decoding is useful, but terminal input can split multi-byte code points across reads. Decoding
correctly needs buffering and error policy, which belongs in a later parser slice.

### Parse Common Arrow-Key Escape Sequences

Arrow keys are tempting, but parsing a few sequences would blur the boundary between basic events
and a real Escape/Control Sequence Introducer parser.

### Return Only Raw Bytes Until A Full Parser Exists

Raw bytes are accurate, but they force every caller to repeat basic printable/control
classification. A minimal event layer adds value while preserving undecoded protocol bytes.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Terminal input reference](../reference/terminal-input.md)
- [Issue #21: Add basic terminal input events](https://github.com/joshka/qwertty/issues/21)
