//! Terminal capabilities: the typed result of the DA1-fenced probe bundle (design 03/06).
//!
//! [`Capabilities`] is what `TerminalSession::probe_capabilities` (blocking, no runtime) and
//! `TokioTerminalSession::probe_capabilities` (available with the optional `tokio` feature on
//! Unix) both return: a struct of typed [`Finding`]s, one per queryable capability the probe
//! bundle asks about in a single write-and-fence round trip, plus [`TerminalIdentity`] and the
//! env-inferred findings that have no query at all (FM-C12). The two drivers share this module's
//! bundle contents and reply-to-field mapping so they can never drift apart; only the I/O differs.
//! Every finding's `value` is `Option<T>`, and
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
//! `.value()` gets the `Option<T>` tri-state; a consumer that needs to tell "the terminal told us
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
//! deciding when to mark findings stale and re-probe, is a caller's concern (design 06 caching
//! policy). A `Capabilities` value carries no staleness of its own — it is the answer as of the
//! probe that produced it.
//!
//! # Detection posture: dumb terminals
//!
//! A dumb terminal is never sent the probe bundle — probing has side effects even when unanswered
//! (FM-C7), and a terminal that does not parse escape sequences echoes them as garbage (FM-C5).
//! Both probe drivers enforce this themselves (R-QRY-5): [`probe_skip_from_env`] detects
//! `TERM=dumb` and the Linux console (`TERM=linux`) before any byte is written, and a skipped
//! probe returns a `Capabilities` whose probe-backed findings are all [`Evidence::Unknown`] with
//! the reason recorded on [`Capabilities::probe_skip`] — an inspectable value, not a silent
//! fallback. [`identity_from_env`] and the env-heuristic findings still run on a skipped probe
//! because they only read env vars and never write to the terminal.

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finding<T> {
    value: Option<T>,
    evidence: Evidence,
}

impl<T> Default for Finding<T> {
    /// Returns [`Finding::unknown`]: no value, no evidence.
    ///
    /// Implemented by hand rather than derived so the value type needs no `Default` of its own —
    /// the default finding holds no value at all, and inventing a default measurement (a zero
    /// [`PixelSize`](crate::PixelSize), say) is exactly what the unknown state exists to avoid.
    fn default() -> Self {
        Self::unknown()
    }
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
/// Identity is itself a [`Finding`], assembled from whichever signals were available rather than
/// carrying one [`Evidence`] for the whole struct: `program`/`version` come from XTVERSION when it
/// answered (`Evidence::Probed { via: "XTVERSION" }`, conceptually — the fields here are the parsed
/// result; see [`identity_from_env`] for how they are derived), cross-checked and, when XTVERSION
/// is silent, filled in from environment variables (`Evidence::Inferred`). DA1's shape is at most a
/// weak signal (FM-C4: it proves nothing about features, and even less about identity — many
/// terminals share widened DA1 attribute lists), used only when nothing else resolved a program.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
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

/// Infers iTerm2 inline-image (OSC 1337 `File`) support from a resolved terminal identity —
/// the protocol has no support query (design 11), so identity is the only honest signal.
///
/// Returns an [`Evidence::Inferred`] `true` when `identity` resolved to a terminal documented to
/// render the protocol: [`TerminalProgram::Iterm2`] itself, or [`TerminalProgram::WezTerm`],
/// which speaks both this protocol and kitty graphics (so a `WezTerm` identity may enable more than
/// one image protocol at once). Every other identity — including none at all — yields
/// [`Evidence::Unknown`], never a `false`: identity fails to *affirm* support, but it cannot
/// prove absence (FM-C4), and this table only ever grows from documented protocol adoptions.
///
/// The result is only as good as the identity behind it. A multiplexer that answers XTVERSION
/// itself resolves `program` to the mux, not the outer terminal (FM-C3), which correctly leaves
/// this unknown — OSC 1337 written into an unaware mux is garbled, whatever the outer terminal
/// renders. This finding is never [`Evidence::Probed`]; a caller that has verified rendering out
/// of band constructs its own finding, exactly as for the kitty transmission gates.
#[must_use]
pub fn infer_iterm2_images(identity: &TerminalIdentity) -> Finding<bool> {
    match identity.program {
        Some(TerminalProgram::Iterm2 | TerminalProgram::WezTerm) => {
            Finding::inferred(Some(true), "terminal identity")
        }
        _ => Finding::unknown(),
    }
}

/// Why a capability probe was skipped without writing a single byte (R-QRY-5, FM-C5).
///
/// Both probe drivers (`TerminalSession::probe_capabilities` and
/// `TokioTerminalSession::probe_capabilities`) consult [`probe_skip_from_env`] before writing the
/// bundle: a terminal that does not parse escape sequences echoes probe bytes as garbage output
/// (FM-C5, notcurses#1828 on the Linux console), so the honest move is to never send them. A
/// skipped probe is recorded on the snapshot as [`Capabilities::probe_skip`] — visible provenance,
/// not a silent fallback — with every probe-backed finding left [`Evidence::Unknown`]: nothing was
/// asked, so nothing is reported as a no-reply.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ProbeSkip {
    /// `TERM=dumb`: the environment declares a terminal that parses nothing.
    TermDumb,
    /// `TERM=linux`: the Linux virtual console, which leaves rogue output from queries it does
    /// not understand (FM-C5).
    ///
    /// Detected from `TERM` rather than the `KDGKBTYPE`-style console ioctl notcurses uses
    /// (notcurses#1828): this crate forbids `unsafe`, and the env signal is what a console login
    /// sets by default. `TERM` is caller-controlled and can lie (FM-C1/C2); a caller that knows
    /// better simply probes a different way or fixes its environment.
    LinuxConsole,
}

