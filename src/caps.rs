//! Terminal capabilities: the typed result of the DA1-fenced probe bundle (design 03/06).
//!
//! [`Capabilities`] is what `TokioTerminalSession::probe_capabilities` (available with the
//! optional `tokio` feature on Unix) returns: a struct of typed [`Finding`]s, one per queryable
//! capability the probe bundle asks about in a single write-and-fence round trip, plus
//! [`TerminalIdentity`] and the env-inferred findings that have no query at all (FM-C12). Every
//! finding's `value` is `Option<T>`, and
//! **`None` means unknown, never unsupported** (FM-C4): a terminal that answers nothing — a silent
//! terminal, or a multiplexer that swallowed the queries — yields an all-unknown `Capabilities`;
//! that is different from a terminal that answered a DECRQM query with "mode reset" (`Some(false)`)
//! or "mode not recognized" (`None` for that one field). Consumers and qwertty's own emit-gating
//! read this distinction *and* the finding's [`Evidence`], so "we probed and it said no," "we
//! inferred it from the environment," and "nothing answered" all degrade differently (design 06).
//!
//! # Evidence provenance
//!
//! Every [`Finding`] carries its [`Evidence`]: [`Evidence::Probed`] names the query/sequence that
//! answered, [`Evidence::Inferred`] names the environment heuristic that guessed, and
//! [`Evidence::Unknown`] means nothing answered and nothing inferred. A consumer that only reads
//! `.value()` gets the same tri-state as M3-S1; a consumer that needs to tell "the terminal told us
//! no" apart from "we never asked" reads `.evidence()`.
//!
//! # Identity is a finding too (R-CAP-5)
//!
//! [`TerminalIdentity`] is derived from the XTVERSION reply, cross-checked against environment
//! variables, because under a multiplexer the probe replies describe the mux, not the outer
//! terminal (FM-C3) — [`TerminalIdentity::mux_stack`] records that context explicitly rather than
//! silently reporting the mux's own identity as if it were the user's terminal.
//!
//! # Env heuristics are a documented, inspectable table (FM-C12)
//!
//! Some features have no query at all — OSC 8 hyperlink support and truecolor support are the
//! canonical examples. [`Capabilities::hyperlinks`] and [`Capabilities::truecolor`] are populated
//! from [`HYPERLINK_ENV_HEURISTICS`] and the `COLORTERM` check, always labeled
//! [`Evidence::Inferred`] so a consumer can never mistake a guess for a probed answer.
//!
//! # Snapshot, not live (FM-C8/FM-C13)
//!
//! `Capabilities` is a snapshot taken at probe time for one attachment. Resume/reattach can move a
//! session to a different outer terminal (zellij multi-client reattach) and mode-2031-style events
//! can change the answer mid-session; treating a stale `Capabilities` as still current, and
//! deciding when to mark findings stale and re-probe, is M6's concern (design 06 caching policy).
//! This slice only produces the snapshot.
//!
//! # Detection posture: dumb terminals
//!
//! A `TERM=dumb` terminal is not sent the probe bundle by well-behaved callers — probing has side
//! effects even when unanswered (FM-C7) — so a caller that detects `TERM=dumb` should skip
//! `probe_capabilities` entirely and treat every finding as [`Evidence::Unknown`]. This module
//! does not enforce that guard itself (it has no opinion on when a caller chooses to probe);
//! [`identity_from_env`] still runs safely over a `TERM=dumb` environment because it only reads
//! env vars and never writes to the terminal.

use std::fmt;

use crate::correlate::DeviceAttributes as CorrelateDeviceAttributes;

/// A 24-bit RGB colour, 8 bits per channel.
///
/// This is the normalized form of an OSC colour report (design 03): terminals report colours in the
/// X11 `rgb:R/G/B` form with 1–4 hex digits per channel, and
/// [`OscColorReport`](crate::report::OscColorReport) scales every width down to this 8-bit-per-
/// channel value so a consumer sees one shape regardless of the terminal's reporting width (FM-P9).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Rgb {
    red: u8,
    green: u8,
    blue: u8,
}

impl Rgb {
    /// Creates an RGB colour from its three 8-bit channels.
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// Returns the red channel.
    #[must_use]
    pub const fn red(self) -> u8 {
        self.red
    }

    /// Returns the green channel.
    #[must_use]
    pub const fn green(self) -> u8 {
        self.green
    }

    /// Returns the blue channel.
    #[must_use]
    pub const fn blue(self) -> u8 {
        self.blue
    }
}

/// The Primary Device Attributes (DA1) a terminal reported as the probe fence.
///
/// DA1 (`CSI ? … c`) is the probe's fence, not a feature oracle (design 03, FM-C4): its arrival
/// means "every reply that was coming has arrived," and its *presence* alone proves nothing about
/// features (a real VT100 answers). This value preserves the raw attribute parameter bytes
/// (everything between `CSI ?` and the final `c`) so [`TerminalIdentity`] can use it as a weak
/// signal — different terminals report different, sometimes widening, attribute lists.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DeviceAttributes {
    params: Vec<u8>,
}

impl DeviceAttributes {
    /// Creates device attributes from the raw DA1 parameter bytes (excluding `?` and the final
    /// `c`).
    #[must_use]
    pub fn new(params: impl Into<Vec<u8>>) -> Self {
        Self {
            params: params.into(),
        }
    }

    /// Returns the raw DA1 parameter bytes, excluding the `?` private marker and the final `c`.
    ///
    /// For `CSI ? 1 ; 2 c` this is `b"1;2"`. An empty slice is possible for a bare `CSI ? c`.
    #[must_use]
    pub fn params(&self) -> &[u8] {
        &self.params
    }
}

