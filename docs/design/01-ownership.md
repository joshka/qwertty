# Design 01 — Ownership model

Who owns the terminal, the read side, the write side; singleton or not; suspend/resume; panic
and drop; restoring state on every exit path. Addresses FM-L*, FM-G*, FM-A2/A6, FM-Q12.

## Decision

**One owned `Session` value per terminal device, no global state, exclusivity by `&mut`.**

- The session opens the **controlling terminal** by default — with `open_path` for PTY tests
  and unusual setups (R-SES-1). *Amended from Phase 4 evidence (FM-A11):* "open the controlling
  terminal" means **dup the inherited stdin descriptor when it is a rdwr tty** (macOS kqueue
  rejects the `/dev/tty` alias everywhere, and even fresh real-path opens inside tmux panes),
  falling back to a ttyname-resolved fresh open of the specific device, then to the `/dev/tty`
  alias; the dup shares its file description with the parent shell, so the ledger also
  restores the description's status flags on every exit path. The device holds one
  fd pair (read/write on the same tty fd) and the termios saved at open.
- **No process-wide singleton.** Nothing prevents two sessions in one process (or two
  processes) from opening the same tty — nothing *can*, cross-process — so we don't pretend:
  the hazard is documented, and a debug-mode advisory check (process-local registry of open
  device inodes, `debug_assert`-grade) catches the common accident without runtime cost in
  release. Global registries are how incumbents got wedged (crossterm's internal shared
  reader is the root of FM-Q1/Q2).
- **Read and write sides live in one owner.** All methods take `&mut self`; the borrow
  checker serializes every interleaving of `next_event()`, queries, writes, and lifecycle
  calls. There are no locks, no shared mutable state, and therefore no lock-order or
  poisoning failure class to prove around. (Concurrency of *queries* is handled inside the
  correlator as a multi-expectation state machine — design 03 — not by concurrent callers.)
- **Split-borrow escape hatch, designed but deferred**: a `parts()` view separating
  `Reader`/`Writer` halves is reserved for when a consumer demonstrates a true full-duplex
  need (render while awaiting events from another task). No surveyed consumer has one today —
  helix, zellij, codex, reedline, and rabbitui all run single-owner loops — and offering it
  early would force the correlator behind a lock.

## State tracking and restore

The session keeps a **mode ledger**: an ordered list of every reversible state change it has
made (raw mode, 1049, 2004, 1004, mouse set, kitty flags, DECSCUSR, changed OSC colors, …),
each entry carrying its undo bytes. This is the single source of truth for all four exit
paths:

1. **`leave(self)`** — replays the ledger in reverse, attempts every step after a failure,
   reports the first error (R-SES-2; FM-L8). Explicitly flushes (FM-L5). The same
   non-blocking bounded-write discipline as the emergency path applies here too (FM-L12): a
   stalled multiplexer makes `leave()` return a timeout-ish error after best-effort partial
   restore + termios reset, never hang — and teardown never routes through `spawn_blocking`
   (main's current `leave()` does; on a shutting-down runtime that is the tokio-Stdin-shaped
   hang this design refuses — salvage flag A2).
