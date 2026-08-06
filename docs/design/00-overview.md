# qwertty Phase 2 design set — overview

Drafted 2026-07-06 against the maintainer-reviewed Phase 1 documents (maintainer-local
records: the requirements and failure-modes catalogs). Each document is one design
area with alternatives considered and the winning rationale; each is readable in a few
minutes; the set is readable in one sitting. **This set is gated: nothing proceeds to
Phase 3 without maintainer sign-off.**

| Doc                                                                | Area                                            | Headline decision                                                                                                                                                                                                             |
| ------------------------------------------------------------------ | ----------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [01-ownership.md](01-ownership.md)                                 | Terminal ownership & lifecycle                  | One owned session per device, `&mut` exclusivity, mode ledger driving all four exit paths, no globals, no library-installed hooks                                                                                             |
| [02-decoder.md](02-decoder.md)                                     | Input decoding                                  | Own the parser (fresh vte spike is decisive); total lossless syntax layer + typed semantic layer; kitty-shaped `KeyEvent` with multi-codepoint text field (OQ-6 settled)                                                      |
| [03-queries.md](03-queries.md)                                     | Query correlation                               | Sans-io correlator with typed full-discriminator expectations; DA1-fenced probe bundle; narrow typed public methods; proof plan = property/fuzz + PTY, no locks so no loom                                                    |
| [04-async.md](04-async.md)                                         | Async strategy                                  | Sans-io core, Tokio-first session (AsyncFd, no threads), blocking one-shot driver as front-end; no runtime traits until a second implementation exists                                                                        |
| [05-database.md](05-database.md)                                   | Sequence database                               | TOML per family, 9-field entries (incl. plain-English `description`), stable ids, refs+fixtures instead of confidence, support status = generated conformance results; `qdb` capture harness on the critical path             |
| [conformance-target-interface.md](conformance-target-interface.md) | Runner target (1-pager, ghostty-rs deliverable) | `feed/drain/state-probe` trait; in-process, PTY-headless, attended-GUI adapters                                                                                                                                               |
| [06-capabilities-policy.md](06-capabilities-policy.md)             | Detection & security                            | Tri-state findings with evidence provenance; identity first-class; explicit probe only; `Policy::restricted()` default, teachable denials                                                                                     |
| [07-platform.md](07-platform.md)                                   | Platform                                        | Unix tier 1; Windows seams now / implementation post-first-publish, VT-only surface; BSD later pass; pure layers check-built everywhere                                                                                       |
| [08-api-boundaries.md](08-api-boundaries.md)                       | Crates, modules, API shape                      | One published crate + unpublished tools; `TerminalDevice` trait with socketpair-backed `FakeDevice` (rabbitui seam, drives the real Tokio session); `geometry()` fallback ladder; vocabulary types are the ecosystem firewall |
| [fm-dispositions.md](fm-dispositions.md)                           | FM catalog accounting                           | Every FM-* entry mapped to its addressing mechanism or declared limit — the catalog's disposition contract discharged                                                                                                         |
| [adr-dispositions.md](adr-dispositions.md)                         | ADR 0001–0018 review                            | 12 affirmed, 4 revised (input-vocabulary cluster 0004/0005/0008 + query capacity 0012), 2 superseded (0006 arrow-parser, 0017 release content)                                                                                |

This table is the 2026-07-06 gate record; the set later grew two checkpoint additions,
[09-width.md](09-width.md) and [11-graphics.md](11-graphics.md) (2026-07-12). `10-windows.md`
(the Windows implementation plan) is a maintainer-local working record, not included here.

## Spikes run (evidence for the above)

- **vte spike** (maintainer-local record) — vte 0.15.0 against qwertty's decoder criteria.
  Verdict facts: APC/PM/SOS invisible, 8-bit-ST OSC vanishes, unbounded OSC under `std`,
  silent param truncation; corpus coverage and chunk-safety good. Grounds the own-parser
  decision ([02-decoder.md](02-decoder.md)).
- **Text-granularity spike** (maintainer-local record) — 19 CSI-u/legacy fixtures × 3 event
  vocabularies. Grounds the kitty-shaped KeyEvent decision ([02-decoder.md](02-decoder.md))
  and corrected a survey error (termwiz has no `Composed` variant terminal-side).