impl From<CorrelateDeviceAttributes> for DeviceAttributes {
    fn from(attrs: CorrelateDeviceAttributes) -> Self {
        Self::new(attrs.params().to_vec())
    }
}

/// How a [`Finding`]'s value was obtained.
///
/// This is the provenance design 06 requires on every capability finding: a consumer (and
/// qwertty's own emit-gating) can tell "we probed and the terminal answered" apart from "we guessed
/// from the environment" apart from "nothing told us anything" — three states that must degrade
/// differently (FM-C4, FM-C12).
///
/// `#[non_exhaustive]` so a future evidence source (for example a conformance-matrix lookup keyed
/// by identity, design 06's `Conformance { result: … }` findings) can be added without breaking
/// existing matches.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum Evidence {
    /// The value came from a terminal reply to an explicit query.
    ///
    /// `via` names the query/sequence that answered, as a stable short label — for example
    /// `"DECRQM 2026"`, `"XTVERSION"`, `"OSC 11"`, `"DA1"`. Not a formal `SequenceId` type (none
    /// exists yet in this crate); a `&'static str` keeps this evidence layer decoupled from any
    /// future query-vocabulary type while still being a stable, greppable label.
    Probed {
        /// The query/sequence that answered.
        via: &'static str,
    },
    /// The value was guessed from environment variables, never from a terminal reply.
    ///
    /// `via` names the heuristic, for example an env var name (`"TERM_PROGRAM"`) or a short
    /// heuristic label (`"hyperlink-env-sniff"`). Always used where no query exists (FM-C12) or as
    /// a cross-check alongside a probed value (identity).
    Inferred {
        /// The environment variable or heuristic that produced the value.
        via: &'static str,
    },
    /// Nothing probed and nothing inferred: the value is unknown, not unsupported (FM-C4).
    #[default]
    Unknown,
}

/// A tri-state capability value paired with the [`Evidence`] that produced it.
///
/// `value: None` means *unknown*, never *unsupported*, regardless of `evidence` — a terminal can
/// answer a DECRQM query with "mode not recognized" (`Probed` evidence, `None` value) just as
/// easily as never answer at all (`Unknown` evidence, `None` value); both are "do not assume this
/// feature," and a consumer that cares about the difference reads `evidence`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Finding<T> {
    value: Option<T>,
    evidence: Evidence,
}

impl<T> Finding<T> {
    /// Creates a finding whose value came from a terminal reply to an explicit query.
    #[must_use]
    pub const fn probed(value: Option<T>, via: &'static str) -> Self {
        Self {
            value,
            evidence: Evidence::Probed { via },
        }
    }

    /// Creates a finding whose value was guessed from the environment.
    #[must_use]
    pub const fn inferred(value: Option<T>, via: &'static str) -> Self {
        Self {
            value,
            evidence: Evidence::Inferred { via },
        }
    }

    /// Creates a finding with no value and no evidence: nothing probed, nothing inferred.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            value: None,
            evidence: Evidence::Unknown,
        }
    }

    /// Returns the finding's value, or `None` for unknown (never unsupported — FM-C4).
    #[must_use]
    pub const fn value(&self) -> Option<&T> {
        self.value.as_ref()
    }

    /// Returns the finding's evidence: how the value (or its absence) was obtained.
    #[must_use]
    pub const fn evidence(&self) -> &Evidence {
        &self.evidence
    }

    /// Returns `true` when the finding has a value, regardless of evidence.
    #[must_use]
    pub const fn is_known(&self) -> bool {
        self.value.is_some()
    }
}

impl<T: Copy> Finding<T> {
    /// Returns the finding's value by copy, for `Copy` value types.
    #[must_use]
    pub const fn value_copied(&self) -> Option<T> {
        self.value
    }
}

/// A terminal program identity, as best-effort derived from XTVERSION and/or environment variables.
///
/// `#[non_exhaustive]` so a newly recognized terminal adds a variant without breaking existing
/// matches; `Unknown(String)` preserves an unrecognized XTVERSION string verbatim rather than
/// discarding it.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TerminalProgram {
    /// kitty.
    Kitty,
    /// Ghostty.
    Ghostty,
    /// iTerm2.
    Iterm2,
    /// `WezTerm`.
    WezTerm,
    /// Alacritty.
    Alacritty,
    /// foot.
    Foot,
    /// Rio.
    Rio,
    /// Visual Studio Code's integrated terminal.
    VsCode,
    /// Windows Terminal.
    WindowsTerminal,
    /// macOS `Terminal.app`.
    AppleTerminal,
    /// tmux, when it is answering the probe itself rather than passing it through (FM-C3).
    ///
    /// This variant is distinct from [`Multiplexer::Tmux`] in [`TerminalIdentity::mux_stack`]:
    /// `mux_stack` records that a mux is *present in the stack*, while this variant is what
    /// `program` becomes when the identity signals (XTVERSION reply, DA1 shape) themselves point at
    /// tmux rather than at the terminal underneath it.
    Tmux,
    /// GNU screen, for the same self-answering reason as `Tmux`.
    Screen,
    /// A recognized-as-unrecognized terminal: the raw XTVERSION or `TERM`/`TERM_PROGRAM` text that
    /// did not match a known program, preserved verbatim rather than discarded.
    Unknown(String),
}

/// A terminal multiplexer present in the attachment's stack.
///
/// Presence in [`TerminalIdentity::mux_stack`] is first-class (design 06): under tmux, probe
/// replies describe tmux, not the outer terminal (FM-C3), and passthrough gating differs per layer
/// (FM-M1/M2), so callers need to know a mux is present even when `program` also resolves to a
/// specific terminal identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Multiplexer {
    /// tmux (`TMUX` env var present).
    Tmux,
    /// GNU screen (`STY` env var present).
    Screen,
    /// Zellij (`ZELLIJ` env var present).
    Zellij,
}

