# Design 02 — Decoder architecture

Streaming, chunk-boundary-safe, forward-compatible input decoding. Implements R-IN-1–10;
addresses FM-P1–P16. Grounded in the reproduced escape-layer spike plus the fresh `vte` spike
(maintainer-local record, run 2026-07-06 against vte 0.15.0).

## The parser decision (spike-settled)

**qwertty owns its syntax parser.** The two candidates that could have replaced it both fail
on measured facts:

- **vte 0.15** (fresh spike): APC/PM/SOS payloads are *invisible* — the state machine
  consumes them with no callback, so byte-lossless preservation is impossible (FAIL);
  OSC terminated by 8-bit ST (0x9C) vanishes with zero callbacks; `osc_raw` is unbounded
  under the default `std` feature (10 MB unterminated OSC → ~20 MB RSS, vte#151 alive in
  0.15); >32 semicolon params silently truncate behind a boolean. Its virtues (correct
  12-case coverage, chunk-split equivalence, no panics, subparams) are real but sit on the
  wrong foundation for a preserve-don't-corrupt decoder.
- **vtparse** (prototype spike, audit-reproduced): best coverage of the field (12/13) but
  0/1 on malformed, and the prototype's rejection reasons verify independently: OSC-ST
  fidelity, DCS metadata gaps, PM/SOS invisibility, no unknown-byte preservation, weaker
  chunk/EOF contracts. It also lives in the wezterm tree (FM-E2's vendoring trap — zellij
  vendored termwiz for exactly this class of fix).
- **Forking vte**: the changes are to the table-driven core itself (new states/actions for
  APC/PM/SOS content, C1 handling, byte-span plumbing, bounds) — a diverged core with
  upstream's API-churn history (five breaking releases, FM-E4) and none of the reuse benefit.
  Writing our own is the same work without the drag.

The parser is small, stable, and *ours to prove*: the escape-layer spike corpus + the vte
spike criteria become its permanent test suite, and the database fixtures feed it forever.

## Two layers: total syntax, typed semantics

```text
bytes ──▶ SyntaxParser ──▶ SyntaxToken ──▶ SemanticDecoder ──▶ InputEvent
          (total, lossless,  Text / C0 /     (keys, mouse,       (public vocabulary)
           bounded, stateful) Csi / Osc /     paste, focus,
                              Dcs / Apc /     reports…)
                              Pm / Sos /
                              Esc / Malformed)
```

**Syntax layer — a total function over bytes.** Invariants (each is a fuzz target):

1. **Reconstruction**: concatenating the byte spans of emitted tokens reproduces the input
   byte-for-byte — including malformed input, which becomes a `Malformed` token carrying its
   bytes, never a silent drop (the spike criterion every incumbent failed; FM-P5/P6/P8).
2. **Split-equivalence**: any chunking of the input yields the identical token sequence
   (R-IN-1; FM-P1). Continuation state lives in the parser; `finish()` flushes pending bytes
   as tokens without guessing ESC semantics (affirmed 0007).
3. **Bounded**: string-sequence payloads (OSC/DCS/APC/PM/SOS) buffer up to a configurable
   bound (default 64 KiB — provisional, chosen to clear known-large legitimate payloads like
   OSC 52 clipboard reads; to be retuned against conformance captures, reviewer m2); overflow
   switches the token to a `truncated: true` streaming form — prefix bytes delivered, tail
   counted-and-dropped, boundary explicit (FM-P2/FM-X6; the only place reconstruction is
   deliberately waived, and it says so on the token).
