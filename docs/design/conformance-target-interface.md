# Conformance runner: the target interface (one-page sketch)

Phase 2 deliverable, promised to ghostty-rs (protocol-status §2 / collab ask 2). Defines what
a "target" — a thing the conformance runner tests — must provide. Design goal: in-process and
PTY-hosted headless emulators are first-class; installed GUI apps are one adapter among
several, not the model.

## The trait (shape, not final Rust)

```rust
trait Target {
    /// Identity the results file is keyed by (name, version, how obtained).
    fn identity(&self) -> TargetIdentity;

    /// Start one test session: fresh terminal state, given geometry.
    fn start(&mut self, cols: u16, rows: u16) -> Result<()>;

    /// Runner -> terminal bytes (what an application would write).
    fn feed(&mut self, bytes: &[u8]) -> Result<()>;

    /// Terminal -> runner bytes (query replies, reports). Blocks up to `deadline`
    /// (None = drain-what's-there); the runner owns deadline *policy*, the adapter
    /// owns the efficient wait (PTY adapters poll the fd; in-process is synchronous)
    /// so the runner never busy-polls.
    fn drain_output(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>>;

    /// Optional state readback for assertions beyond echoed bytes.
    fn read_state(&mut self, probe: StateProbe) -> Result<Option<StateReading>>;

    fn resize(&mut self, cols: u16, rows: u16) -> Result<()>;
    fn end(&mut self) -> Result<()>;
}

enum StateProbe {           // extensible; None = "can't answer" is always legal
    CursorPosition,
    ModeState(u16),         // DECRQM-shaped, answered from emulator state
    CellText { row: u16, col: u16, len: u16 },
    ScreenHash,             // stable hash of the visible grid
    ScrollbackLine(u32),    // for the FM-V insertion-integrity tests
}
```

## Three adapters, one contract

1. **In-process** (`ghostty-rs`): direct calls into the embeddable VT core. Deterministic,
   CI-runnable on every commit; the only adapter that can honestly answer every `StateProbe`.
   Injectable clock per ghostty-rs's headless design; no display.
2. **PTY-hosted headless** (any emulator that runs against a PTY without a display): foot
   has an explicit headless mode; kitty, WezTerm, and Alacritty may be drivable under
   Xvfb/wayland-headless or their own offscreen modes — **whether each matrix terminal can
   run unattended is itself a Phase 5 finding to record per terminal**, not an assumption
   either way. `read_state` limited to what queries can answer; the runner records
   "unprobeable" rather than guessing.
3. **Attended GUI** (whatever resists headless hosting — Terminal.app and Windows Terminal
   at minimum): same PTY adapter launched inside the real app (a helper the user runs in a
   tab). Never CI. Honesty rule for the matrix: every checked-in result file carries its run
   date and adapter kind, and the rendered matrix marks attended cells **stale** past a
   defined age instead of presenting old attended runs as current — the matrix never implies
   an automation level the adapters don't have.

## Runner responsibilities (so targets stay dumb)

Deadlines and retries; fixture selection honoring `replay` safety classes (nothing `modal`/
`destructive` without explicit opt-in — the DECSLPP incident rule); reply parsing against the
database's `responds` linkage; identity verification (probe DA1/XTVERSION and cross-check
`identity()`); writing `conformance/results/<terminal>/<version>.toml`. **Capture mode** is
the same loop with recording on: raw reply bytes become `origin=capture:` fixtures
(quarantine replacement) and results in one pass.

## What ghostty-rs can rely on

- This trait's `feed`/`drain_output`/`start`/`end` core is stable in intent from this sketch
  onward; `StateProbe` grows additively.
- Differential parity vs libghostty-vt (your Phase 1 exit) remains the agreed trust bar for
  accepting ghostty-rs captures into the database.
- The capture format handshake (collab ask 4): capture logs are raw bytes + a TOML sidecar
  (identity, probe id, timestamps); exact sidecar schema lands with the harness
  implementation and will be dropped in this folder before it freezes.