/// The terminal's derived identity: program, version, and multiplexer stack (design 06, R-CAP-5).
///
/// Identity is itself a finding, assembled from whichever signals were available rather than
/// carrying one `Evidence` for the whole struct: `program`/`version` come from XTVERSION when it
/// answered (`Evidence::Probed { via: "XTVERSION" }`, conceptually — the fields here are the parsed
/// result; see [`identity_from_env`] for how they are derived), cross-checked and, when XTVERSION
/// is silent, filled in from environment variables (`Evidence::Inferred`). DA1's shape is at most a
/// weak signal (FM-C4: it proves nothing about features, and even less about identity — many
/// terminals share widened DA1 attribute lists), used only when nothing else resolved a program.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TerminalIdentity {
    /// The recognized terminal program, or `None` when nothing (XTVERSION, DA1, env) resolved one.
    pub program: Option<TerminalProgram>,
    /// The terminal's self-reported version string, verbatim, when one was available.
    pub version: Option<String>,
    /// Every multiplexer detected in the attachment's stack, outermost session state first.
    ///
    /// Empty when no multiplexer env var was observed. A stack can have more than one entry when
    /// muxes are nested (for example zellij running inside tmux); detection is independent per
    /// mux env var, so order here follows the fixed check order in [`identity_from_env`], not a
    /// measured nesting order.
    pub mux_stack: Vec<Multiplexer>,
}

/// Looks up an environment variable's value.
///
/// [`identity_from_env`] and the env-heuristic findings take a `env: impl EnvSource` rather than
/// calling [`std::env::var`] directly, so identity/env inference logic is purely testable — no test
/// needs to mutate the process environment, which is unsound to do from parallel tests. Production
/// callers pass [`std::env::var`] itself, since `Fn(&str) -> Option<String>` is implemented for it
/// once wrapped in a closure (see [`identity_from_env`]/[`std_env_source`]).
pub trait EnvSource: Fn(&str) -> Option<String> {}
impl<F: Fn(&str) -> Option<String>> EnvSource for F {}