4. **Param fidelity**: CSI/DCS params keep raw bytes alongside parsed numbers; separator
   (`:` vs `;`) preserved; param-count overflow is a token flag, not silent merging
   (FM-P3; vte's `ignore` boolean is the anti-pattern).
5. **C1 forms recognized**: 8-bit CSI/OSC/DCS/ST per ECMA-48, as syntax — a decoder that
   drops an 0x9C-terminated OSC (vte's behavior) corrupts; recognition is not endorsement,
   and db fixtures cover both forms (audit's bogus-C1-fixture finding noted: fixtures come
   from spec text, not the prototype corpus).
6. **CAN/SUB abort** semantics per ECMA-48; aborted sequences emit as `Malformed` with bytes.

**Semantic layer — typed decoding over tokens**, table-fed from the database where mechanical
(finals, private markers, mode numbers): kitty keyboard (full `CSI…u` decode incl. event
types, alternates, associated text), legacy key encodings with their documented ambiguities
(FM-P11), SGR mouse + legacy tolerance (FM-P13) with **no scroll coalescing ever** (raw event
counts and timing pass through so apps can normalize per-terminal ratios; only resize
coalesces, and that in the session — FM-V6 vs R-IN-8, deliberately opposite policies), paste
bracket handling with `\r` normalization, focus, unsolicited notifications (mode-2031 theme
change / `CSI ? 997 n` reports as typed events — they are not correlated replies, reviewer
m6), and the report parsers the correlator consumes (design 03). Paste, precisely (reviewer
M7 — two mechanisms, not one): a **terminated** paste is delivered losslessly regardless of
size, segmented into bounded chunks with a `final` flag (rabbitui gets one event in the
normal case; a 1 MB paste arrives complete, just in pieces); the byte bound with its
`truncated` marker applies **only to unterminated accumulation** — a missing `ESC[201~` can
cost memory up to the bound and then degrades visibly, never hangs (FM-P12/FM-A8, R-IN-7
honored: legitimate large pastes are never truncated).
Unrecognized-but-well-formed tokens pass through as syntax events (`InputEvent::Csi/Osc/…`) —
the forward-compatibility contract: new protocols degrade to visible, lossless syntax, never
to fake keypresses (FM-P8, crossterm#834's inverse).

**ESC ambiguity** stays above the decoder: the session applies a configurable hold timer
(default ~25–50 ms, evidence range) to a bare-ESC-terminal token; kitty disambiguation, when
granted, bypasses the timer entirely (FM-P7).

## Text event payload (OQ-6 — spike-settled)

The spike (maintainer-local record, 19 fixtures × 3 candidate vocabularies) settled it:

- char-only events (crossterm/termina shape) split a ZWJ emoji into **5 unrelated events**
  with no togetherness marker and cannot carry modifiers with text (candidate A);
- a separate `ComposedText(String)` variant fixes batching but still **loses key/modifier
  association** the moment text appears, and turned out to be nobody's shipped terminal-side
  shape (candidate B — wezterm's `Composed(String)` lives in its *GUI* input types;
  termwiz-the-terminal-side is `Char(char)` only, source-verified);
- **text as a field on the key event** preserved association in *every* fixture and
  represents single codepoints, decomposed accents, jamo runs, and ZWJ clusters as one event
  each (candidate C — which is the kitty wire model itself). Provenance honesty (reviewer
  M1): the multi-codepoint fixtures (ZWJ family, jamo run) are **synthesized capacity tests**
  — the spec's grammar allows them but no emitter is confirmed to send them; the confirmed
  fixtures (precomposed é, emoji, CJK, shifted-with-text) all fit every candidate. The
  decision therefore rests on representational capacity plus association, not on observed
  multi-codepoint emission — and conformance captures will show what real terminals send.

**Decision: kitty-shaped key events.**
`KeyEvent { key, modifiers, kind: Press|Repeat|Release, text: Option<TextPayload> }` where
`TextPayload` is a small multi-codepoint-capable string (inline-optimized); legacy plain
UTF-8 input becomes a `KeyEvent` with `text` set and the trivial keycode. **Paste stays a
dedicated aggregated event** (`Paste`), not keyless text — this resolves candidate C's one
weakness (paste/keyless-text conflation) and matches the crossterm/termwiz precedent
consumers expect. The event mirrors the wire protocol's own semantics, so nothing is invented
that a fixture can't exercise. Grapheme segmentation stays out (rabbitui owns measurement;
mode 2027 is rendering-only). The spec's silence on *when* terminals emit multi-codepoint
text (quoted in the spike) is handled by capacity, not assumption: we can represent whatever
arrives, and conformance captures will document what real terminals actually send.

## Proof obligations

Fuzz: reconstruction, split-equivalence, no-panic (with malformed/CAN/C1/UTF-8-interleaved
dictionaries). Property: paste segmentation reassembles; decoder-then-encode round-trips db
fixtures. Corpus: escape-layer 12 cases + vte-spike criteria + FM-derived regressions
(vte#24/#46/#77/#81/#151 shapes as fixtures). The decoder is pure (sans-io) — all of this
runs without a terminal.

## Alternatives considered

vte / vtparse / fork-vte — measured and rejected above. **anes**: 3/13, discards unknowns,
dormant (FM-E7). **Single-layer decoder** (semantics straight from bytes, crossterm's shape):
rejected — forward compatibility then requires foreseeing every protocol, and the incumbents'
leak-fragments-as-text bugs (codex#7829 class) are the result. **Grapheme-segmented events**:
rejected per seam + mode-2027 evidence; revisit only if rabbitui's composer work surfaces a
concrete need.