impl fmt::Display for ProbeSkip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TermDumb => f.write_str("TERM=dumb"),
            Self::LinuxConsole => f.write_str("Linux console (TERM=linux)"),
        }
    }
}

/// Detects a terminal the capability probe must not be written to (R-QRY-5, FM-C5).
///
/// Returns `Some` for `TERM=dumb` and for the Linux console (`TERM=linux`); `None` otherwise —
/// including when `TERM` is unset, which is merely *unknown* and no reason to withhold an
/// explicitly requested probe. Both probe drivers call this before writing the bundle; it is
/// public so a caller composing its own query flow can apply the identical guard.
#[must_use]
pub fn probe_skip_from_env(env: impl EnvSource) -> Option<ProbeSkip> {
    match env("TERM").as_deref() {
        Some("dumb") => Some(ProbeSkip::TermDumb),
        Some("linux") => Some(ProbeSkip::LinuxConsole),
        _ => None,
    }
}

/// The typed result of the capability probe bundle plus environment inference (design 03/06).
///
/// Every DECRQM/query-backed field is a [`Finding`] whose `value` is `Option<T>` where **`None`
/// means unknown, not unsupported** (FM-C4), and whose [`Evidence`] records how the value was
/// obtained. Build one only through `TerminalSession::probe_capabilities` (blocking, Unix) or
/// `TokioTerminalSession::probe_capabilities` (with the optional `tokio` feature); this type offers
/// no public constructor because a hand-built `Capabilities` would carry no evidence of how it was
/// obtained, which is the whole point of this layer.
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
/// [`hyperlinks`](Self::hyperlinks), [`truecolor`](Self::truecolor), and
/// [`iterm2_images`](Self::iterm2_images) are never [`Evidence::Probed`] — no query exists for
/// any of them (FM-C12) — so they are always [`Evidence::Inferred`] or [`Evidence::Unknown`].
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
    /// iTerm2 inline-image (OSC 1337 `File`) support, inferred from the terminal's identity —
    /// the protocol has no support query, so this finding is never [`Evidence::Probed`].
    ///
    /// `Some(true)` with [`Evidence::Inferred`] when [`identity`](Self::identity) resolved to a
    /// terminal documented to render the protocol (iTerm2 itself, or `WezTerm`, which speaks both
    /// this protocol and kitty graphics); [`Evidence::Unknown`] for every other identity —
    /// identity alone never proves absence (FM-C4), it just fails to affirm support. Derived by
    /// [`infer_iterm2_images`] from the best identity available: re-derived when an XTVERSION
    /// reply improves on the env-only identity, so under a multiplexer that answers XTVERSION
    /// itself (FM-C3) the finding honestly stays unknown even if `TERM_PROGRAM` named the outer
    /// terminal.
    pub iterm2_images: Finding<bool>,
    /// kitty graphics protocol support, probed with the protocol's own `a=q` query.
    ///
    /// `Some(true)` means the terminal answered the query with `OK` — it speaks the protocol.
    /// `Some(false)` means it answered with an error: it parses graphics escapes but refused the
    /// canonical one-pixel probe, so emitting images to it is not safe. `None` means silence —
    /// *unknown*, never unsupported (FM-C4): a multiplexer may simply have swallowed the APC.
    pub kitty_graphics: Finding<bool>,
    /// The text-area size in pixels, from the XTWINOPS `CSI 14 t` query.
    ///
    /// A terminal that answered with zero dimensions gets a `Probed` finding with a `None` value:
    /// the zeros are an admission of not knowing, never turned into a fabricated geometry (FM-Z5).
    pub text_area_pixels: Finding<crate::PixelSize>,
    /// The character-cell size in pixels, from the XTWINOPS `CSI 16 t` query.
    ///
    /// This is the cells-to-pixels conversion needed to size image placements. Zero answers stay
    /// `None` exactly as in [`text_area_pixels`](Self::text_area_pixels) (FM-Z5).
    pub cell_size: Finding<crate::PixelSize>,
    /// The terminal's derived identity: program, version, and multiplexer stack (R-CAP-5).
    pub identity: TerminalIdentity,
    /// `Some` when the probe driver detected a dumb terminal and never wrote the bundle
    /// (R-QRY-5, FM-C5); `None` when the probe actually ran.
    ///
    /// A skipped probe leaves every probe-backed finding [`Evidence::Unknown`] — indistinguishable
    /// by value from a fully silent terminal — so this field is the inspectable difference between
    /// "we asked and nothing answered" and "we refused to ask" (see [`ProbeSkip`]). The
    /// env-inferred fields ([`hyperlinks`](Self::hyperlinks), [`truecolor`](Self::truecolor),
    /// [`iterm2_images`](Self::iterm2_images), [`identity`](Self::identity)) are still populated
    /// on a skipped probe: they only read environment variables and never write to the terminal.
    pub probe_skip: Option<ProbeSkip>,
}

