# Failure-mode dispositions

The catalog's contract discharged: every FM entry from the failure-mode catalog, now
published as [docs/reference/failure-modes.md](../reference/failure-modes.md) (this table's
FM-* ids match the phase-1 catalog the published page evolved from), mapped to the design
mechanism that addresses it, or an explicit declared limit / decline. Produced by an
independent coverage pass over the design set, then updated after the adversarial review's
fixes landed (both 2026-07-06). Doc numbers refer to siblings in this directory.

Dispositions: **A** = addressed (mechanism named) · **D** = deferred-by-design (Windows: the
seam that must hold is named) · **L** = declared limit (surfaced/detected, not solved — with
reason) · **X** = declined (out of qwertty's seam — needs maintainer confirmation at the gate).

## Q — Query/reply integrity — all A

| FM  | Doc   | Mechanism                                                                                                                                                                                                                                                                                                                                   |
| --- | ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Q1  | 01,03 | Single `&mut` owner; correlator passes unmatched input through in order — nothing concurrent can steal a reply                                                                                                                                                                                                                              |
| Q2  | 01,03 | No locks exist; exclusivity is compile-time                                                                                                                                                                                                                                                                                                 |
| Q3  | 03    | Expectations removed at resolve; late/unsolicited replies become ordinary events                                                                                                                                                                                                                                                            |
| Q4  | 03,06 | Replies consumed from the decoded stream once; no implicit probes; dumb-terminal guards                                                                                                                                                                                                                                                     |
| Q5  | 03    | Matchers parse to the true terminator; bytes enter the decoder once — no stdin residue possible                                                                                                                                                                                                                                             |
| Q6  | 02,03 | Decoder split-equivalence reassembles replies across reads; interleaved input preserved in order                                                                                                                                                                                                                                            |
| Q7  | 03    | Fence resolution only after the full decode batch drains (named fixture)                                                                                                                                                                                                                                                                    |
| Q8  | 03    | Timeout/EOF/cancel are distinct resolutions; probe bundle bounds total wait                                                                                                                                                                                                                                                                 |
| Q9  | 03,06 | Caller-owned deadlines: every query/probe takes its budget as a parameter, nothing is hardcoded, so the catalog's too-short-fixed-timeout failure cannot originate in qwertty. **Downgraded 2026-07-12 (F5 audit):** the "identity-informed suggested budgets" half is unbuilt — no such helper exists in code; building it is future work. |
| Q10 | 03    | Full-discriminator matchers; overlapping pending matchers are a checked error                                                                                                                                                                                                                                                               |
| Q11 | 03    | Late reply can never complete a later query (main's contract kept)                                                                                                                                                                                                                                                                          |
| Q12 | 01    | Session opens `/dev/tty`, never stdio wrappers                                                                                                                                                                                                                                                                                              |
| Q13 | 03    | CPR/F3 collision: match only when disambiguation or param shape excludes the key form, else serialize                                                                                                                                                                                                                                       |
| Q14 | 03    | Duplicate identical queries coalesce with waiter counts                                                                                                                                                                                                                                                                                     |

## L — Lifecycle — all A except L13 (downgraded to unbuilt, 2026-07-12)

| FM  | Doc   | Mechanism                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| --- | ----- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| L1  | 01    | `&mut`-free restore handle for app-installed hooks; worker-thread/abort gaps documented                                                                                                                                                                                                                                                                                                                                                                               |
| L2  | 01,06 | Ledger pops everything; exit blob is stronger-than-pop (`CSI < u`); granted flags recorded                                                                                                                                                                                                                                                                                                                                                                            |
| L3  | 01    | Cursor state is a ledger entry with undo bytes                                                                                                                                                                                                                                                                                                                                                                                                                        |
| L4  | 01    | Atomic "restored" flag; Drop is panic-aware and skip-if-restored                                                                                                                                                                                                                                                                                                                                                                                                      |
| L5  | 01    | `leave()` explicitly flushes                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| L6  | 01    | Ledger replays exactly what was set — partial setup unwinds cleanly                                                                                                                                                                                                                                                                                                                                                                                                   |
| L7  | 01    | Emergency handle: preallocated blob, `write(2)`+`tcsetattr` only                                                                                                                                                                                                                                                                                                                                                                                                      |
| L8  | 01    | Every cleanup step attempted; first error reported                                                                                                                                                                                                                                                                                                                                                                                                                    |
| L9  | 01    | Handoff/resume resync termios instead of trusting cache                                                                                                                                                                                                                                                                                                                                                                                                               |
| L10 | 01    | Per-line enter/leave = ledger replay, no probing, no fd re-registration                                                                                                                                                                                                                                                                                                                                                                                               |
| L11 | 01    | No controlling tty → typed error, never panic/hang                                                                                                                                                                                                                                                                                                                                                                                                                    |
| L12 | 01    | Teardown writes non-blocking with bounded retry — a stalled mux degrades, never hangs                                                                                                                                                                                                                                                                                                                                                                                 |
| L13 | 01,06 | **Unbuilt (downgraded 2026-07-12, F5 audit):** the designed mechanism — changed OSC colors ledgered with captured originals — has no delivering code. No OSC color *setter* exists in the typed API (only the OSC 10/11 query builders), so no ledger entry exists either; an app that sets OSC 11 through the raw-bytes hatch gets no capture/restore. The failure is currently unreachable through typed API, not addressed; setter + ledger entry are future work. |
| L14 | 01    | One fd pair on the controlling terminal — no split-stream assumptions                                                                                                                                                                                                                                                                                                                                                                                                 |

## A — Async — all A

| FM     | Doc   | Mechanism                                                                                                     |
| ------ | ----- | ------------------------------------------------------------------------------------------------------------- |
| A1–A5  | 04    | AsyncFd readiness, no helper threads/tasks; per-await-point cancel-safety tests; state in session not futures |
| A6     | 01,04 | Handoff pause is full fd release; buffered events survive                                                     |
| A7     | 08    | `TerminalDevice` seam; `FakeDevice` substitutable                                                             |
| A8     | 02    | Bounded accumulation with visible truncation; terminated pastes segment losslessly                            |
| A9–A10 | 04    | Event-driven readiness; no poll cadence, no self-managed ready-fd                                             |

## G — Signals/resize

| FM  | Disp  | Doc | Mechanism / limit                                                                                              |
| --- | ----- | --- | -------------------------------------------------------------------------------------------------------------- |
| G1  | A     | 01  | Suspend/resume ops + optional Tokio signals helper; resize as events                                           |
| G2  | A     | 01  | Session coalesces pending resizes to one final-geometry event                                                  |
| G3  | A     | 01  | Restore-then-`kill(0, SIGTSTP)` — process group                                                                |
| G4  | A     | 01  | Bounded retry re-claim; termios resync; optional stale-input flush; synthetic Resize                           |
| G5  | A     | 01  | qwertty installs no handlers — nothing accumulates                                                             |
| G6  | **L** | 01  | qwertty delivers the resize event and CPR primitive; repaint correctness is the renderer's (rabbitui/app seam) |
| G7  | A     | 01  | Degenerate process groups error, don't crash                                                                   |

## Z — Size — all A

| FM    | Doc   | Mechanism                                                                                             |
| ----- | ----- | ----------------------------------------------------------------------------------------------------- |
| Z1    | 01,08 | `geometry()` reads the tty device fd, never stdout                                                    |
| Z2    | 08    | Degenerate values (0×0, u16::MAX) rejected as errors                                                  |
| Z3–Z4 | 08    | Documented fallback ladder: ioctl → `CSI 18 t` (probe hygiene) → env (`Inferred`) → typed unknown     |
| Z5    | 08,05 | Pixels-if-known shape; `CSI 14/16 t` as database queryables — absence representable, never fake zeros |

## V — Inline viewports/scrollback

| FM    | Disp  | Doc   | Mechanism / limit                                                                                                                                                                               |
| ----- | ----- | ----- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| V1,V2 | A     | 06,05 | `inline_insertion_safe` conformance-derived finding gates DECSTBM/RI helpers; `ScrollbackLine` probe is the oracle; portable insertion *recipe* is a named Phase 3 deliverable from matrix data |
| V3    | **L** | 05,06 | Per-terminal clear quirks live in conformance results; `CommandBuffer` composes atomic blobs (codex one-write pattern) — a canonical clear recipe joins the Phase 3 deliverable above           |
| V4    | A     | 06    | 2026 emission is capability-gated; framing helpers make out-of-frame writes hard                                                                                                                |
| V5    | **L** | 01    | No portable re-anchor signal exists (codex's own finding); qwertty provides CPR + resize events — the heuristic is necessarily the app's                                                        |
| V6    | A     | 02    | Scroll events pass through at full fidelity, never coalesced; caniuse records per-terminal tick ratios; normalization is app-side by declared limit                                             |
| V7    | A     | 06,05 | Per-mux/terminal insertion quirks are identity-keyed conformance data, not match arms                                                                                                           |

## P — Parser/decoder

| FM          | Disp          | Doc              | Mechanism                                                                                                                                                                                                                                                                                 |
| ----------- | ------------- | ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| P1–P14, P16 | A             | 02               | Total lossless syntax layer (reconstruction + split-equivalence + bounded invariants); malformed→tokens; ESC timing above the decoder; kitty/legacy/mouse/paste semantic decoding; both BEL/ST; C1 recognized; kitty-shaped KeyEvent with multi-codepoint text (P16)                      |
| P15         | A (direction) | 06 + gate record | Decline reversed at the gate (60/40): width is terminal-behavior knowledge — identity/version/2027-keyed — so qwertty owns terminal-aware width measurement; conformance runner measures real width behavior (grapheme + CPR) into caniuse; mechanism = named Phase 3 design item + spike |

## C — Capability detection — all A

| FM      | Doc   | Mechanism                                                                                                                |
| ------- | ----- | ------------------------------------------------------------------------------------------------------------------------ |
| C1–C3   | 06    | Query-first with evidence provenance; identity + `mux_stack` first-class; terminfo demoted to (unused) inference at most |
| C4      | 03,06 | DA1 = shape-tolerant fence, never a feature oracle; unknown ≠ unsupported                                                |
| C5–C7   | 03    | Dumb-terminal skip; single-write single-deadline bundle; nothing probes implicitly                                       |
| C8      | 06    | Per-attachment findings; stale-marking on resume/reattach/2031; explicit re-probe                                        |
| C9–C10  | 05,06 | Support claims machine-generated only; emit-gating forbids TERM-sniff emission                                           |
| C11     | 06    | Verify-after-push; ledger stores granted reality                                                                         |
| C12–C13 | 06    | Labeled env-heuristic table; per-attachment capability lifetime                                                          |

## M — Multiplexer/remote

| FM  | Disp  | Doc   | Mechanism / limit                                                                                                                                     |
| --- | ----- | ----- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| M1  | A     | 06    | Passthrough emitted only when mux detected *and* policy allows                                                                                        |
| M2  | **L** | 06    | tmux gating modeled; mosh's `c`-only/single-packet OSC 52 is an identity-keyed declared transport limit                                               |
| M3  | A     | 06    | mosh treated as its own terminal identity                                                                                                             |
| M4  | A     | 03    | Fence + one deadline: swallowed queries resolve `NoReply`, never hang                                                                                 |
| M5  | A     | 06    | Outer-terminal inference labeled; latency budget caller-owned (the identity-informed suggestion helper is unbuilt — same downgrade as Q9, 2026-07-12) |
| M6  | **L** | 06    | Mid-session mux re-encoding loss is detected (granted-set verify, stale-marking), not preventable — transport is lossy and the design says so         |
| M7  | A     | 01,06 | Per-attachment caps + resume re-probe; no atomic-reset assumptions on reattach                                                                        |

## W — Windows — all D (deferred-by-design, per resolved OQ-2)

| FM  | Seam that must hold (design 07)                                                  |
| --- | -------------------------------------------------------------------------------- |
| W1  | ConPTY as a conformance identity — passthrough gaps become runner output         |
| W2  | Decoder syntax layer + policy gate accommodate all-CSI input mode                |
| W3  | Event vocabulary is decoder-independent — INPUT_RECORD-assisted path can feed it |
| W4  | Ledger entries are undo *operations*, not termios — console-mode restore fits    |
| W5  | Database-fed semantic decoder gains Windows fixtures without core changes        |
| W6  | Emit-gating keys on conhost/WT identity — unsupported encodings never emitted    |
| W7  | Day-one Windows check-build CI keeps the seams from rotting pre-implementation   |

## X — Security — all A

| FM    | Doc   | Mechanism                                                                          |
| ----- | ----- | ---------------------------------------------------------------------------------- |
| X1–X2 | 03,06 | qwertty never auto-responds — the echoback/answerback class can't originate here   |
| X3    | 06    | Title sanitization on by default (control + bidi blocklist)                        |
| X4    | 06    | Clipboard read off by default; write policy-gated best-effort                      |
| X5    | 02,06 | Paste as data, `contains_control()` surfaced, optional strip; bounded accumulation |
| X6    | 02    | Bounded reply/sequence buffers with visible truncation                             |
| X7    | 06    | Passthrough/control-mode policy-gated with teachable denials                       |
| X8    | 05    | Fixture `replay` classes; runner refuses modal/destructive without opt-in          |

## E — Ecosystem/process — all A or process-addressed

| FM         | Doc          | Mechanism                                                                                  |
| ---------- | ------------ | ------------------------------------------------------------------------------------------ |
| E1         | 08           | No third-party types in the public API; vocabulary types as firewall                       |
| E2, E4, E7 | 02,04        | Own the parser; no vendored cores; no traits over zero implementations                     |
| E3         | 08 + R-REL-2 | Minimal deps, multi-target check-builds (process + posture)                                |
| E5         | 05           | No confidence field; refs+captures are the only trust path; direction-aware evidence rules |
| E6         | 00 + process | The design set's own size discipline + maintainer gates                                    |
| E8         | 08,04        | Socketpair `FakeDevice` + sans-io core: downstream CI needs no PTY answering CPR           |

## Residual honesty

Five declared limits (G6, V3, V5, M2, M6) — each surfaced with evidence and reasoning rather
than solved, because the failure lives in the renderer seam or in a lossy transport qwertty
cannot fix from the app side. The one decline that was pending maintainer confirmation
(**P15**, width measurement) was instead **reversed at the gate**: qwertty owns
terminal-aware width measurement, mechanism designed in Phase 3. The Windows category is
wholly deferred-by-design per resolved OQ-2, with named seams. Nothing in the catalog is
unaddressed.

2026-07-12 honesty pass (from the F5 requirements audit): Q9's "identity-informed suggested
budgets" clause, its M5 echo, and L13's "OSC colors ledgered with captured originals" each
claimed a mechanism with no delivering code. All three downgraded above to state what actually
exists (caller-owned deadlines; no OSC color setter or ledger entry at all); building the
mechanisms is future work, tracked in the audit's found-work backlog.
