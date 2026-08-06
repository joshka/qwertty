# Phase 3 — Salvage disposition

What ports as-is, what ports with rework, what gets discarded — both codebases audited against
the approved Phase 2 design (2026-07-06, two independent audit passes; the fixture-level trust
map is a maintainer-local record that remains authoritative and is applied here, not re-derived).

## Part 1 — main (~2,700 src + ~1,800 test lines)

Verdict: almost nothing is delete-and-forget. The discards are type shapes and routing
internals the gate explicitly superseded; the behaviors and contracts around them survive.

| Bucket             | ~src lines | ~test lines | Contents                                                                                                                                                                                                                                                                                                                                                                                                        |
| ------------------ | ---------- | ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ports as-is        | 500        | 350         | `Command`/`CommandBuffer`/`ProtocolPosition`; `commands::{cursor,screen}` + escape helpers; `unsupported` platform stub; termios/nonblocking free fns; encode + device/session PTY test files and the PTY harness helpers (dedupe into one shared harness during the port)                                                                                                                                      |
| Ports with rework  | 1,700      | 1,460       | `Terminal`→`device::UnixTty` behind the trait (+ geometry validation); sync session→`Session` over the mode ledger; `InputDecoder` buffering discipline→syntax layer; `CsiInput` losslessness→all string families; CPR/DSR parsers→`report::` typed matchers; `TokioTerminalSession` AsyncFd I/O bodies + `next_event()`→new session over the correlator; decoder/query test contracts rewritten to the new API |
| Superseded/discard | 530        | small       | `InputEvent`/`KeyInput`/`ControlInput` vocabulary (replaced by kitty-shaped `KeyEvent` et al.); `QueryRouting` single-pending internals (replaced by the correlator); `InputBytes::events()` chunk-local dual path; the four-arrow parser (superseded ADR 0006)                                                                                                                                                 |

**The contract floor** (preserved regardless of implementation; design 03 names these):
output ordering; raw-bytes read; decoded-event delivery in order; query round-trip;
preserved-unrelated-input; timeout-preserves-events; wrong-report isolation; unmatched
query-shaped CSI never swallowed; EOF distinct from timeout; late reply becomes an event;
cancellation preserves everything; leave-restores-cooked. Contracts 4–11 exist ×2 (CPR and
DSR) — both instantiations are the floor. Plus the matcher contracts from input.rs
(first-match, duplicate-after-first, malformed-preserved).

**Audit flags → resolutions:**

- *A2 — `leave()` via `spawn_blocking`:* design 01 amended — teardown never routes through
  `spawn_blocking`; the non-blocking bounded-write discipline covers normal `leave()` too.
- *A3 — `request_*` pre-checks already-buffered events before reading:* design 03 amended —
  registering an expectation drains buffered undelivered events through the correlator first.
