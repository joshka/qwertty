# Design 11 — Graphics (inline images)

The public surface, policy gating, capability model, and lifecycle for terminal image protocols;
the protocol scope decision; and the G-impl milestone cut. Extends design 05 (database), design 06
(capabilities/policy), and design 08 (command families); realizes plan checkpoint 3 (G1 graphics
design). Governed by ADR 0019 (vocabulary freeze), which explicitly leaves **new protocol families
additive and unrestricted** — so a `commands::graphics` family is permitted without a freeze
reversal.

**Status: CHECKPOINT — awaiting maintainer approval (phase-5 checkpoint 3). No implementation
lands before the answer.** Drafted 2026-07-12, windows lane (D-queue complete, took G1 off the
backlog per the plan). Task files are maintainer-local records.

## Decision summary

1. **Scope, in priority order: kitty graphics first, iTerm2 inline images second, sixel deferred
   (explicit P2).** kitty is the only protocol with an honest support probe (it answers), the most
   capable (placement, deletion, animation), best-specified, and already has a `db/kitty-graphics`
   seed. iTerm2 images are a simple identity-keyed second. Sixel is deferred because its only
   "capability signal" (the DA1 sixel attribute) is provably unreliable (FM-C9: zellij hard-codes
   it; FM-E5: the corpus faked it) — shipping it needs conformance-measured support, which is
   runner work, not a cheap probe.
2. **Surface = `commands::graphics`, data-first, one submodule per protocol — no unifying "render
   an image" abstraction yet.** Each protocol's commands encode to bytes without a device (R-OUT-1)
   and compose through `CommandBuffer`. A cross-protocol convenience is a *later* layer, added once
   ≥2 protocols exist and a consumer asks (second-implementation rule; narrow-primitive rule —
   never force-combine protocols that differ in real ways).
3. **Policy gates resource-naming transmission, not inline bytes.** Sending image bytes the app
   already holds (kitty *direct*, iTerm2 inline base64) is capability-gated but not policy-gated —
   it opens no new resource. Transmission modes where the *escape names a resource the terminal
   opens* (kitty *file* / *temp-file* / *shared-memory*) are gated behind `Policy` — they are a
   local-file-read / exfil primitive, exactly the "file transfer" gate design 06 already reserves.
   Default `restricted()` = off; denial is a typed `PolicyDenied` naming the gate.
4. **Capability = honest per-protocol provenance.** kitty: probe-by-response (`a=q` query →
   `OK`/error, verify-after-push, the kitty-keyboard pattern already built) → `Probed`. iTerm2:
   no query exists → `TerminalIdentity`-keyed → `Inferred`. sixel: never trust DA1 →
   `Conformance` only. Cell pixel geometry (needed to size placements, FM-Z5) is a `Finding`
   from `CSI 14 t`/`CSI 16 t`, honestly `Unknown` when the terminal reports zeros (most do).
5. **Placed images are app-owned content, not session state.** They are not entered in the mode
   ledger and not restored on `leave`/drop (no more than emitted text is). qwertty provides
   explicit delete/clear commands; it never auto-clears graphics on teardown and never auto-responds
   to a graphics query (R-SEC-2).
6. **Milestones: G2 (kitty graphics) → G3 (iTerm2 images) → G4 (sixel, P2/deferred).** Task
   files are maintainer-local records.

## Q1 — Surface: which protocols, and the command/decode shape

**The three candidate protocols and the call on each.**

