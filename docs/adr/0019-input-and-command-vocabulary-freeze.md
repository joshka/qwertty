# ADR 19: Input and command vocabulary freeze for 0.1

## Status

Accepted

## Context

The public input event vocabulary (`Event` and its members) and the command families
(`commands::{cursor, screen, style, osc, terminal}`) were rebuilt from first principles during
the Phase 4 implementation and exercised by every input and output slice, by 285 host-to-terminal
fixtures, by an OQ-6 text-granularity spike, and against two real terminal implementations (tmux
and headless ghostty). These types are what downstream frameworks link, so their shape needs a
stability commitment before they proliferate through consumers.

The versioning policy (ADR 15) allows breaking pre-1.0 types freely but requires a stated point
where a contract stabilises. This ADR is that point for the vocabulary.

## Decision

The following are **frozen for 0.1**. Breaking their shape now requires an explicit maintainer
decision and a superseding record; additive growth stays open because the enums are
`#[non_exhaustive]`.

1. **`KeyEvent` carries text on the key event** — `{ key, modifiers, kind, text:
   Option<TextPayload>, shifted_key, base_layout_key }`. Text is a multi-codepoint payload
   associated with the key, not one event per codepoint and not a separate composed-text event.
2. **`PasteEvent` is segmented and lossless** — `{ data, is_first, is_final, terminated }`. Paste
   is captured opaquely at the syntax layer; large pastes segment without truncating data, and an
   unterminated paste flushes a `terminated == false` final segment.
3. **Resize coalesces; mouse and scroll never do** — a resize storm collapses to one
   `Event::Resize(ResizeEvent { cells, pixels })` with the final geometry; every mouse and scroll
   event is delivered.
4. **`Event::Syntax` is a permanent passthrough variant** — anything not semantically decoded
   arrives as a lossless `SyntaxToken`, never a fabricated keypress; this stays even as more
   decoders land.
5. **SGR emits the semicolon form** — colon subparameters only where they are the sole form
   (underline style `4:x`); the widest-supported spelling elsewhere.
6. **Module boundaries and public names** — commands grouped by protocol domain; `TerminalSession`
   / `TokioTerminalSession`; reports at the crate root and under `report::` (the stable citation
   path for downstream analysis).

## Consequences

- Downstream frameworks (rabbitui) can build durable event-handling against these types and pin to
  this point rather than tracking churn; the substrate status document records the frozen change.
- Post-freeze shape changes to these types are maintainer-gated. New protocol families, new key or
  event variants, and new command helpers remain additive and unrestricted.
- Width *measurement* is explicitly out of this freeze: it is a separate future API with an open
  design spike, and freezing the vocabulary does not commit its shape.

## Alternatives Considered

- **`Text(char)`-only key events** (the crossterm shape): rejected by the OQ-6 spike — it splits
  multi-codepoint and ZWJ-cluster text into unrelated events and cannot carry modifiers with text,
  which is the source of chronic incumbent IME bugs.
- **A separate `ComposedText(String)` event**: rejected — it loses the key/modifier association the
  moment text appears, and is not an as-shipped terminal-side shape.
- **One `Paste(String)` event**: rejected — a multi-megabyte paste as one allocation blocks the
  event loop, and reassembling paste from decoded tokens cannot keep embedded escape bytes
  byte-exact or bound memory on an unterminated paste.
- **Deferring the freeze to 1.0**: rejected — the vocabulary is exercised and stable now, and
  leaving it unpinned forces downstream to track churn indefinitely.

## Reference Material

- OQ-6 text-granularity spike: `work/phase2/spikes/text-granularity/`.
- Decoder and vocabulary design: `work/phase2/design/02-decoder.md`,
  `work/phase2/design/08-api-boundaries.md`.
- Maintainer review and acknowledgement: `work/phase4/review-01-vocabulary-freeze.md` (acked
  2026-07-07).
