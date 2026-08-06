# Design 04 — Async strategy

Runtime coupling, the sync story, and why. Addresses FM-A1–A10; implements R-ASY-1–5 and the
OQ-1 resolution.

## Decision

**Sans-io core, Tokio-first session, blocking driver as a thin front-end. No runtime traits.**

Three layers, strictly ordered by what they're allowed to know:

1. **Pure layers** (no I/O, no clock, no runtime, `default = []`, build everywhere):
   commands/encoding, decoder (design 02), correlator (design 03), capability model,
   mode ledger. These are the product's brain; everything else is plumbing that feeds them
   bytes and time. This is what "runtime-neutral core" *means* here — neutrality by
   construction, not by trait abstraction over hypothetical executors.
2. **Tokio session** (`tokio` feature, Unix): `AsyncFd`-readiness on the caller's runtime.
   No helper threads, no spawned tasks, no channels (FM-A1/A3; affirms main's model on its
   own evidence). Every `async fn` is cancel-safe and documented as such: state lives in the
   session, not in futures, so a dropped future abandons only its own wait (FM-A5). Runtime
   shutdown can never hang on a read because nothing blocks (tokio's own Stdin caveat is the
   counterexample we refuse).
3. **Blocking one-shot driver** (OQ-1): a `poll(2)`-based loop shipped first as a
   **documented recipe + example**, promoted to a public `oneshot` API at/after the capture
   harness milestone. Precision about what the recipe uses (reviewer finding C1): the
   correlator is `pub(crate)`, so the recipe is written against the layers that *are*
   public — sync `Terminal` + raw-mode guard, `commands::*` probe blob, `poll(2)` timeout,
   public `InputDecoder`, public `report::*` parsers — which is sufficient for a one-shot
   probe (typeahead survives as leftover decoded events; the correlator's job, interleaving
   pending expectations with a live event loop, doesn't arise in a one-shot). The promoted
   `oneshot` API is what later wraps the internal correlator for correlator-grade guarantees.
   The design constraint stands: any core API that would force an executor (async trait in
   the correlator, `Instant::now()` inside the decoder) is a defect.

Event-loop contract (R-ASY-2): the caller owns the loop; `next_event()` is a leaf future for
`select!`. **Handoff pause is full release** — the input source drops its fd registration and
the session can surrender the fd (FM-A2/A6; codex's drop-and-recreate is the pattern, here
first-class). This is distinct from per-cycle `enter()/leave()` (design 01): the REPL cycle
keeps the device and its reactor registration and moves only mode bytes — R-SES-7's 10k-cycle
latency clause is met because fd re-registration never happens on that path.
Buffered-but-undelivered events survive pause/resume in order.

## Why not runtime-generic traits (async-std/smol adapters)?

- Zero surveyed consumers asked for a non-Tokio async session; helix embeds termina in a
  Tokio select, zellij runs threads + a Tokio path, rabbitui targets Tokio explicitly.
- A trait over one implementation freezes speculation (the same reasoning that kills
  premature crate splits); vte's five breaking releases show what abstraction churn costs
  downstream (FM-E4).
- The sans-io core already delivers the portable part: a smol adapter, if ever demanded, is a
  driver PR, not a redesign — that's the acceptance test of this design.
- Cost accepted: the session type name is Tokio-coupled pre-1.0; rabbitui's seam-in-one-file
  strategy (substrate-status §2) is the documented consumption pattern meanwhile.

## The sync session question

The existing sync `TerminalSession` (write + raw bytes, no decoded events) stays as the thin
device-plus-ledger layer the Tokio session builds on — it is the sync core ADR-0001 pointed
at, now with a job (OQ-1's driver, tests, and simple write-only tools). It does **not** grow
decoded-event or query APIs of its own beyond what the one-shot recipe needs; a full blocking
event-loop session (gitui-shaped consumers) is P2, buildable on the same core if demanded.

## Cancellation and time

- All deadlines are driver-owned (`tokio::time` vs `poll(2)` timeout math) against
  deadline-free core operations (design 03).
- Cancel-safety test matrix is per-await-point, not per-method (design 03 §proof plan).
- No `Instant`/`SystemTime` in pure layers — enforced by a clippy `disallowed-methods` entry
  so the constraint outlives this document.

## Alternatives considered

- **Threads + channels facade** (crossterm EventStream shape): rejected — it *is* the FM-A
  catalog (thread-per-event, unkillable reads, stolen bytes).
- **Async trait in core (`AsyncRead`-parameterized session)**: rejected — infects the pure
  layers with pinning/executor concerns, kills the blocking driver, and tests regress to
  needing a runtime.
- **spawn_blocking bridge for a "portable" session**: rejected — it's the tokio::Stdin
  anti-pattern with our name on it (FM-A1).
- **Actor/task-owned terminal with message passing** (what several ratatui apps hand-roll):
  rejected as a *library* choice — R-ASY-2's caller-owns-the-loop is a rabbitui contract ask,
  and an actor is trivially built on top of awaitable primitives, never the reverse.