impl Capabilities {
    /// Returns `true` when the terminal answered nothing at all — every probe-backed finding is
    /// unknown.
    ///
    /// This is the fully-silent case (a terminal that ignored the probe, or a transport that
    /// swallowed it): unknown across the board, never a claim of unsupported (FM-C4). Env-inferred
    /// fields ([`hyperlinks`](Self::hyperlinks), [`truecolor`](Self::truecolor),
    /// [`iterm2_images`](Self::iterm2_images)) are excluded from this check: they are independent
    /// of whether the terminal answered, so a silent terminal in an environment with
    /// `COLORTERM=truecolor` set is still "all unknown" from the probe's point of view.
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
            && !self.kitty_graphics.is_known()
            && !self.text_area_pixels.is_known()
            && !self.cell_size.is_known()
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

// --- The DA1-fenced capability probe bundle (design 03) -----------------------------------------
//
// Shared between the two query drivers ([`crate::TerminalSession`]'s synchronous
// `probe_capabilities` and [`crate::TokioTerminalSession::probe_capabilities`]): the bundle's
// shape (which queries, what order, DA1 last as the fence) and how a reply populates
// [`Capabilities`] must never drift between the two, so both drivers build the same commands here
// and hand replies to the same [`store_bundle_reply`]. Only the I/O — how each driver waits for
// and reads bytes — differs, and stays in `session.rs`/`tokio_session.rs`.
//
// Gated to exactly the platforms that have a consumer: the synchronous driver is Unix-only
// (`unix`), the Tokio driver is `all(feature = "tokio", any(unix, windows))` — combined, `any(unix,
// all(feature = "tokio", windows))` (the `unix` arm already covers Tokio-on-Unix). Without this
// gate a default-feature Windows cross-compile has neither consumer, and every item here is
// legitimately dead code — `just check-cross` catches exactly that class of hole.
#[cfg(any(unix, all(feature = "tokio", windows)))]
pub(crate) use bundle::{
    ProbeBundle, env_seeded_capabilities, probe_bundle_commands, skipped_capabilities,
    store_bundle_reply,
};

#[cfg(any(unix, all(feature = "tokio", windows)))]
mod bundle {
    use super::{Capabilities, EnvSource, Finding, ProbeSkip, identity_from_env, std_env_source};