/// An [`EnvSource`] backed by the real process environment ([`std::env::var`]).
///
/// Production callers use this; tests pass a closure over a fixed map instead so identity/env
/// inference logic is exercised without mutating process-global state.
#[must_use]
pub fn std_env_source(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Best-effort program-name matching against an XTVERSION reply string.
///
/// XTVERSION reply text is not standardized beyond "some identifying text" (design 03/06), so this
/// is deliberately substring matching against known prefixes/markers rather than a strict grammar.
/// Matching is case-sensitive against each terminal's own documented self-report string, checked in
/// a fixed order; the first match wins. Returns `None` when nothing recognized matches, so the
/// caller can fall back to env or leave `program` unresolved.
///
/// Known markers (see each terminal's own XTVERSION output):
///
/// - `"kitty"` → [`TerminalProgram::Kitty`] (kitty reports like `kitty(0.35.1)`)
/// - `"ghostty"` → [`TerminalProgram::Ghostty`] (ghostty reports like `ghostty 1.0.0`)
/// - `"WezTerm"` → [`TerminalProgram::WezTerm`]
/// - `"iTerm2"` → [`TerminalProgram::Iterm2`]
/// - `"Alacritty"` → [`TerminalProgram::Alacritty`]
/// - `"foot"` → [`TerminalProgram::Foot`]
/// - `"rio"` (case-insensitive substring) → [`TerminalProgram::Rio`]
/// - `"tmux"` → [`TerminalProgram::Tmux`] (FM-C3: under tmux, XTVERSION answers for tmux itself)
/// - `"screen"` → [`TerminalProgram::Screen`]
#[must_use]
pub fn program_from_xtversion(version: &str) -> Option<TerminalProgram> {
    // Order matters only where a marker could be a substring of another (none currently overlap);
    // kept as a fixed sequence of checks so a future addition documents its own position.
    if version.contains("kitty") {
        Some(TerminalProgram::Kitty)
    } else if version.contains("ghostty") {
        Some(TerminalProgram::Ghostty)
    } else if version.contains("WezTerm") {
        Some(TerminalProgram::WezTerm)
    } else if version.contains("iTerm2") {
        Some(TerminalProgram::Iterm2)
    } else if version.contains("Alacritty") {
        Some(TerminalProgram::Alacritty)
    } else if version.contains("foot") {
        Some(TerminalProgram::Foot)
    } else if version.contains("tmux") {
        Some(TerminalProgram::Tmux)
    } else if version.contains("screen") {
        Some(TerminalProgram::Screen)
    } else if version.to_ascii_lowercase().contains("rio") {
        Some(TerminalProgram::Rio)
    } else {
        None
    }
}

/// Best-effort program-name matching against the `TERM_PROGRAM` environment variable.
///
/// `TERM_PROGRAM` values are each terminal's own convention, not standardized; this matches the
/// documented values for terminals that set it. Returns `None` for an absent or unrecognized value.
#[must_use]
pub fn program_from_term_program(term_program: &str) -> Option<TerminalProgram> {
    match term_program {
        "iTerm.app" => Some(TerminalProgram::Iterm2),
        "Apple_Terminal" => Some(TerminalProgram::AppleTerminal),
        "vscode" => Some(TerminalProgram::VsCode),
        "WezTerm" => Some(TerminalProgram::WezTerm),
        "ghostty" => Some(TerminalProgram::Ghostty),
        "tmux" => Some(TerminalProgram::Tmux),
        "rio" => Some(TerminalProgram::Rio),
        _ => None,
    }
}

/// Best-effort program-name matching against the `TERM` environment variable.
///
/// `TERM` is the least reliable identity signal (FM-C1/C2: stale, wrong, or describing a mux rather
/// than the outer terminal), so this is consulted last, after XTVERSION and `TERM_PROGRAM`.
#[must_use]
pub fn program_from_term(term: &str) -> Option<TerminalProgram> {
    match term {
        "xterm-kitty" => Some(TerminalProgram::Kitty),
        "alacritty" => Some(TerminalProgram::Alacritty),
        "foot" | "foot-extra" => Some(TerminalProgram::Foot),
        "tmux-256color" | "tmux" => Some(TerminalProgram::Tmux),
        "screen" | "screen-256color" => Some(TerminalProgram::Screen),
        _ => None,
    }
}

/// Derives a [`TerminalIdentity`] from an XTVERSION reply (when one arrived) and environment
/// variables, using an injected [`EnvSource`] so this logic is purely testable.
///
/// Signal priority, strongest first:
///
/// 1. **XTVERSION reply** ([`program_from_xtversion`]): the terminal's own self-report, when the
///    probe bundle received one.
/// 2. **`TERM_PROGRAM`** ([`program_from_term_program`]): set by many terminals' own shell
///    integration; used to fill in `program` when XTVERSION was silent, and consulted for `version`
///    fallback text (`TERM_PROGRAM_VERSION`) when XTVERSION carried none.
/// 3. **`TERM`** ([`program_from_term`]): the least reliable signal (FM-C1/C2), consulted last.
///
/// `mux_stack` is independent of `program`: every mux env var present (`TMUX`, `STY`, `ZELLIJ`)
/// adds its [`Multiplexer`] to the stack regardless of which signal resolved `program`, because
/// FM-C3 means the mux's presence matters even when the *outer* terminal was still identified.
///
/// DA1 is intentionally not consulted here: design 06 and FM-C4 call it "a weak signal, not a
/// feature oracle" for identity precisely because its param shape overlaps across terminals (a real
/// VT100 answers, tmux widens its own params); this function only uses it implicitly in that a
/// caller who has *no* XTVERSION and *no* env signals is expected to leave `program` unresolved
/// rather than guess from DA1 alone. A future slice may add DA1-shape matching as a fourth,
/// lowest-priority signal if evidence justifies it.
#[must_use]
pub fn identity_from_env(xtversion: Option<&str>, env: impl EnvSource) -> TerminalIdentity {
    let mut program = xtversion.and_then(program_from_xtversion);
    let mut version = xtversion.map(ToOwned::to_owned);

    if program.is_none()
        && let Some(term_program) = env("TERM_PROGRAM")
    {
        program = program_from_term_program(&term_program);
    }
    if program.is_none()
        && let Some(term) = env("TERM")
    {
        program = program_from_term(&term);
    }
    if version.is_none() {
        version = env("TERM_PROGRAM_VERSION");
    }

    let mut mux_stack = Vec::new();
    if env("TMUX").is_some() {
        mux_stack.push(Multiplexer::Tmux);
    }
    if env("STY").is_some() {
        mux_stack.push(Multiplexer::Screen);
    }
    if env("ZELLIJ").is_some() {
        mux_stack.push(Multiplexer::Zellij);
    }

    TerminalIdentity {
        program,
        version,
        mux_stack,
    }
}

/// One row of the OSC 8 hyperlink-support env-heuristic table (FM-C12).
///
/// OSC 8 has no query: `supports-hyperlinks` (the de facto sniffing convention this table mirrors)
/// establishes support by checking a fixed set of env vars/values. Each row is either an exact
/// `TERM_PROGRAM`/`TERM` value or a named var whose mere *presence* (any value) is the signal
/// (`WT_SESSION`, `KONSOLE_VERSION`); `VTE_VERSION` additionally needs a numeric-threshold check
/// handled separately in [`infer_hyperlinks`] because it is not a fixed-value match.
#[derive(Clone, Copy, Debug)]
pub struct HyperlinkEnvHeuristic {
    /// The environment variable this row checks.
    pub var: &'static str,
    /// `Some(value)` for an exact-value match (for example `TERM_PROGRAM=iTerm.app`); `None` when
    /// the variable's mere presence is the signal, regardless of value.
    pub value: Option<&'static str>,
}

/// The documented, inspectable OSC 8 hyperlink-support env-heuristic table (FM-C12).
///
/// Mirrors the `supports-hyperlinks` sniff set. Consulted only when nothing queries OSC 8 support
/// directly (nothing does — there is no such query), and only produces
/// [`Evidence::Inferred`] findings, never [`Evidence::Probed`]. `VTE_VERSION >= 5000` is checked
/// separately in [`infer_hyperlinks`] (a threshold, not a value in this table).
pub const HYPERLINK_ENV_HEURISTICS: &[HyperlinkEnvHeuristic] = &[
    HyperlinkEnvHeuristic {
        var: "TERM_PROGRAM",
        value: Some("Hyper"),
    },
    HyperlinkEnvHeuristic {
        var: "TERM_PROGRAM",
        value: Some("iTerm.app"),
    },
    HyperlinkEnvHeuristic {
        var: "TERM_PROGRAM",
        value: Some("WezTerm"),
    },
    HyperlinkEnvHeuristic {
        var: "TERM_PROGRAM",
        value: Some("vscode"),
    },
    HyperlinkEnvHeuristic {
        var: "TERM_PROGRAM",
        value: Some("ghostty"),
    },
    HyperlinkEnvHeuristic {
        var: "TERM",
        value: Some("xterm-kitty"),
    },
    HyperlinkEnvHeuristic {
        var: "TERM",
        value: Some("alacritty"),
    },
    HyperlinkEnvHeuristic {
        var: "DOMTERM",
        value: None,
    },
    HyperlinkEnvHeuristic {
        var: "WT_SESSION",
        value: None,
    },
    HyperlinkEnvHeuristic {
        var: "KONSOLE_VERSION",
        value: None,
    },
];

/// The minimum `VTE_VERSION` (VTE's `MAJOR*10000 + MINOR*100 + MICRO` encoding) that indicates OSC
/// 8 hyperlink support, per the `supports-hyperlinks` convention (FM-C12).
pub const VTE_HYPERLINK_MIN_VERSION: u32 = 5000;

/// Infers OSC 8 hyperlink support from the environment (FM-C12: no query exists).
///
/// Checks [`HYPERLINK_ENV_HEURISTICS`] first, then `VTE_VERSION >= `[`VTE_HYPERLINK_MIN_VERSION`]
/// separately (a threshold check the table can't express as a row). `NO_COLOR`/`FORCE_COLOR` do not
/// affect this finding — they are color overrides, not a hyperlink signal — but a set `FORCE_COLOR`
/// with an otherwise-unmatched environment still leaves this `Unknown`, since forcing color output
/// says nothing about hyperlink support specifically.
///
/// Returns a [`Finding`] whose evidence is always [`Evidence::Inferred`] (a matched row) or
/// [`Evidence::Unknown`] (no row matched) — this finding is never [`Evidence::Probed`], because no
/// query for it exists.
#[must_use]
pub fn infer_hyperlinks(env: impl EnvSource) -> Finding<bool> {
    for heuristic in HYPERLINK_ENV_HEURISTICS {
        let Some(actual) = env(heuristic.var) else {
            continue;
        };
        let matched = match heuristic.value {
            Some(expected) => actual == expected,
            None => true,
        };
        if matched {
            return Finding::inferred(Some(true), heuristic.var);
        }
    }
    if let Some(vte_version) = env("VTE_VERSION")
        && let Ok(version) = vte_version.parse::<u32>()
        && version >= VTE_HYPERLINK_MIN_VERSION
    {
        return Finding::inferred(Some(true), "VTE_VERSION");
    }
    Finding::unknown()
}

/// Infers truecolor (24-bit RGB SGR) support from `COLORTERM` (FM-C12: no query exists).
///
/// `COLORTERM` set to `truecolor` or `24bit` is the de facto signal (the `COLORTERM` workaround
/// FM-C1 cites for truecolor being otherwise inexpressible in terminfo). `NO_COLOR` (any value,
/// per the no-color.org convention) overrides this to `Some(false)` — a color override the caller
/// must respect regardless of what `COLORTERM` says; `FORCE_COLOR` (any non-empty value, the
/// complementary convention) overrides to `Some(true)` when `NO_COLOR` is absent. Both overrides
/// are reported with [`Evidence::Inferred`] naming the override variable, so a consumer can see
/// that a color decision was forced rather than sniffed from `COLORTERM`.
#[must_use]
pub fn infer_truecolor(env: impl EnvSource) -> Finding<bool> {
    if env("NO_COLOR").is_some() {
        return Finding::inferred(Some(false), "NO_COLOR");
    }
    if let Some(colorterm) = env("COLORTERM")
        && (colorterm == "truecolor" || colorterm == "24bit")
    {
        return Finding::inferred(Some(true), "COLORTERM");
    }
    if env("FORCE_COLOR").is_some() {
        return Finding::inferred(Some(true), "FORCE_COLOR");
    }
    Finding::unknown()
}

/// The typed result of the capability probe bundle plus environment inference (design 03/06).
///
/// Every DECRQM/query-backed field is a [`Finding`] whose `value` is `Option<T>` where **`None`
/// means unknown, not unsupported** (FM-C4), and whose [`Evidence`] records how the value was
/// obtained. Build one only through `TokioTerminalSession::probe_capabilities` (available with the
/// optional `tokio` feature on Unix); this type offers no public constructor because a hand-built
/// `Capabilities` would carry no evidence of how it was obtained, which is the whole point of this
/// layer.
///
/// # The four DECRQM booleans
///
/// [`synchronized_output`](Self::synchronized_output) (mode 2026),
/// [`grapheme_clustering`](Self::grapheme_clustering) (mode 2027),
/// [`in_band_resize`](Self::in_band_resize) (mode 2048), and
/// [`bracketed_paste`](Self::bracketed_paste) (mode 2004) each come from a DEC private-mode DECRQM
/// answer: `Some(true)` when the terminal reported the mode set or permanently set, `Some(false)`
/// when reset or permanently reset, and `None` when the terminal did not answer *or* answered "mode
/// not recognized" (value 0). The not-recognized-versus-silent difference is collapsed to `None`
/// value on purpose — both mean "do not assume this feature" — but they carry different `Evidence`:
/// a "not recognized" answer is still `Evidence::Probed` (the terminal *did* answer, just in the
/// negative-unknown way DECRQM allows), while true silence is `Evidence::Unknown`.
///
/// # Env-inferred fields have no query
///
/// [`hyperlinks`](Self::hyperlinks) and [`truecolor`](Self::truecolor) are never
/// [`Evidence::Probed`] — no query exists for either (FM-C12) — so they are always
/// [`Evidence::Inferred`] or [`Evidence::Unknown`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct Capabilities {
    /// Synchronized output (DEC private mode 2026): whether the terminal batches a frame so a
    /// redraw does not tear (FM-V4).
    pub synchronized_output: Finding<bool>,
    /// Grapheme clustering / mode 2027: whether the terminal measures width by grapheme cluster
    /// (FM-P15).
    pub grapheme_clustering: Finding<bool>,
    /// In-band resize (DEC private mode 2048): whether the terminal reports size changes in the
    /// input stream (design 01, R-IN-8).
    pub in_band_resize: Finding<bool>,
    /// Bracketed paste (DEC private mode 2004): whether the terminal brackets pasted text
    /// (FM-P12).
    pub bracketed_paste: Finding<bool>,
    /// The kitty keyboard progressive-enhancement flags the terminal reported active for the
    /// `CSI ? u` query (design 06).
    pub kitty_keyboard: Finding<crate::KittyKeyboardFlags>,
    /// The Primary Device Attributes the terminal reported as the fence (design 03). `None` value
    /// means no DA1 arrived — a fully silent terminal, in which case every other field is also
    /// unknown.
    pub primary_device_attributes: Option<DeviceAttributes>,
    /// The terminal's default foreground colour from OSC 10.
    pub foreground_color: Finding<Rgb>,
    /// The terminal's default background colour from OSC 11.
    pub background_color: Finding<Rgb>,
    /// OSC 8 hyperlink support, inferred from the environment (FM-C12: no query exists).
    pub hyperlinks: Finding<bool>,
    /// Truecolor (24-bit RGB SGR) support, inferred from `COLORTERM` (FM-C12: no query exists).
    pub truecolor: Finding<bool>,
    /// The terminal's derived identity: program, version, and multiplexer stack (R-CAP-5).
    pub identity: TerminalIdentity,
}

