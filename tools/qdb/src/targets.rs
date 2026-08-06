//! The conformance `Target` trait — what a thing the runner tests must provide.
//!
//! This is the approved Phase 2 sketch (`work/phase2/design/conformance-target-interface.md`)
//! made concrete: in-process and PTY-hosted headless emulators are first-class; installed GUI
//! apps are one adapter among several, not the model. The runner (`crate::runner`) owns policy
//! — deadlines, retries, replay-class gating, identity verification — so targets stay dumb:
//! they move bytes and answer state probes, nothing more.
//!
//! Three adapter kinds share this one contract: in-process (future ghostty-rs), PTY-hosted
//! headless (tmux, betamax, and every Phase 5 breadth target), and attended GUI (the same PTY
//! mechanics launched inside a real app). Whether a given terminal can run unattended is a
//! per-target *finding recorded in results*, never an assumption.

use std::time::Duration;

#[cfg(unix)]
pub mod alacritty;
#[cfg(unix)]
pub mod betamax;
#[cfg(unix)]
pub mod foot;
#[cfg(unix)]
pub mod kitty;
#[cfg(unix)]
pub mod relay;
#[cfg(unix)]
pub mod tmux;
#[cfg(unix)]
pub(crate) mod util;
#[cfg(unix)]
pub mod wezterm;
#[cfg(unix)]
pub mod xterm;

// The ConPTY adapter (issue #196 item 2): a `#[cfg(windows)]` pseudo-console host with a relay
// child, the Windows sibling of the Unix PTY-hosted targets. It is a draft skeleton — it compiles
// (including cross-compiled for `x86_64-pc-windows-msvc`) but has never run on a real host, and is
// not wired into CI. The side-channel framing is platform-neutral and its unit tests run on the
// host under `--all-targets` (`cfg(test)`); everything else is `#[cfg(windows)]`.
#[cfg(windows)]
pub mod conpty;
#[cfg(any(test, windows))]
pub mod conpty_frame;
#[cfg(windows)]
pub mod conpty_sys;
#[cfg(windows)]
pub mod relay_conpty;

/// How a target is hosted — recorded with results so the matrix never implies an automation
/// level the adapter doesn't have (the attended-cells honesty rule).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterKind {
    /// Direct calls into an embeddable VT core (future ghostty-rs adapter).
    InProcess,
    /// A terminal driven over a PTY without a display (tmux pane, betamax tape, headless foot).
    PtyHosted,
    /// The same PTY mechanics launched inside a real GUI app by a person; never CI.
    AttendedGui,
}

impl AdapterKind {
    /// The `adapter` field spelling for a results file (schema v2).
    #[must_use]
    pub const fn as_results_str(self) -> &'static str {
        match self {
            Self::InProcess => "in-process",
            Self::PtyHosted => "pty-headless",
            Self::AttendedGui => "attended",
        }
    }
}

/// Identity the results file is keyed by: who this target is and how we drive it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetIdentity {
    /// The target slug used in artifact paths (`db/results/<name>.toml`), e.g. `tmux`.
    pub name: String,
    /// Out-of-band version hint (e.g. `tmux -V` output). The wire XTVERSION reply is
    /// authoritative when present — the emulator naming itself beats the harness's guess.
    pub version_hint: String,
    /// How the target is hosted.
    pub adapter: AdapterKind,
    /// The name the terminal is expected to report on the wire (XTVERSION), for the runner's
    /// identity cross-check. Distinct from `name` when a harness hosts another emulator:
    /// betamax hosts ghostty, so its wire name is `ghostty` while its slug stays `betamax`.
    /// `None` means the adapter claims nothing and the cross-check records "unverifiable".
    pub expected_wire_name: Option<String>,
}

/// Optional state readback for assertions beyond echoed bytes. Extensible; a target answering
/// `None` ("can't answer") is always legal — the runner records "unprobeable" rather than
/// guessing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateProbe {
    /// Where the cursor is, 1-based.
    CursorPosition,
    /// A DECRQM-shaped mode query answered from emulator state, not the wire.
    ModeState(u16),
    /// The text content of a run of cells.
    CellText {
        /// 1-based row.
        row: u16,
        /// 1-based first column.
        col: u16,
        /// Number of cells to read.
        len: u16,
    },
    /// A stable hash of the visible grid.
    ScreenHash,
    /// One scrollback line, for insertion-integrity tests.
    ScrollbackLine(u32),
}

/// A state probe's answer, mirroring [`StateProbe`] variants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StateReading {
    /// Cursor position, 1-based.
    CursorPosition {
        /// 1-based row.
        row: u16,
        /// 1-based column.
        col: u16,
    },
    /// A DECRQM-style mode value (`0` not recognized, `1` set, `2` reset, …).
    ModeState(u16),
    /// Text content read from the grid or scrollback.
    Text(String),
    /// A stable hash of the visible grid.
    Hash(u64),
}

/// A thing the conformance runner tests. See the module docs for the adapter taxonomy.
///
/// The `feed`/`drain_output`/`start`/`end` core is stable in intent from the Phase 2 sketch
/// onward; `StateProbe` grows additively.
pub trait Target {
    /// Identity the results file is keyed by (name, version hint, how obtained).
    fn identity(&self) -> TargetIdentity;

    /// Starts one test session: fresh terminal state, given geometry.
    ///
    /// # Errors
    ///
    /// Returns an error if the target cannot be launched or the session cannot be established.
    fn start(&mut self, cols: u16, rows: u16) -> Result<(), String>;

    /// Runner -> terminal bytes (what an application would write).
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes cannot be delivered (e.g. the target died).
    fn feed(&mut self, bytes: &[u8]) -> Result<(), String>;

    /// Terminal -> runner bytes (query replies, reports). Blocks up to `deadline` for the first
    /// byte, then returns everything available; `None` means drain-what's-there without
    /// waiting. The runner owns deadline *policy* (how long, how often, when to settle); the
    /// adapter owns the efficient wait so the runner never busy-polls.
    ///
    /// # Errors
    ///
    /// Returns an error if the target died mid-drain. A quiet target is `Ok(vec![])`, not an
    /// error — silence is data.
    fn drain_output(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>, String>;

    /// Optional state readback for assertions beyond echoed bytes. `Ok(None)` — "can't
    /// answer" — is always legal.
    ///
    /// # Errors
    ///
    /// Returns an error only on transport failure, never for an unanswerable probe.
    fn read_state(&mut self, probe: StateProbe) -> Result<Option<StateReading>, String>;

    /// Resizes the live session.
    ///
    /// # Errors
    ///
    /// Returns an error if the adapter cannot resize (recorded, like any capability gap).
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String>;

    /// Ends the session and releases the target.
    ///
    /// # Errors
    ///
    /// Returns an error if teardown fails; the runner reports it but keeps captured data.
    fn end(&mut self) -> Result<(), String>;
}