    /// The snapshot a probe driver returns when [`super::probe_skip_from_env`] said not to write
    /// the bundle (R-QRY-5, FM-C5).
    ///
    /// Shared between the sync and Tokio drivers like the rest of this module, so the two skip
    /// paths can never drift: env-inferred findings and env-only identity populated (they never
    /// touch the terminal), every probe-backed finding honestly [`super::Evidence::Unknown`] —
    /// nothing was asked, so nothing is a no-reply — and the skip recorded as visible provenance
    /// on [`Capabilities::probe_skip`].
    pub(crate) fn skipped_capabilities(skip: ProbeSkip, env: impl EnvSource) -> Capabilities {
        Capabilities {
            probe_skip: Some(skip),
            ..env_seeded_capabilities(env)
        }
    }

    /// The [`Capabilities`] snapshot every probe path starts from: the findings that come from
    /// the environment alone, with every probe-backed field [`super::Evidence::Unknown`].
    ///
    /// [`Capabilities::hyperlinks`] and [`Capabilities::truecolor`] never come from a terminal
    /// reply (FM-C12: no query exists); [`Capabilities::identity`] starts env-only and is
    /// overwritten by `store_bundle_reply` when an XTVERSION reply arrives, which also re-derives
    /// the identity-keyed [`Capabilities::iterm2_images`]. Shared between the sync and Tokio
    /// drivers and [`skipped_capabilities`] so the three construction sites can never drift.
    pub(crate) fn env_seeded_capabilities(env: impl EnvSource) -> Capabilities {
        let identity = identity_from_env(None, &env);
        Capabilities {
            hyperlinks: super::infer_hyperlinks(&env),
            truecolor: super::infer_truecolor(&env),
            iterm2_images: super::infer_iterm2_images(&identity),
            identity,
            ..Capabilities::default()
        }
    }

    /// The DEC private modes the capability probe bundle queries, and the [`Capabilities`] field
    /// each answer sets. Kept as one table so the write side, the register side, and the
    /// collect side stay in agreement.
    const PROBE_MODES: [ProbeMode; 4] = [
        ProbeMode {
            mode: 2026,
            field: CapabilityField::SynchronizedOutput,
        },
        ProbeMode {
            mode: 2027,
            field: CapabilityField::GraphemeClustering,
        },
        ProbeMode {
            mode: 2048,
            field: CapabilityField::InBandResize,
        },
        ProbeMode {
            mode: 2004,
            field: CapabilityField::BracketedPaste,
        },
    ];

    /// One DEC private mode the probe asks about, and which [`Capabilities`] boolean its answer
    /// sets.
    #[derive(Clone, Copy)]
    struct ProbeMode {
        mode: u16,
        field: CapabilityField,
    }

    /// Which [`Capabilities`] boolean a DECRQM answer populates.
    #[derive(Clone, Copy)]
    enum CapabilityField {
        SynchronizedOutput,
        GraphemeClustering,
        InBandResize,
        BracketedPaste,
    }

    /// The image id the bundle's kitty graphics `a=q` query uses.
    ///
    /// The query action stores nothing and replaces nothing terminal-side, so any nonzero id is
    /// safe even if an application uses the same one; `u32::MAX` is simply the value least likely
    /// to collide with app-chosen ids (which typically count up from 1). The id's only job is to
    /// let the correlator match the response echo (the discriminator).
    const GRAPHICS_PROBE_IMAGE_ID: u32 = u32::MAX;

    /// Builds the probe bundle's request bytes in the fixed order the fence semantics depend on:
    /// DA1 **last**, so it is the fence every other query races against.
    pub(crate) fn probe_bundle_commands() -> crate::CommandBuffer {
        let mut buffer = crate::CommandBuffer::new();
        buffer
            .command(crate::commands::terminal::request_xtversion())
            .command(crate::commands::terminal::request_kitty_keyboard_flags())
            .command(crate::commands::graphics::kitty::query_support(
                GRAPHICS_PROBE_IMAGE_ID,
            ))
            .command(crate::commands::terminal::request_text_area_pixels())
            .command(crate::commands::terminal::request_cell_size())
            .command(crate::commands::osc::request_foreground_color())
            .command(crate::commands::osc::request_background_color());
        for probe in PROBE_MODES {
            buffer.command(crate::commands::terminal::request_dec_private_mode(
                probe.mode,
            ));
        }
        buffer.command(crate::commands::terminal::request_primary_device_attributes());
        buffer
    }