impl Capabilities {
    /// Returns `true` when the terminal answered nothing at all — every probe-backed finding is
    /// unknown.
    ///
    /// This is the fully-silent case (a terminal that ignored the probe, or a transport that
    /// swallowed it): unknown across the board, never a claim of unsupported (FM-C4). Env-inferred
    /// fields ([`hyperlinks`](Self::hyperlinks), [`truecolor`](Self::truecolor)) are excluded from
    /// this check: they are independent of whether the terminal answered, so a silent terminal in
    /// an environment with `COLORTERM=truecolor` set is still "all unknown" from the probe's point
    /// of view.
    #[must_use]
    pub fn is_all_unknown(&self) -> bool {
        !self.synchronized_output.is_known()
            && !self.grapheme_clustering.is_known()
            && !self.in_band_resize.is_known()
            && !self.bracketed_paste.is_known()
            && !self.kitty_keyboard.is_known()
            && self.primary_device_attributes.is_none()
            && !self.foreground_color.is_known()
            && !self.background_color.is_known()
    }
}

impl fmt::Display for TerminalProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kitty => f.write_str("kitty"),
            Self::Ghostty => f.write_str("Ghostty"),
            Self::Iterm2 => f.write_str("iTerm2"),
            Self::WezTerm => f.write_str("WezTerm"),
            Self::Alacritty => f.write_str("Alacritty"),
            Self::Foot => f.write_str("foot"),
            Self::Rio => f.write_str("Rio"),
            Self::VsCode => f.write_str("Visual Studio Code"),
            Self::WindowsTerminal => f.write_str("Windows Terminal"),
            Self::AppleTerminal => f.write_str("Terminal.app"),
            Self::Tmux => f.write_str("tmux"),
            Self::Screen => f.write_str("GNU screen"),
            Self::Unknown(text) => write!(f, "unknown ({text})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    fn no_env(_key: &str) -> Option<String> {
        None
    }

    // --- Finding ---------------------------------------------------------------------------------

    #[test]
    fn finding_probed_carries_value_and_evidence() {
        let finding = Finding::probed(Some(true), "DECRQM 2026");
        assert_eq!(finding.value(), Some(&true));
        assert_eq!(finding.evidence(), &Evidence::Probed { via: "DECRQM 2026" });
        assert!(finding.is_known());
    }

    #[test]
    fn finding_probed_can_still_be_unknown_value() {
        // A DECRQM "not recognized" answer: the terminal DID answer (Probed), but the value is
        // still None (unknown, not unsupported — FM-C4).
        let finding: Finding<bool> = Finding::probed(None, "DECRQM 2031");
        assert_eq!(finding.value(), None);
        assert!(!finding.is_known());
        assert_eq!(finding.evidence(), &Evidence::Probed { via: "DECRQM 2031" });
    }

    #[test]
    fn finding_inferred_carries_value_and_evidence() {
        let finding = Finding::inferred(Some(true), "COLORTERM");
        assert_eq!(finding.value(), Some(&true));
        assert_eq!(finding.evidence(), &Evidence::Inferred { via: "COLORTERM" });
        assert!(finding.is_known());
    }

    #[test]
    fn finding_unknown_has_no_value_and_unknown_evidence() {
        let finding: Finding<bool> = Finding::unknown();
        assert_eq!(finding.value(), None);
        assert!(!finding.is_known());
        assert_eq!(finding.evidence(), &Evidence::Unknown);
    }

    #[test]
    fn finding_default_is_unknown() {
        let finding: Finding<bool> = Finding::default();
        assert_eq!(finding, Finding::unknown());
    }

    #[test]
    fn finding_value_copied_round_trips_for_copy_types() {
        let finding = Finding::probed(Some(42u8), "test");
        assert_eq!(finding.value_copied(), Some(42));
    }

    // --- Capabilities::is_all_unknown
    // -------------------------------------------------------------

    #[test]
    fn default_capabilities_are_all_unknown() {
        let caps = Capabilities::default();
        assert!(caps.is_all_unknown());
        assert!(!caps.synchronized_output.is_known());
        assert!(!caps.background_color.is_known());
    }

    #[test]
    fn one_answered_field_is_not_all_unknown() {
        let caps = Capabilities {
            synchronized_output: Finding::probed(Some(true), "DECRQM 2026"),
            ..Capabilities::default()
        };
        assert!(!caps.is_all_unknown());
    }

    #[test]
    fn env_inferred_fields_do_not_affect_is_all_unknown() {
        // A silent terminal in a COLORTERM=truecolor environment is still "all unknown" from the
        // probe's point of view: hyperlinks/truecolor are independent env findings.
        let caps = Capabilities {
            truecolor: Finding::inferred(Some(true), "COLORTERM"),
            hyperlinks: Finding::inferred(Some(true), "TERM_PROGRAM"),
            ..Capabilities::default()
        };
        assert!(caps.is_all_unknown());
    }

    #[test]
    fn rgb_channels_round_trip() {
        let rgb = Rgb::new(0x12, 0x34, 0x56);
        assert_eq!(rgb.red(), 0x12);
        assert_eq!(rgb.green(), 0x34);
        assert_eq!(rgb.blue(), 0x56);
    }

    #[test]
    fn device_attributes_preserve_params() {
        let attrs = DeviceAttributes::new(b"62;1;6".to_vec());
        assert_eq!(attrs.params(), b"62;1;6");
    }

    // --- Identity: XTVERSION parsing ----------------------------------------------------------

    #[test]
    fn program_from_xtversion_recognizes_kitty() {
        assert_eq!(
            program_from_xtversion("kitty(0.35.1)"),
            Some(TerminalProgram::Kitty)
        );
    }

    #[test]
    fn program_from_xtversion_recognizes_ghostty() {
        assert_eq!(
            program_from_xtversion("ghostty 1.0.0"),
            Some(TerminalProgram::Ghostty)
        );
    }

    #[test]
    fn program_from_xtversion_recognizes_wezterm() {
        assert_eq!(
            program_from_xtversion("WezTerm 20240203-110809-5046fc22"),
            Some(TerminalProgram::WezTerm)
        );
    }

    #[test]
    fn program_from_xtversion_recognizes_iterm2() {
        assert_eq!(
            program_from_xtversion("iTerm2 3.5.0"),
            Some(TerminalProgram::Iterm2)
        );
    }

    #[test]
    fn program_from_xtversion_recognizes_alacritty() {
        assert_eq!(
            program_from_xtversion("Alacritty 0.13.2"),
            Some(TerminalProgram::Alacritty)
        );
    }

    #[test]
    fn program_from_xtversion_recognizes_foot() {
        assert_eq!(
            program_from_xtversion("foot(1.16.2)"),
            Some(TerminalProgram::Foot)
        );
    }

    #[test]
    fn program_from_xtversion_recognizes_tmux() {
        // FM-C3: under tmux, XTVERSION answers for tmux itself.
        assert_eq!(
            program_from_xtversion("tmux 3.4"),
            Some(TerminalProgram::Tmux)
        );
    }

    #[test]
    fn program_from_xtversion_unrecognized_returns_none() {
        assert_eq!(program_from_xtversion("some-unknown-terminal 9.9"), None);
    }

    // --- Identity: env cross-check ------------------------------------------------------------

    #[test]
    fn identity_from_env_prefers_xtversion_over_env() {
        let env = env_map(&[("TERM_PROGRAM", "vscode")]);
        let identity = identity_from_env(Some("kitty(0.35.1)"), env);
        assert_eq!(identity.program, Some(TerminalProgram::Kitty));
        assert_eq!(identity.version.as_deref(), Some("kitty(0.35.1)"));
    }

    #[test]
    fn identity_from_env_falls_back_to_term_program() {
        let env = env_map(&[("TERM_PROGRAM", "iTerm.app")]);
        let identity = identity_from_env(None, env);
        assert_eq!(identity.program, Some(TerminalProgram::Iterm2));
    }

    #[test]
    fn identity_from_env_falls_back_to_term_program_vscode() {
        let env = env_map(&[("TERM_PROGRAM", "vscode")]);
        let identity = identity_from_env(None, env);
        assert_eq!(identity.program, Some(TerminalProgram::VsCode));
    }

    #[test]
    fn identity_from_env_falls_back_to_term_when_no_term_program() {
        let env = env_map(&[("TERM", "alacritty")]);
        let identity = identity_from_env(None, env);
        assert_eq!(identity.program, Some(TerminalProgram::Alacritty));
    }

    #[test]
    fn identity_from_env_resolves_nothing_when_silent() {
        let identity = identity_from_env(None, no_env);
        assert_eq!(identity.program, None);
        assert_eq!(identity.version, None);
        assert!(identity.mux_stack.is_empty());
    }

    #[test]
    fn identity_from_env_version_falls_back_to_term_program_version() {
        let env = env_map(&[
            ("TERM_PROGRAM", "iTerm.app"),
            ("TERM_PROGRAM_VERSION", "3.5.0"),
        ]);
        let identity = identity_from_env(None, env);
        assert_eq!(identity.version.as_deref(), Some("3.5.0"));
    }

    // --- Identity: mux_stack -------------------------------------------------------------------

    #[test]
    fn identity_from_env_detects_tmux_in_mux_stack() {
        let env = env_map(&[("TMUX", "/tmp/tmux-1000/default,1234,0")]);
        let identity = identity_from_env(None, env);
        assert_eq!(identity.mux_stack, vec![Multiplexer::Tmux]);
    }

    #[test]
    fn identity_from_env_detects_screen_in_mux_stack() {
        let env = env_map(&[("STY", "1234.pts-0.host")]);
        let identity = identity_from_env(None, env);
        assert_eq!(identity.mux_stack, vec![Multiplexer::Screen]);
    }

    #[test]
    fn identity_from_env_detects_zellij_in_mux_stack() {
        let env = env_map(&[("ZELLIJ", "0")]);
        let identity = identity_from_env(None, env);
        assert_eq!(identity.mux_stack, vec![Multiplexer::Zellij]);
    }

    #[test]
    fn identity_from_env_detects_nested_mux_stack() {
        let env = env_map(&[("TMUX", "x"), ("ZELLIJ", "0")]);
        let identity = identity_from_env(None, env);
        assert_eq!(
            identity.mux_stack,
            vec![Multiplexer::Tmux, Multiplexer::Zellij]
        );
    }

    #[test]
    fn identity_from_env_mux_present_alongside_resolved_program() {
        // FM-C3: mux presence is recorded even when the outer terminal was still identified (here
        // via XTVERSION, which under a real tmux passthrough would actually answer as tmux itself
        // — this test exercises the struct's independence of the two signals, not a live tmux).
        let env = env_map(&[("TMUX", "x")]);
        let identity = identity_from_env(Some("ghostty 1.0.0"), env);
        assert_eq!(identity.program, Some(TerminalProgram::Ghostty));
        assert_eq!(identity.mux_stack, vec![Multiplexer::Tmux]);
    }

    // --- Env-heuristic table: hyperlinks --------------------------------------------------------

    #[test]
    fn infer_hyperlinks_iterm2_term_program() {
        let env = env_map(&[("TERM_PROGRAM", "iTerm.app")]);
        let finding = infer_hyperlinks(env);
        assert_eq!(finding.value(), Some(&true));
        assert_eq!(
            finding.evidence(),
            &Evidence::Inferred {
                via: "TERM_PROGRAM"
            }
        );
    }

    #[test]
    fn infer_hyperlinks_vscode_term_program() {
        let env = env_map(&[("TERM_PROGRAM", "vscode")]);
        let finding = infer_hyperlinks(env);
        assert_eq!(finding.value(), Some(&true));
    }

    #[test]
    fn infer_hyperlinks_kitty_term() {
        let env = env_map(&[("TERM", "xterm-kitty")]);
        let finding = infer_hyperlinks(env);
        assert_eq!(finding.value(), Some(&true));
        assert_eq!(finding.evidence(), &Evidence::Inferred { via: "TERM" });
    }

    #[test]
    fn infer_hyperlinks_vte_version_threshold() {
        let env = env_map(&[("VTE_VERSION", "6003")]);
        let finding = infer_hyperlinks(env);
        assert_eq!(finding.value(), Some(&true));
        assert_eq!(
            finding.evidence(),
            &Evidence::Inferred { via: "VTE_VERSION" }
        );
    }

    #[test]
    fn infer_hyperlinks_vte_version_below_threshold_is_unknown() {
        let env = env_map(&[("VTE_VERSION", "4800")]);
        let finding = infer_hyperlinks(env);
        assert_eq!(finding.value(), None);
        assert_eq!(finding.evidence(), &Evidence::Unknown);
    }

    #[test]
    fn infer_hyperlinks_presence_only_var() {
        let env = env_map(&[("WT_SESSION", "guid-here")]);
        let finding = infer_hyperlinks(env);
        assert_eq!(finding.value(), Some(&true));
        assert_eq!(
            finding.evidence(),
            &Evidence::Inferred { via: "WT_SESSION" }
        );
    }

    #[test]
    fn infer_hyperlinks_no_signal_is_unknown() {
        let finding = infer_hyperlinks(no_env);
        assert_eq!(finding.value(), None);
        assert_eq!(finding.evidence(), &Evidence::Unknown);
    }

    // --- Env-heuristic table: truecolor ---------------------------------------------------------

    #[test]
    fn infer_truecolor_colorterm_truecolor() {
        let env = env_map(&[("COLORTERM", "truecolor")]);
        let finding = infer_truecolor(env);
        assert_eq!(finding.value(), Some(&true));
        assert_eq!(finding.evidence(), &Evidence::Inferred { via: "COLORTERM" });
    }

    #[test]
    fn infer_truecolor_colorterm_24bit() {
        let env = env_map(&[("COLORTERM", "24bit")]);
        let finding = infer_truecolor(env);
        assert_eq!(finding.value(), Some(&true));
    }

    #[test]
    fn infer_truecolor_no_color_overrides_to_false() {
        let env = env_map(&[("NO_COLOR", "1"), ("COLORTERM", "truecolor")]);
        let finding = infer_truecolor(env);
        assert_eq!(finding.value(), Some(&false));
        assert_eq!(finding.evidence(), &Evidence::Inferred { via: "NO_COLOR" });
    }

    #[test]
    fn infer_truecolor_force_color_overrides_to_true() {
        let env = env_map(&[("FORCE_COLOR", "1")]);
        let finding = infer_truecolor(env);
        assert_eq!(finding.value(), Some(&true));
        assert_eq!(
            finding.evidence(),
            &Evidence::Inferred { via: "FORCE_COLOR" }
        );
    }

    #[test]
    fn infer_truecolor_no_signal_is_unknown() {
        let finding = infer_truecolor(no_env);
        assert_eq!(finding.value(), None);
        assert_eq!(finding.evidence(), &Evidence::Unknown);
    }

    #[test]
    fn infer_truecolor_unrelated_colorterm_value_is_unknown() {
        let env = env_map(&[("COLORTERM", "gnome-terminal")]);
        let finding = infer_truecolor(env);
        assert_eq!(finding.value(), None);
    }

    // --- Display -------------------------------------------------------------------------------

    #[test]
    fn terminal_program_display_known_variants() {
        assert_eq!(TerminalProgram::Kitty.to_string(), "kitty");
        assert_eq!(TerminalProgram::Ghostty.to_string(), "Ghostty");
    }

    #[test]
    fn terminal_program_display_unknown_preserves_text() {
        let program = TerminalProgram::Unknown("some-terminal 9.9".to_owned());
        assert_eq!(program.to_string(), "unknown (some-terminal 9.9)");
    }
}