- *A1 — stateless one-shot classification:* no separate entrypoint; the one-shot recipe uses
  the stateful parser + `finish()` (design 02's EOF contract). Recorded here as the answer.
- *`TerminalSize` validation is a behavior change* at every current `size()` call site when
  the geometry ladder lands — expected, flagged for the port PR.
- *Device-trait seam confirmed:* `set_mode` carries raw/cooked only; richer modes are ledger
  command bytes, so the trait does not grow. Consistent as designed.

## Part 2 — prototype branch harvest inventory (closed list)

### Registry tables → `db/` (368 entries, 19 family files)

Field mapping: `id`→`id` (scheme already matches), `name`→`name`,
**`terminal_notes[0]`→`description`** (populated, one-sentence prose on nearly every entry —
the schema's new field is largely pre-written; osc/common is the thin spot, 19/68),
`direction`→`direction` (value spelling normalized), `syntax`→`syntax`,
`source_refs`→`refs` (re-resolved via `db/sources.toml`, each re-verified), `fixtures`→
`fixtures`, `response`→`responds`, `terminal_notes[1..]`→`notes`.
**Dropped on import:** `family`, `role`, `support_tier`, `owner_crate`, `capabilities`,
`policy`, `provenance`, `confidence` — all excluded by the approved schema.

| Family file             | Entries | Verdict                                                          |
| ----------------------- | ------- | ---------------------------------------------------------------- |
| dec/controls            | 19      | harvest-ready                                                    |
| ecma48/csi              | 72      | harvest h→t (64); 8 report-syntax strings re-derive via capture  |
| ecma48/syntax           | 10      | harvest-ready                                                    |
| iterm2/escape_codes     | 54      | best family (53/53 fixtures clean) — harvest; reports cautiously |
| kitty/color             | 5       | harvest h→t; report re-capture                                   |
| kitty/file_transfer     | 3       | **discard family** (schematic long keys, not wire format)        |
| kitty/graphics          | 8       | harvest h→t minus graphics_query/delete                          |
| kitty/keyboard          | 12      | harvest h→t minus named_key_f5/push; reports quarantine          |
| kitty/misc              | 3       | harvest minus ghostty_extension / notification_kitty_created     |
| kitty/multiple_cursors  | 7       | harvest — audit-confirmed real (kitty 0.43.0), byte-correct      |
| kitty/pointer           | 5       | harvest h→t; report re-capture                                   |
| osc/common              | 68      | harvest h→t (57; OSC numbering clean); reports quarantine        |
| rio/glyph               | 5       | **discard family** — invented syntax; rebuild from the real spec |
| vendor/advanced_dcs     | 5       | harvest minus decaupss (sixel/ReGIS/DECUDK framing clean)        |
| vendor/placeholders     | 7       | **discard** (placeholders incl. screen_passthrough)              |
| xterm/capabilities      | 8       | harvest h→t minus XTSMGRAPHICS; reports quarantine               |
| xterm/dec_private_modes | 18      | harvest h→t (mode numbers clean); mode reports quarantine        |
| xterm/input             | 41      | harvest h→t (mouse arithmetic clean); all 21 reports quarantine  |
| xterm/session           | 18      | harvest minus in_band_resize/save_modes/restore_modes/unscroll   |

### Fixtures → `db/fixtures/` (400 files; import = unescape + LF-normalize + re-verify)

Convention for FORMAT.md, mechanically confirmed: escaped text (`\e`, `\xNN`, `\\`); **all
395 non-rio fixtures end in a trailing LF that is not payload; all 5 rio fixtures don't**
(and rio is discarded anyway — the rule becomes uniform). Discard paths are enumerated in the
Part B audit (rio/*, kitty file_transfer group, ghostty_extension,
notification_kitty_created, graphics_query/delete, keyboard_named_key_f5/push,
screen_passthrough, unscroll, save/restore_modes, decaupss, XTSMGRAPHICS group,
in_band_resize group — `in_band_resize_query` is the DANGEROUS DECSLPP one — DA-tertiary and
XTWINOPS reports, all compat/ attributions, and **every terminal→host report fixture**: those
re-enter only via `origin=capture:`).

### Decoder test corpus

The 12-case escape-layer spike corpus (exact bytes recovered, incl. the `malformed` case
every incumbent failed) → the decoder's permanent test suite. The 14-entry `use_cases()`
round-trip table (query/report byte pairs: CPR, kitty, paste, mouse, focus, 2026, OSC 52/8,
theme) → correlator + decoder fixtures with `#! direction=` headers.

### Fuzz targets (9) → seeds/shape for the new proof obligations

`syntax_parser` (already chunk-loops — maps to reconstruction/split-equivalence),
`semantic_event_decoder` (no-panic, paste-state-terminates), `keyboard_events`,
`mouse_events` (no-coalescing), `osc_payloads`, `kitty_protocols` (minus file_transfer),
`query_registry` (correlator single-result/cancel/no-dup properties), `registry_toml`
(retargets to `qdb validate`). `policy_ratatui` — out of scope.

### Test ideas from the prototype session crate (150 inline tests scanned)

Contract-worthy ideas to re-derive (not port): duplicate-query coalescing + shared-result
fan-out + cancel-one-waiter semantics; family-scoped (non-greedy) reply matching; unmatched
reports stay app-visible; cancelled/timed-out queries never consume late replies; registry
state freed only after all waiters read; query-EOF distinct from transport EOF;
reverse-enablement cleanup order; skipped events restored FIFO to the front; resize
coalescer keeps latest-until-non-resize. **Gap worth noting: the prototype's
suspend/resume had zero dedicated tests** — ours are named in design 01's proof obligations.

### Also harvest / never harvest

Harvest: `registry/corpus/xterm-cursor.toml` as the capture-log template shape (the other two
corpus files audited wrong); bench scaffolding shapes; `source_refs` URLs to seed
`db/sources.toml`. Never harvest (closed): the 17-crate structure, doc shells
(`docs/spec/**`), `confidence`, the support-tier ladder, report fixtures, `compat/`
attributions, rio/file_transfer/placeholder families.