## Cross-cutting invariants (appear in multiple docs; listed once)

1. **Sans-io core**: decoder, correlator, ledger, capabilities are pure state machines;
   drivers inject bytes and time. This single property delivers runtime-neutrality (04),
   the blocking one-shot path (OQ-1), the in-memory test seam (08), and the proof plan (03).
2. **Evidence provenance everywhere**: capability findings, database entries, fixtures, and
   conformance results all carry where-they-came-from; nothing asserts above its evidence.
3. **Nothing implicit**: no probes at construction, no library-installed hooks, no
   auto-responses, no background threads.

## Verification run on this set (before the gate)

Two independent passes ran against the drafted set (2026-07-06): a **complete FM coverage
map** (every catalog entry located or flagged) and an **adversarial review** (2 critical,
8 major, 12 minor findings). All critical and major findings were fixed in place: the one-shot
recipe now uses only public layers (C1), `FakeDevice` is socketpair-backed so `AsyncFd`
genuinely works (C2), the DA1 fence drains the decode batch first (M5), the restore handle
got a double-buffered blob + loom obligation (M3/M8), `inline_insertion_safe` became a
conformance-derived capability (M4), paste segmentation vs truncation was disambiguated
(M7), per-cycle enter/leave never touches fd registration (M2), and the matrix's attended
cells get stale-marking (M6). The resulting accounting is
[fm-dispositions.md](fm-dispositions.md).

## Standing-obligation updates made with this set

- rabbitui: all 20 memo items + contract asks were dispositioned in `substrate-status.md` §6
  (nothing declined); the dispositions are preserved in maintainer-local records for
  re-placement in the peer drop-box folder (maintainer attention needed).
- ghostty-rs: [conformance-target-interface.md](conformance-target-interface.md) dropped in
  the peer folder; `protocol-status.md` restamped (database schema drafted, id scheme
  drafted — still do not cite ids until the Phase 2 gate).

## Phase 2 exit checklist (for the maintainer)

Read order: this page → 01 → 02 → 03 → 04 → 05 (+1-pager) → 06 → 07 → 08 →
adr-dispositions → fm-dispositions (skim; read its "Residual honesty" section).

**Gate record (maintainer review, 2026-07-06):**

- Design shape, naming/module map, default numbers, debug double-open check: **approved**.
- Schema (R-DB-5): maintainer's read surfaced a real gap — no plain-English "what does this
  do" field — fixed (`description` added, 05 amended) and the amended schema **signed off**.
- P15 (width/grapheme measurement): **decline REVERSED — qwertty owns terminal-aware width
  measurement** (maintainer call, stated as a narrow 60/40). Rationale in the maintainer's
  words: the right answer "comes from knowing deeply which terminal and version is in use,
  and that probably belongs at the terminal layer rather than the tui layer." This fits the
  machinery uniquely: width truth is a function of (identity, version, mode-2027 state) —
  the capability model's own keys — and the conformance runner can *measure* width behavior
  empirically (emit grapheme, read CPR) into the caniuse dataset, which no TUI-layer static
  table can do. Consequences: the non-goal is amended (requirements §2), the caps model
  gains a width-behavior finding, and the measurement *mechanism* (static tables vs
  measured-per-terminal vs hybrid, and the Unicode-data version policy) is a named Phase 3
  design item with a spike. Because the call is 60/40, the reversal is cheap-to-re-reverse
  until that spike lands — nothing else builds on it. **Seam change rabbitui must hear
  about**: its memo item 16 assumed measurement stayed on its side.

- Markdownlint config updated per maintainer request (aligned table delimiters,
  leading/trailing pipes; `work/` ignored) — an uncommitted working-tree change awaiting a
  jj commit.

**GATE CLOSED 2026-07-06 — the design set is maintainer-approved in full.** Phase 3 (salvage
audit + build plan) is unlocked; the salvage audit is published as
[prototype-salvage.md](prototype-salvage.md), and the build plan hits the brief's checkpoint 3
(build plan + publication timing) as a maintainer-local record before implementation begins.
