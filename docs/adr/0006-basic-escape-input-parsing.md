# ADR 0006: Basic Escape Input Parsing

## Status

Accepted

## Context

qwertty can read raw input bytes, classify ASCII controls, classify complete UTF-8 text within one
input chunk, and preserve incomplete or invalid UTF-8 without loss. Escape-prefixed input has
remained undecoded so qwertty does not accidentally claim a full terminal parser.

The next useful step is to classify a tiny set of common Escape-prefixed keys while keeping
unsupported and incomplete sequences lossless.

## Decision

Add `KeyInput` for the four common arrow-key Control Sequence Introducer encodings:

- `ESC [ A` as `KeyInput::Up`;
- `ESC [ B` as `KeyInput::Down`;
- `ESC [ C` as `KeyInput::Right`;
- `ESC [ D` as `KeyInput::Left`.

Add `InputEvent::Key` for parsed key input. Unknown, unsupported, or incomplete Escape-prefixed
bytes remain `InputEvent::Undecoded`.

This is not a general Control Sequence Introducer parser. It does not parse parameters,
intermediates, final bytes beyond the documented arrow-key set, terminal query responses, paste,
mouse, focus, keyboard enhancement, graphics, clipboard, or vendor extension protocols.

## Consequences

- Callers can handle the most common arrow-key input without writing byte matching themselves.
- Unknown Escape-prefixed input remains visible and lossless for later parser work.
- The parser boundary stays intentionally narrow and documented.
- Future parser and query-routing slices can replace or extend this classifier with broader
  parsing without preserving accidental behavior for unsupported sequences.

## Alternatives Considered

### Parse More CSI Keys Immediately

Home, End, Page Up, Page Down, function keys, modifiers, and keyboard enhancement protocols are
common, but parsing them introduces parameter and mode policy that belongs in a broader parser
slice.

### Leave All Escape-Prefixed Input Undecoded

This is safest, but arrow keys are common enough that a tiny documented parser provides immediate
value while still preserving unknown bytes.

### Add A Stateful Parser Now

A stateful parser will likely be needed for chunking, timeouts, query routing, and paste
ambiguity. Adding it now would force ownership and buffering decisions before qwertty has enough
input surface to prove the shape.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Terminal input reference](../reference/terminal-input.md)
- [Issue #29: Add basic Escape input parsing](https://github.com/joshka/qwertty/issues/29)