| Protocol          | Wire                             | Probe?                 | Capability                     | Verdict              |
| ----------------- | -------------------------------- | ---------------------- | ------------------------------ | -------------------- |
| kitty graphics    | APC `ESC _ G <ctrl>;<b64> ESC \` | yes — `a=q` → response | placement, deletion, animation | **G2 (first)**       |
| iTerm2 inline img | OSC 1337 `File=<args>:<b64>`     | no                     | one-shot, no control           | **G3 (second)**      |
| sixel             | DCS `q ... ST`                   | DA1 attr (unreliable)  | palette, cell-based            | **G4 — deferred P2** |

kitty leads because it is the only one that lets qwertty *ask* and get a truthful answer, which is
the whole capability discipline (design 06: probe over sniff). iTerm2 images are trivial to emit
but undetectable except by identity. Sixel's DA1 attribute is the canonical capability-lie (FM-C9)
and was fabricated in the prototype corpus (FM-E5), so honest sixel support is conformance-measured
— high cost, low near-term payoff; deferred until a consumer asks.

**Command family (design 08 rules).** A new `commands::graphics` module, submoduled per protocol
(`graphics::kitty`, `graphics::iterm2`, later `graphics::sixel`), each helper a data value that
encodes to bytes with no device. Additive under ADR 0019. The huge base64 payloads are just bytes
in the encoded blob; `CommandBuffer` composes them and the raw-bytes escape hatch stays. No
third-party image types in the public API (R-OUT-1 firewall) — helpers take `&[u8]`
encoded-payload plus typed control parameters; *encoding a pixel buffer to PNG/whatever is the
caller's job*, not a re-exported `image` crate dependency (the reedline#456 lesson).

**Decode side.** The syntax layer already parses APC, OSC 1337, and DCS losslessly as bounded
`StringSequence`/`ControlSequence` tokens — no new syntax work. New semantic parsers:

- **kitty graphics response** (`report::`): the `ESC _ G i=<id>,p=<pid>;OK ESC \` / error form → a
  typed response feeding a correlator `Expectation::KittyGraphics` (mirrors `KittyKeyboardFlags`).
  The `db/kitty-graphics` family already seeds `OK` and error-response fixtures.
- iTerm2 and sixel have no host→app response to decode (fire-and-forget), so they add no decode
  surface — only encode + capability.

## Q2 — Policy: gate resource-naming transmission, not inline bytes

The exfil surface is not "images" — it is **which transmission modes let the escape stream name a
resource the terminal will open.**

| Transmission mode            | Who supplies the bytes        | New resource access? | Gating                   |
| ---------------------------- | ----------------------------- | -------------------- | ------------------------ |
| kitty **direct** (`t=d`)     | app (inline base64)           | none                 | capability only          |
| iTerm2 inline (`File=…`)     | app (inline base64)           | none                 | capability only          |
| kitty **file** (`t=f`)       | terminal reads a path         | **local file read**  | `Policy` (file transfer) |
| kitty **temp file** (`t=t`)  | terminal reads+unlinks a path | **local file read**  | `Policy` (file transfer) |
| kitty **shared mem** (`t=s`) | terminal opens a shm object   | **local IPC read**   | `Policy` (file transfer) |

**The split (narrow-primitive + policy).** The single-operation primitives are always exposed:
`transmit_direct(bytes, …)` and iTerm2 `inline_image(bytes, …)` need no policy — they carry
bytes the app already owns and open nothing. The resource-naming primitives —
`transmit_file(path)`, `transmit_temp_file`, `transmit_shared_memory` — are gated: the
session-level emit takes the capability finding *and* consults `Policy`; a denied call returns
typed `Error::PolicyDenied` naming the gate, whose rustdoc cites the attack class (a malicious
payload steering the terminal to read `/etc/…` or a secret file and render/exfil it). This
reuses design 06's existing **file transfer** gate (default off in `restricted()`); no new
policy field is required, though the design may name a `graphics_file_transmission` sub-gate if
the maintainer wants finer control than the umbrella `file transfer` toggle — a checkpoint
question.

This honors the narrow-primitive rule (the raw single-op transmit is always available; only the
risky resource-naming combos gate) and surfaces every denial as an inspectable typed error rather
than a silent drop (visible provenance over invisible fallback).

iTerm2's OSC 1337 carries many non-image subcommands (`StealFocus`, `SetProfile`, clipboard, …);
the graphics surface emits **only** the image subcommand — the rest are out of scope here (some
belong to other gated families, e.g. clipboard already has its OSC 52 gate).

## Q3 — Capability: honest provenance per protocol, and pixel geometry

- **kitty graphics — `Probed`.** A query transmit (`a=q`, a 1×1 probe or a real transmit with the
  query action) elicits a response the correlator matches, exactly the verify-after-push shape
  kitty keyboard already uses (design 06). Support enters `Capabilities` as a `Finding` sourced
  `Probed { via: <the query SequenceId> }`. Silence is `Unknown`, never `unsupported` (FM-C4) — a
  mux may have swallowed it.
- **iTerm2 images — `Inferred`.** No query exists, so support is `TerminalIdentity`-keyed
  (`TERM_PROGRAM=iTerm.app`; WezTerm supports both kitty *and* iTerm2, so identity may enable more
  than one). Labeled `Inferred { via: TERM_PROGRAM }` — honestly weaker evidence than a probe.
- **sixel — `Conformance` only.** The DA1 sixel attribute is not trusted (FM-C9/FM-E5). If sixel
  ships (G4), its finding is sourced from checked-in conformance results (the runner renders a
  sixel and verifies), looked up by identity — the same discipline `inline_insertion_safe` uses.
  This is precisely why sixel is deferred: there is no cheap honest signal.
- **Pixel geometry (FM-Z5).** Placing/sizing an image needs the cell pixel size. `ResizeEvent`
  already carries optional pixel geometry; add `report::` parsers for `CSI 14 t` (window pixels)
  and `CSI 16 t` (cell pixels) and expose them as `Finding`s. Most backends report zeros
  (prototype backend, ratatui WindowSize) — that is surfaced as `Unknown`/absent, never a
  fabricated default; the app reads the finding before sizing and decides its own fallback.

**Emit-gating (R-CAP-4).** Every graphics emit helper at the session layer takes the capability
finding (or an explicit `Override::Force`) so an image protocol can never leak into a terminal that
never answered — the same rule that stops 2026 wraps leaking onto a console (FM-V4).

## Q4 — Lifecycle: images are content, not session state

- **Not ledgered, not restored.** A placed image is output content, like emitted text — it does
  not enter the mode ledger and is not replayed or undone by `leave`/drop/`RestoreHandle`.
  Restoring "the images that were on screen" is not a coherent operation; the app owns its
  render loop.
- **Explicit cleanup, never automatic.** qwertty provides delete/clear commands (kitty
  `a=d` delete-by-id / delete-all / clear-placements; iTerm2 and sixel have no delete — documented
  as a protocol limitation, not a qwertty gap). Teardown does **not** auto-clear graphics: an
  alternate-screen app's `1049l` clears its screen already, and a primary-screen app owns its
  cleanup. This matches "qwertty never auto-responds / auto-manages content" (R-SEC-2).
- **Emergency-restore blob unchanged.** The panic blob restores *modes*, not content; it neither
  clears nor needs to clear images. Documented so the stance is explicit, not an oversight.
- **kitty image IDs are app-namespaced.** The API surfaces image/placement IDs as caller-chosen
  values (the protocol's client-assigned id space); qwertty does not allocate or track them — the
  app that transmits owns the id lifecycle, and the delete helpers take the same ids. No hidden
  registry (avoids the prototype's un-grokkable-registry failure mode).

## Q5 — Milestones: the G2/G3/G4 cut

| Slice | Contents                                                                                                                                                                                                                                                                                                                                         | Exit criteria (task files normative)                                                                                                         |
| ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------- |
| G2    | `commands::graphics::kitty` (transmit direct, place, delete); kitty graphics response decode + correlator `Expectation`; `Probed` capability finding + session probe; `CSI 14t/16t` pixel-geometry findings; `Policy` gate on file/temp/shm transmit; `db/kitty-graphics` verified/extended (cited spec only); fixtures; example; reference page | probe round-trips vs a real kitty-graphics terminal (capture); policy-denied test for file transmit; no-fabrication held; gates + fuzz green |
| G3    | `commands::graphics::iterm2` inline image (OSC 1337, inline base64 only); identity-keyed `Inferred` finding; capability-gated emit; docs (incl. the WezTerm dual-protocol note)                                                                                                                                                                  | fixtures; identity-gating test; example renders under iTerm2/WezTerm (attended or captured)                                                  |
| G4    | sixel — **deferred, explicit P2.** Only on a consumer ask: `commands::graphics::sixel` encode, conformance-measured capability (never DA1-trusted), palette/cell docs                                                                                                                                                                            | (not scheduled; listed so the scope boundary is explicit)                                                                                    |

Continuous: a `docs/reference/graphics.md` concept page (protocol comparison, the policy split, the
capability-provenance table, the pixel-geometry honesty rule) and caniuse rows as targets gain
graphics support through the runner.

## Constraints compliance

- **ADR 0019**: `commands::graphics` is a *new protocol family* — additive and unrestricted per
  the freeze's own terms; no `Event` vocabulary change (graphics responses are `report::` types +
  correlator expectations, not new `Event` variants — the kitty-keyboard precedent).
- **Design 06 policy**: reuses the existing `file transfer` gate; default `restricted()` keeps
  file/temp/shm transmit off; denials are teachable typed errors citing the FM-X class.
- **Design 05 database / no-fabrication**: every `db/kitty-graphics` and future sixel/iTerm2 entry
  comes from a cited spec (kitty graphics protocol doc, iTerm2 proprietary-escape docs, the sixel
  spec) or a capture — never invented, and terminal→app response syntax stays quarantined until
  captured (the discipline that keeps the 67 quarantined entries honest).
- **Narrow primitives**: raw single-op transmit/place/delete always exposed; no forced
  cross-protocol combiner; convenience deferred to a second implementation.
- **No third-party types in the public API**: helpers take encoded bytes + typed params; no `image`
  crate re-export (R-OUT-1 / reedline#456).

## Alternatives considered

- **A unifying `Image` abstraction over all three protocols now**: rejected — the protocols differ
  in real ways (placement, deletion, animation, response) a lowest-common-denominator type would
  erase; build protocol-faithful primitives first, layer convenience once ≥2 exist.
- **Trusting the DA1 sixel attribute for capability**: rejected — FM-C9/FM-E5 show it is the
  archetypal capability-lie; sixel support must be conformance-measured or not claimed.
- **Gating all graphics behind policy**: rejected — inline transmission opens no new resource;
  gating it would punish the common, safe case and push apps to the raw-bytes escape hatch,
  defeating the gate. Gate the resource-naming modes only.
- **Auto-clearing images on teardown / tracking them in a registry**: rejected — images are
  app-owned content; a hidden registry is the prototype's un-grokkable pattern. Explicit delete
  commands + documented app ownership instead.
- **Re-exporting an image-encoding crate for convenience**: rejected — ecosystem-lockstep firewall
  (R-OUT-1); the caller encodes pixels to a wire format and hands qwertty bytes.

## Checkpoint asks (maintainer)

1. **Approve the protocol scope + order**: kitty graphics (G2) → iTerm2 images (G3) → sixel
   deferred P2 (G4).
2. **Approve the policy split**: inline transmission capability-gated only; kitty file/temp/shm
   transmission gated behind `Policy`. Decide **umbrella `file transfer` gate vs a dedicated
   `graphics_file_transmission` sub-gate** (design leans umbrella-reuse; maintainer decision).
3. **Approve the capability approach**: kitty `Probed`, iTerm2 `Inferred`, sixel `Conformance`-only,
   pixel geometry as an honest `Finding` (zeros → `Unknown`).
4. **Confirm the lifecycle stance**: images are app-owned content — not ledgered, not
   auto-restored, not auto-cleared; explicit delete helpers; caller-owned kitty image IDs.
5. **Confirm no-fabrication** for all graphics wire syntax (db from cited specs/captures only).

Until answered, the graphics thread is stopped per the protocol.
