# Design 03 — Query correlation

How replies are matched; concurrent/pipelined behavior; cancel-safety; timeouts; late
replies; the race-freedom proof plan. Addresses FM-Q1–Q14, FM-C6–C8; implements R-QRY-1–7.

## Decision

**A sans-io correlator: a pure state machine holding a set of typed expectations, fed decoded
events, emitting (completions | passthrough events).** No I/O, no clock, no locks — time and
bytes are injected by the driver (Tokio session, blocking one-shot loop, or a test).

```text
Correlator state:
  expectations: Vec<Expectation>        // small-N ordered set, not a HashMap
  Expectation { id, matcher: TypedMatcher, policy: OnMatch }

feed(event) -> Outcome:
  first expectation whose matcher fully matches consumes the event -> Completion(id, parsed)
  no match -> Passthrough(event)        // ordinary input, in order, never dropped
resolve(id, Timeout | Eof | Cancelled)  // driver-injected; expectation removed
```

Key rules, each pinned to a failure mode:

1. **Full-discriminator matching.** A matcher matches on the complete identity of the reply:
   DECRPM matchers carry the mode number (the prototype's cross-completion bug, FM-Q10); OSC
   color matchers carry the color index; XTWINOPS matchers the report kind. A matcher that
   cannot distinguish its reply from another pending expectation's is a **type-level error**:
   `Expectation::new` rejects (or serializes behind) an overlapping pending matcher.
   Overlap is decided by a static `distinguishes(a, b)` relation over matcher types — small,
   enumerable, unit-tested per query pair.
2. **Ambiguity policy per query type, stated in the database.** CPR (`CSI r;c R`) collides
   with modifier-F3 (FM-Q13): the CPR matcher therefore only matches when kitty
   disambiguation is active or the parameter shape excludes the key form; otherwise CPR
   queries *serialize* with a documented note. Each queryable sequence's database entry
   records reply syntax + collision notes; matchers are generated/checked against it.
3. **Duplicate identical queries coalesce** to one expectation with a waiter count (FM-Q14;
   the prototype's one verified-good idea here, re-derived: two `background_color()` calls
   want the same answer and terminals answer once per probe).
4. **Late replies can never complete a later query.** Expectations are removed at resolve
   time; a reply arriving after its expectation is gone is a Passthrough event (FM-Q3/Q11 —
   main's tested contract, kept). Unsolicited query-shaped input is always Passthrough
   (reedline#856's monitoring-software CPRs).
5. **Typeahead and interleaved input are Passthrough in arrival order** — buffered by the
   driver for later `next_event()` delivery, never reordered relative to each other (FM-Q1,
   FM-Q6). The codex resume-probe case (focus report interleaved with CPR reply) is a fixture.
6. **EOF/timeout/cancel are distinct resolutions** surfaced as distinct errors (FM-Q8);
   drivers own deadlines (R-QRY-3) so the blocking driver can use `poll(2)` and the Tokio
   driver `tokio::time` against the *same* correlator.

## The probe bundle (R-QRY-4)

A `Probe` is a precomposed set of expectations + one write blob + a DA1 fence expectation:
single `write(2)`, one deadline. Three fence/ordering rules with named victims (reviewer
M5/m7; salvage flag A3): the fence resolves still-pending expectations as `NoReply` **only
after the entire current decode batch has been fed to the correlator** — DA1 and a slower
reply arriving in one read() must both land before the fence acts (FM-Q7: notcurses missed a
CPR sitting behind DA1 in the same buffer); the DA1 fence matcher matches the report *shape*
(`CSI ? … c`), tolerating any parameter count/values (FM-C4: tmux widening `?1;2c` to
`?1;2;4c` hung notcurses' parser); and registering any expectation **first drains
already-buffered undelivered events through the correlator** — a reply that arrived
interleaved with earlier typeahead and is sitting in the session's event queue must be able
to complete the query before any new read happens (main's `request_*` methods do exactly
this pre-check today; the salvage audit flagged it as a real ordering hazard worth naming,
not just an implementation detail).
`NoReply` means supported=unknown — DA1 is a fence, not a feature oracle. Default
bundle: DA1, XTVERSION, DECRQM {2026, 2027, 2048, 2004, 1049?}, `CSI ? u`, OSC 10/11. Budget
default ~150 ms locally, caller-owned (codex ships 100 ms; crossterm's 2 s is the
anti-pattern — FM-C6); the ssh/mosh story is a longer caller-chosen budget, not a longer
default (FM-Q9 vs startup latency). Probes never run implicitly (FM-C7/R-QRY-5); dumb-terminal
guards (`TERM=dumb`, Linux console ioctl) skip the write entirely (FM-C5).

Public API stays **narrow typed methods** (`cursor_position()`, `probe_capabilities()`,
`background_color()`) — no public router/registry; every surveyed consumer wants the
question, not the machinery. The correlator is `pub(crate)`. **Helix's "decisive feature" —
public filtered reading of arbitrary escape replies — is delivered structurally, not
deferred** (reviewer m12): every reply qwertty lacks a typed method for arrives through
`next_event()` as a lossless preserved syntax event (design 02's forward-compat contract),
so an app can filter for any reply itself with nothing stolen or reordered — which is
strictly stronger than termina's filtered read. What *is* deferred until demonstrated demand
is only the convenience form (`wait_for(matcher)` wrapping an `Expectation`); the capability
itself is P0 via the event stream. The blocking one-shot recipe likewise uses public layers
only (decoder + report parsers — design 04 §3).

## Reply hygiene

Matchers parse to the true end of a reply including its terminator, accepting BEL and ST
forms (FM-P9); payload parsing is bounded (FM-X6); nothing is ever echoed or auto-answered
(R-SEC-2). Replies are consumed from the decoded-event layer, so the "leftover `ESC \` in
stdin" class (FM-Q5) is structurally impossible: bytes enter the decoder once, in order.

## Race-freedom proof plan (designed with the mechanism, per the brief)

The claim decomposes; each part gets the cheapest sufficient proof:

1. **Correlator correctness** (no loss/reorder/misroute for any event×expectation
   interleaving): property tests + fuzzing on the pure state machine — random event streams
   with planted replies, asserting the passthrough sequence equals input minus consumed
   replies, order preserved; plus the FM-derived fixture set (stale CPR, wrong-type, split
   reply, two-replies-one-read, interleaved focus).
2. **Driver cancel-safety** (dropping a future mid-await corrupts nothing): Tokio tests
   cancelling at every await point; the correlator's `resolve(Cancelled)` is synchronous so
   there is no async hole between "gave up" and "cleaned up".
3. **No shared-state races**: by construction — correlator is owned data behind `&mut`;
   nothing to loom *here*. The design's one real piece of shared state is the restore
   handle's atomics, which get their own loom test (design 01 §proof obligations). If the
   deferred `parts()` split ever lands, loom becomes mandatory for the lock it introduces;
   recorded as a tripwire in that design's preconditions.
4. **End-to-end**: PTY integration tests (main's ten contracts carried forward as the floor) +
   two real emulators early per the Phase 3 risk ordering.

## Alternatives considered

- **HashMap registry keyed by query type** (prototype shape): rejected — hashing hides the
  overlap question that caused its cross-completion bug; a small ordered Vec makes overlap a
  checked invariant and is faster at N≤10.
- **Serialize all queries, no expectation set** (main today): rejected — cannot express the
  probe bundle, which every serious consumer independently built (helix, zellij, notcurses,
  kitten, codex); N-sequential-timeouts is the documented anti-pattern (rabbitui P1-10).
- **Fully concurrent public query futures** (any query, any task, anytime): rejected —
  requires locks + waker bookkeeping (FM-Q2's shape), and no surveyed consumer needs it;
  coalescing + the probe bundle cover the real concurrency in the wild.
- **Timeout inside the correlator** (own the clock): rejected — kills sans-io testability and
  the blocking driver (OQ-1 constraint: core loop runtime-independent).
