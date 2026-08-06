# Design 08 — Crate/module boundaries and public API shape

Boundaries, naming, and the API shape for the Phase 1 workflows — including rabbitui's
substitutable-device seam. Implements R-TST-1, R-REL-2, R-IN-10, R-OUT-1.

## Crates

**One published crate: `qwertty`.** Plus, unpublished in-workspace: `tools/qdb` (database
validate/generate/capture — different audience, no semver coupling) and, when the runner
lands, `tools/qconform`. The prototype's 17-crate sprawl and the ecosystem's version-lockstep
pain (FM-E1) both argue against premature splits; the split rule stays evidence-based
(affirmed ADR 0014). If the database consumption path ever needs a tiny `qwertty-db-types`
crate for third parties, that's the one foreseeable split — deferred until ghostty-rs asks
for compiled types instead of TOML.

## Module map (public surface)

```text
qwertty
├── command/        // Command, CommandBuffer — pure encode, builds everywhere
│   └── commands::{cursor, screen, style, mode, osc, kitty, ...}  // grows per family
├── event/          // InputEvent + decoded types — the shared vocabulary (R-IN-10)
├── decode/         // InputDecoder (syntax layer) + semantic decoders   [design 02]
├── report/         // typed reply parsers (CPR, DECRPM, DA, OSC colors, ...)
├── caps/           // Capabilities, TerminalIdentity, Evidence          [design 06]
├── policy/         // Policy + presets                                  [design 06]
├── device/         // TerminalDevice trait + UnixTty + FakeDevice       [this doc]
├── session/        // sync core: device + mode ledger + restore handle  [design 01]
└── tokio/          // TokioSession (feature "tokio")                    [design 04]
    (correlator is pub(crate) under session/, driven by tokio/ and the oneshot recipe)
```

Feature layout: `default = []` (pure layers + sync core), `tokio` adds the async session.
Nothing else gates behavior — feature combinatorics is a bug budget. The no-clock rule
(design 04) is enforced as a workspace clippy `disallowed-methods` lint scoped to the pure
modules (`command`, `event`, `decode`, `report`, `caps`) so the constraint outlives the docs.

## The device seam (rabbitui's highest-value ask)

```rust
pub trait TerminalDevice {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;   // non-blocking semantics
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn flush(&mut self) -> io::Result<()>;
    fn geometry(&self) -> io::Result<TerminalSize>;            // cells + pixels-if-known
    fn set_mode(&mut self, mode: DeviceMode) -> io::Result<()>; // raw/cooked (ledger's floor)
    fn readiness(&self) -> Readiness;                          // how a driver waits (fd | poll-only)
}
```

Implementations: `UnixTty` (real), **`FakeDevice`** (the R-TST-1 seam), and later
`WindowsConsole`. `FakeDevice` mechanics, stated precisely because `AsyncFd` requires a real
pollable fd and an in-memory buffer has none (reviewer finding C2): on Unix it is
**socketpair-backed** — the device side is one end (a genuine fd the Tokio session registers
with `AsyncFd`, unmodified), the test script owns the other end to feed input bytes and read
emitted bytes; `set_mode` records mode changes as inspectable data instead of ioctls. No PTY,
no terminal, std-only — rabbitui drives the *real* `TokioSession` in a unit test, which is
R-TST-1's acceptance criterion satisfied by mechanism rather than assertion. (On non-Unix, or
for pure logic tests, the sans-io core is tested directly with no device at all.)
`geometry()` carries an explicit contract (FM-Z2–Z4, the ~30-issue size cluster): degenerate
values (0×0, `u16::MAX`) are rejected as errors, never returned; the session's `size()`
implements a documented fallback ladder — tty ioctl → `CSI 18 t` query (full probe hygiene;
xterm gates it behind `allowWindowOps`) → `COLUMNS`/`LINES` env (labeled `Inferred`) → typed
"unknown" so the caller supplies the default — and "how do I even get the size" stops being
downstream folklore. PTY harnesses remain the integration tier (unchanged from main).

## Event and command vocabulary rules (the ecosystem-lockstep firewall)

- `event::` and `command::` types: no third-party types in the public API, `#[non_exhaustive]`
  everywhere extension is plausible, no `Copy` promises on non-trivial enums, and **no
  re-exported dependency types** (reedline#456's lesson). These are the types widget
  ecosystems will link; they change freely pre-0.1 (rabbitui contract) and calcify at publish.
- Commands are data (R-OUT-1): every typed command encodes to bytes without a device;
  `CommandBuffer` composes blobs the caller may write atomically (zellij/codex pattern);
  raw-bytes escape hatch stays.
- Text event payload shape: decided by the OQ-6 spike (design 02 records it).

## Workflow walkthroughs (API sanity checks against the Phase 1 profiles)

- **rabbitui frame loop**: `TokioSession::open()` → probe → `select! { ev = s.next_event() …}`
  → render into `CommandBuffer` (encode-only, no session borrow) → `s.write(buf).flush()`
  wrapped in 2026 commands. Panic hook from `s.restore_handle()`.
- **helix-shaped editor**: `s.suspend()/resume()` around SIGTSTP; `s.run_detached(|| spawn
  $EDITOR)` handoff; `probe_capabilities()` at startup; kitty push with granted-set ledger.
- **reedline REPL**: `session.enter()/leave()` per line over one long-lived device; CPR via
  `s.cursor_position()` — unsolicited stale CPRs surface as events, matcher unaffected.
- **codex-shaped inline TUI**: scroll-region/RI commands from `commands::screen`; 2026
  helpers; probe with 100ms budget; `pause` = full release for child processes.
- **one-shot CLI (recipe)**: `Terminal::open()` + `enter_raw()` guard + probe bundle over the
  blocking driver → typed findings → drop restores. No async dep compiled.

## Naming

`Session`/`TokioSession` (drop the `Terminal` prefix stutter — `qwertty::Session` reads
clean); `TerminalDevice` keeps the noun where ambiguity is real; command modules named by
protocol domain (`style`, `mode`, `osc`) not by terminal vendor except where the protocol is
vendor-defined (`kitty`, `iterm2`). ids in the database, not type names, carry canonical
sequence identity.

## Alternatives considered

- **Facade + N subcrates** (prototype): rejected — 17 crates froze boundaries that were
  guesses; module tree gives the same navigation without the semver blast radius.
- **Device trait as `AsyncRead + AsyncWrite`**: rejected — infects pure layers with runtime
  types and makes the in-memory fake awkward; a byte-level sync trait with a readiness hint
  is the smallest thing every driver can build on.
- **Public correlator/router module**: rejected here as in design 03 — narrow typed methods
  are the consumer-shaped surface; the router is machinery.
- **Prelude module**: deferred — preludes calcify accidental surface; revisit at publish.