    /// The registered expectation ids of one capability probe bundle (design 03).
    ///
    /// Keyed for the fence and for reply collection: `fence` is the DA1 expectation whose
    /// completion resolves the rest as no-reply; the others are paired with the
    /// [`Capabilities`] field their reply fills.
    pub(crate) struct ProbeBundle {
        fence: Option<crate::correlate::ExpectationId>,
        xtversion: Option<crate::correlate::ExpectationId>,
        kitty: Option<crate::correlate::ExpectationId>,
        graphics: Option<crate::correlate::ExpectationId>,
        text_area_pixels: Option<crate::correlate::ExpectationId>,
        cell_size: Option<crate::correlate::ExpectationId>,
        foreground: Option<crate::correlate::ExpectationId>,
        background: Option<crate::correlate::ExpectationId>,
        modes: Vec<(crate::correlate::ExpectationId, CapabilityField)>,
    }

    impl ProbeBundle {
        /// Registers every bundle expectation on `correlator`, DA1 last as the fence.
        ///
        /// # Panics
        ///
        /// Never in practice: the bundle's expectations are all mutually distinct (distinct modes,
        /// colours, and frames), so none can conflict with another mid-registration.
        pub(crate) fn register(correlator: &mut crate::correlate::Correlator) -> Self {
            let register = |correlator: &mut crate::correlate::Correlator,
                            expectation: crate::correlate::Expectation| {
                correlator
                    .register(expectation)
                    .expect("bundle expectations never overlap: distinct modes/colours/frames")
            };
            let xtversion = Some(register(
                correlator,
                crate::correlate::Expectation::XtVersion,
            ));
            let kitty = Some(register(
                correlator,
                crate::correlate::Expectation::KittyKeyboardFlags,
            ));
            let graphics = Some(register(
                correlator,
                crate::correlate::Expectation::KittyGraphics {
                    image_id: GRAPHICS_PROBE_IMAGE_ID,
                },
            ));
            let text_area_pixels = Some(register(
                correlator,
                crate::correlate::Expectation::TextAreaPixels,
            ));
            let cell_size = Some(register(
                correlator,
                crate::correlate::Expectation::CellSize,
            ));
            let foreground = Some(register(
                correlator,
                crate::correlate::Expectation::OscColor {
                    which: crate::report::OscColorKind::Foreground,
                },
            ));
            let background = Some(register(
                correlator,
                crate::correlate::Expectation::OscColor {
                    which: crate::report::OscColorKind::Background,
                },
            ));
            let modes = PROBE_MODES
                .iter()
                .map(|probe| {
                    let id = register(
                        correlator,
                        crate::correlate::Expectation::DecPrivateMode { mode: probe.mode },
                    );
                    (id, probe.field)
                })
                .collect();
            let fence = Some(register(
                correlator,
                crate::correlate::Expectation::PrimaryDeviceAttributes,
            ));

            Self {
                fence,
                xtversion,
                kitty,
                graphics,
                text_area_pixels,
                cell_size,
                foreground,
                background,
                modes,
            }
        }

        /// Returns every registered id in the bundle (fence included), for whole-bundle resolution.
        pub(crate) fn ids(&self) -> Vec<crate::correlate::ExpectationId> {
            let mut ids = Vec::new();
            ids.extend(self.fence);
            ids.extend(self.xtversion);
            ids.extend(self.kitty);
            ids.extend(self.graphics);
            ids.extend(self.text_area_pixels);
            ids.extend(self.cell_size);
            ids.extend(self.foreground);
            ids.extend(self.background);
            ids.extend(self.modes.iter().map(|(id, _)| *id));
            ids
        }

        /// The DA1 fence's expectation id.
        pub(crate) fn fence(&self) -> Option<crate::correlate::ExpectationId> {
            self.fence
        }