2. **`Drop`** — best-effort ledger replay; **skipped when a restore already ran** (an
   `AtomicBool` in the shared restore state — FM-L4's Drop-clobbers-restore bug) and
   panic-aware.
3. **Panic/emergency handle** — `session.restore_handle()` returns a cheap `Arc`'d value
   (raw fd + saved termios + teardown buffer) obtainable without `&mut`, usable from a panic
   hook (R-SES-3; rabbitui P0-7). Mechanics, precisely (reviewer findings M3/M8): the handle
   holds a **preallocated double-buffered teardown blob**; the session re-renders the inactive
   buffer and swaps an atomic pointer/index on every ledger change (under `&mut`, in the
   session thread — composition never happens in the signal/hook path). The hook path does
   only: `write(2)` of the current complete blob (async-signal-safe), `tcsetattr`
   (best-effort), set the "restored" flag. A swap racing a panic delivers either the old or
   the new *complete* blob — both valid teardowns — never a torn one. The teardown `write`
   uses the fd in non-blocking mode with a bounded retry so a stalled multiplexer cannot hang
   teardown (FM-L12: notcurses froze on exit under screen/tmux; we degrade to partial
   restore + the termios reset rather than hang). Enhancement-mode teardown in this blob is
   **stronger than pop** (`CSI < u` full reset — FM-L2, codex practice). qwertty installs **no hook
   itself**; it provides the handle and documents the two-line hook, because hook ordering is
   the app's business (ratatui/color-eyre ecosystem evidence) — and documents plainly that
   non-main-thread panics and `abort()` bypass hooks (FM-L1: gitui's rayon case).
4. **Suspend/resume** (Unix): `suspend()` replays the ledger (keeping a resume snapshot),
   then `kill(0, SIGTSTP)` — the process group, not self (FM-G3). `resume()` re-enters
   ledger state with a bounded retry (the shell races for the tty — FM-G4), forces a termios
   resync rather than trusting cache (codex's disable→enable), optionally flushes stale input
   (`tcflush`), and emits a synthetic Resize event. qwertty does not install signal handlers;
   it exposes these as operations the app calls from its own SIGTSTP/SIGCONT integration,
   plus an optional `signals` helper stream (Tokio feature) for apps that want one-line
   wiring. Degenerate process groups return errors, not crashes (FM-G7).

**Handoff** (`run_detached`-shape, R-SES-6): releases the terminal — ledger replayed, input
source *fully torn down* so nothing holds the fd (FM-A2: pause is not enough; codex
drop-and-recreate evidence) — hands the caller a guard; on guard drop, re-enters modes with
the same resync-don't-trust-cache discipline as resume, because the child may have scrambled
termios (FM-L9). Per-REPL-line cycling of enter/leave is required cheap and re-arming
(R-SES-7; FM-L10): the ledger makes re-entry a replay, with no probing in the cycle — and
**`enter()/leave()` never touch the device fd or its reactor registration** (mode bytes only);
full fd release is exclusively the handoff/suspend path (reviewer finding M2 — this is what
makes the 10k-cycle latency clause hold).

Two adjacent contracts made explicit: opening with no controlling terminal (`/dev/tty`
absent, CI, redirected everything) returns a typed error — never a panic or hang (FM-L11;
termion#64, notcurses#2496). And the session's resize path **coalesces**: when multiple
resize signals/events are pending, `next_event()` delivers one Resize with the final
geometry (R-IN-8's acceptance; FM-G2 — zellij's hand-rolled 50 ms throttle becomes library
behavior), while mouse/scroll events are never coalesced (FM-V6 fidelity; the two policies
are deliberately opposite).

## Alternatives considered

- **Process-wide singleton with runtime enforcement** (notcurses-style context): rejected —
  it cannot actually enforce across processes, adds global state (the thing main got right by
  omission), and breaks multi-PTY tools and tests.
- **Reader/Writer split as the primary API** (termwiz/zellij ClientOsApi shape): rejected for
  v1 — every surveyed consumer is a single-owner loop; the split forces locks into the
  correlator and reopens the FM-Q2 deadlock class we exist to kill. Reserved as `parts()`.
- **Interior mutability (`&self` + internal Mutex)** for ergonomics: rejected — it trades a
  compile-time exclusivity proof for a runtime one, and the runtime one is exactly what
  crossterm has (FM-Q2).
- **Library-installed panic hook**: rejected — double-installation and ordering fights with
  app hooks (helix PR#7931 class); a handle the app installs is strictly more composable.

## Proof obligations (land with implementation)

PTY tests: teardown byte-exactness for arbitrary ledger states; panic-mid-session restores
and backtrace readable; TSTP/CONT ordering; handoff with a termios-scrambling child; 10k
enter/leave cycles without drift. **The restore handle's double-buffer swap is the one piece
of genuinely concurrent shared state in the design and gets a loom test** (hook fires
concurrently with a ledger change and with `leave()`/Drop): assert the hook always observes
a complete blob and double-restore is idempotent (reviewer finding M3 — a unit test is not
enough here). Nothing else shares mutable state, so loom stops there.
