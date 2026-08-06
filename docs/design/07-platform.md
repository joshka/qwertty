# Design 07 — Platform strategy

What Unix-first costs, what Windows requires, and when. Implements R-PLAT-1/2 (OQ-2/OQ-3
resolved); addresses FM-W*, FM-Z*.

## Decision

**Unix tier 1 (Linux + macOS, PTY-tested CI). Windows: seams designed now, implementation
post-first-publish, VT-over-ConPTY only. BSDs: later pass, explicit P2.**

### What Unix-first costs (stated, not hand-waved)

- Windows users keep crossterm for now; ratatui-shaped adoption of qwertty stays partial
  until the Windows driver lands. Accepted: shipping a *correct* Unix core beats shipping the
  incumbent failure matrix on two platforms (FM-W7: Windows paths bitrot exactly when they're
  an afterthought — so we make them an explicit later project, not a neglected present one).
- Every public type must therefore be scrutinized *now* for Unix leakage — that's the real
  price, paid in design review rather than code.

### The seams that make Windows an implementation, not a redesign

1. **Device trait** (design 08): `read/write/geometry/mode-control` object. Unix impl wraps
   the tty fd + termios; Windows impl wraps `CONIN$`/`CONOUT$` handles +
   `SetConsoleMode`-based raw toggling + `GetConsoleScreenBufferInfo` size fallback (FM-Z2,
   FM-W4 — restore must reset console modes crossterm leaves behind). The mode **ledger**
   (design 01) is already platform-neutral: entries carry undo *operations*, not termios
   assumptions.
2. **Event vocabulary** is decoder-independent (R-IN-10) — helix ships two backends into one
   event type today; our events never expose termios/VT details, so a future
   INPUT_RECORD-assisted path can feed them.
3. **Pure layers build everywhere already** (enforced in CI from day one: a
   `cargo check --target x86_64-pc-windows-msvc` + wasm32 job on the default feature set).
   Live-session APIs return `Unsupported` off-Unix — a typed, documented boundary.
4. **Capability/identity model** covers Windows Terminal/ConPTY as identities with
   conformance rows, so the "what does ConPTY actually pass through" unknown (FM-W1) becomes
   runner output, not folklore.

### Windows requirements dossier (for the future milestone)

Floor: Windows 10 1809+ (ConPTY), VT-only public surface, no legacy Console API types
exposed (termina precedent; rabbitui P2-17). Known hazards to budget for, from evidence:
VT-only *input* is not sufficient — CJK/IME breaks (FM-W3, helix re-added crossterm on
Windows for this); win32-input-mode is all-or-nothing today (FM-W2) — decode support yes,
enabling it policy-gated; console-mode restore asymmetries (FM-W4); `CONIN$` blocking-read
cancellation needs a real design (no AsyncFd equivalent — likely overlapped I/O or a
watchdog thread with an explicit contract, decided in that milestone, sans-io core unchanged).
The paste story needs the burst-heuristic hooks (FM-P12) because 2004 is absent in places.

### BSDs (OQ-3)

`rustix` covers them; the gap is CI. Plan: opportunistic `cargo check` cross-builds are cheap
and can land anytime; *claiming support* requires PTY-tested VMs (FreeBSD has no Docker
path), which is a self-contained P2 work item. Until then: "expected to work, not validated"
— worded exactly that way in platform docs (support = validated behavior).

### WASM / exotic targets

Pure layers only (encode/decode/db consumption) — this falls out of the sans-io architecture
for free and gets the same CI check-build. No terminal ownership story is claimed (FM: the
crossterm WASM/UEFI issue tail is a warning, not a market).

## Alternatives considered

- **Windows in the first release**: rejected — doubles the highest-risk surface (input
  decoding + lifecycle) before the query core is proven on PTYs (Phase 3 risk ordering), and
  the evidence says half-done Windows is worse than absent (FM-W3/W7).
- **Legacy Console API dual-stack** (crossterm's shape): rejected permanently for the public
  surface — it's the divergence machine behind a third of crossterm's tracker (audit theme
  5); if INPUT_RECORD assistance is ever needed for IME, it hides behind the device trait.
- **Unix-only forever** (termion's path): rejected — termion's Windows vacuum is why
  crossterm exists and won (FM-W: termion GL#103); the mission is to *replace* the incumbents,
  which requires their platform reach eventually.