        /// Resolves every still-registered bundle expectation with `resolution`.
        ///
        /// Called once the DA1 fence completes, or once the whole-probe deadline/EOF/cancellation
        /// ends the probe. A still-pending expectation is one whose reply never arrived;
        /// resolving it removes it so a later matching reply passes through as an ordinary
        /// event (design 03 rule 4) instead of completing a stale probe.
        pub(crate) fn resolve_all(
            &self,
            correlator: &mut crate::correlate::Correlator,
            resolution: crate::correlate::Resolution,
        ) {
            for id in self.ids() {
                correlator.resolve(id, resolution);
            }
        }
    }

    /// The evidence label recorded on every DECRQM-backed [`Finding`] the probe bundle populates.
    ///
    /// One stable string per mode number so a consumer's `Evidence::Probed { via }` match names the
    /// exact query that answered (design 06).
    const fn decrqm_evidence(mode: u16) -> &'static str {
        match mode {
            2026 => "DECRQM 2026",
            2027 => "DECRQM 2027",
            2048 => "DECRQM 2048",
            2004 => "DECRQM 2004",
            _ => "DECRQM",
        }
    }

    /// Records one bundle reply into the matching [`Capabilities`] field, as a [`Finding`] with
    /// [`Evidence::Probed`] naming the query that answered.
    ///
    /// The XTVERSION reply also feeds `capabilities.identity` (design 06, R-CAP-5: identity is a
    /// finding too) via [`identity_from_env`], cross-checked against the environment.
    pub(crate) fn store_bundle_reply(
        bundle: &ProbeBundle,
        id: crate::correlate::ExpectationId,
        reply: crate::correlate::Reply,
        capabilities: &mut Capabilities,
    ) {
        match reply {
            crate::correlate::Reply::XtVersion(report) => {
                let version = report.version().to_owned();
                capabilities.identity = identity_from_env(Some(&version), std_env_source);
                // The identity just improved from env-only to wire-informed, so the
                // identity-keyed finding is re-derived from it — an XTVERSION that resolves to a
                // mux answering for itself (FM-C3) honestly downgrades an env-inferred `true`
                // back to unknown.
                capabilities.iterm2_images = super::infer_iterm2_images(&capabilities.identity);
            }
            crate::correlate::Reply::KittyKeyboardFlags(bits) => {
                capabilities.kitty_keyboard =
                    Finding::probed(Some(crate::KittyKeyboardFlags::from_bits(bits)), "CSI ?u");
            }
            crate::correlate::Reply::OscColor(report) => match report.kind() {
                crate::report::OscColorKind::Foreground => {
                    capabilities.foreground_color = Finding::probed(Some(report.rgb()), "OSC 10");
                }
                crate::report::OscColorKind::Background => {
                    capabilities.background_color = Finding::probed(Some(report.rgb()), "OSC 11");
                }
            },
            crate::correlate::Reply::DecPrivateMode(report) => {
                store_mode_reply(bundle, id, report, capabilities);
            }
            crate::correlate::Reply::PrimaryDeviceAttributes(attrs) => {
                capabilities.primary_device_attributes = Some(attrs.into());
            }
            crate::correlate::Reply::KittyGraphics(report) => {
                // OK means the terminal loaded the probe's one-pixel query: it speaks the
                // protocol. An error response still proves it parses graphics escapes, but
                // refusing the canonical probe means emitting images is not safe — recorded as a
                // probed `false`.
                capabilities.kitty_graphics =
                    Finding::probed(Some(report.is_ok()), "kitty graphics a=q");
            }
            crate::correlate::Reply::TextAreaPixels(report) => {
                // `pixel_size()` is `None` for a zero answer: Probed evidence, unknown value
                // (FM-Z5 — answered zeros is an admission, not a measurement).
                capabilities.text_area_pixels = Finding::probed(report.pixel_size(), "CSI 14 t");
            }
            crate::correlate::Reply::CellSize(report) => {
                capabilities.cell_size = Finding::probed(report.pixel_size(), "CSI 16 t");
            }
            // The bundle never registers CursorPosition/TerminalStatus expectations, so those reply
            // variants cannot appear here.
            crate::correlate::Reply::CursorPosition(_)
            | crate::correlate::Reply::TerminalStatus(_) => {}
        }
    }

    /// Stores a DECRQM answer into the [`Capabilities`] finding its mode maps to (via the bundle),
    /// as [`Evidence::Probed`] naming the exact mode queried.
    ///
    /// The mode's enabled/reset/permanently-* state becomes a `Some(true)`/`Some(false)` finding
    /// value; a "not recognized" (value 0) answer leaves the finding's value `None` but its
    /// evidence is still `Probed` — the terminal *did* answer, just in the negative-unknown way
    /// DECRQM allows (FM-C4). The bundle maps the completing expectation id back to which of
    /// the four fields it fills.
    fn store_mode_reply(
        bundle: &ProbeBundle,
        id: crate::correlate::ExpectationId,
        report: crate::report::DecPrivateModeReport,
        capabilities: &mut Capabilities,
    ) {
        let Some((_, field)) = bundle.modes.iter().find(|(mode_id, _)| *mode_id == id) else {
            return;
        };
        let enabled = report.is_enabled();
        let evidence_via = decrqm_evidence(report.mode());
        let finding = Finding::probed(enabled, evidence_via);
        match field {
            CapabilityField::SynchronizedOutput => capabilities.synchronized_output = finding,
            CapabilityField::GraphemeClustering => capabilities.grapheme_clustering = finding,
            CapabilityField::InBandResize => capabilities.in_band_resize = finding,
            CapabilityField::BracketedPaste => capabilities.bracketed_paste = finding,
        }
    }
} // mod bundle

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

    #[test]
    fn infer_iterm2_images_iterm2_identity() {
        let identity = super::TerminalIdentity {
            program: Some(super::TerminalProgram::Iterm2),
            ..Default::default()
        };
        let finding = super::infer_iterm2_images(&identity);
        assert_eq!(finding.value_copied(), Some(true));
        assert_eq!(
            finding.evidence(),
            &Evidence::Inferred {
                via: "terminal identity"
            }
        );
    }

    #[test]
    fn infer_iterm2_images_wezterm_speaks_the_protocol_too() {
        let identity = super::TerminalIdentity {
            program: Some(super::TerminalProgram::WezTerm),
            ..Default::default()
        };
        let finding = super::infer_iterm2_images(&identity);
        assert_eq!(finding.value_copied(), Some(true));
    }

    #[test]
    fn infer_iterm2_images_other_identity_is_unknown_never_false() {
        // FM-C4: an identity that fails to affirm support is unknown, not unsupported.
        for program in [
            Some(super::TerminalProgram::Kitty),
            Some(super::TerminalProgram::Tmux),
            Some(super::TerminalProgram::Unknown("mystery".to_owned())),
            None,
        ] {
            let identity = super::TerminalIdentity {
                program,
                ..Default::default()
            };
            let finding = super::infer_iterm2_images(&identity);
            assert_eq!(finding.value_copied(), None, "for {identity:?}");
            assert_eq!(finding.evidence(), &Evidence::Unknown, "for {identity:?}");
        }
    }

    #[test]
    #[cfg(any(unix, all(feature = "tokio", windows)))]
    fn env_seeded_capabilities_derives_iterm2_images_from_env_identity() {
        let env = env_map(&[("TERM_PROGRAM", "iTerm.app")]);
        let caps = super::env_seeded_capabilities(env);
        assert_eq!(caps.identity.program, Some(super::TerminalProgram::Iterm2));
        assert_eq!(caps.iterm2_images.value_copied(), Some(true));
        // Probe-backed findings stay untouched by the env seed.
        assert_eq!(caps.kitty_graphics.evidence(), &Evidence::Unknown);
    }

    // --- Dumb-terminal probe skip (R-QRY-5, FM-C5) -----------------------------------------------

    #[test]
    fn probe_skip_detects_term_dumb() {
        let env = env_map(&[("TERM", "dumb")]);
        assert_eq!(probe_skip_from_env(env), Some(ProbeSkip::TermDumb));
    }

    #[test]
    fn probe_skip_detects_linux_console() {
        let env = env_map(&[("TERM", "linux")]);
        assert_eq!(probe_skip_from_env(env), Some(ProbeSkip::LinuxConsole));
    }

    #[test]
    fn probe_skip_none_for_a_capable_terminal() {
        let env = env_map(&[("TERM", "xterm-256color")]);
        assert_eq!(probe_skip_from_env(env), None);
    }

    #[test]
    fn probe_skip_none_when_term_is_unset() {
        // Unset TERM is unknown, not dumb: an explicitly requested probe still runs.
        assert_eq!(probe_skip_from_env(no_env), None);
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
