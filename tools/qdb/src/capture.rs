//! The capture/recording core (design 05, "The live-capture harness").
//!
//! The pure half of `qdb capture`: it decides *what* to probe (the [`ProbePlan`], built from the
//! database under the replay-class gate), models what a run produced (the [`ProbeReport`]), and
//! mints every artifact from that report — per-entry sidecars (`db/captures/FORMAT.md`),
//! report-direction fixtures with `origin=capture:` headers (the quarantine replacement), the
//! conformance results seed, and the scripted fixture-array edits. Everything here is exercised
//! by unit tests feeding canned reports, no terminal needed.
//!
//! The live half is the conformance runner (`crate::runner`) driving a `Target` adapter
//! (`crate::targets`): capture mode is the runner loop with recording on, and the report it
//! returns is this module's input.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::escape;
use crate::model::{Database, Sequence};

/// How a query entry's probe bytes are constructed, so the reason is auditable per entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeSource {
    /// The entry's existing query fixture already carries complete, sendable bytes (parameters
    /// baked in): unescape it and send verbatim. This is the common case.
    FixtureBytes,
}

/// An entry's replay safety class (`db/README.md`, "The replay rubric").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayClass {
    /// Pure output or a query; no lasting state change.
    Safe,
    /// Changes a terminal mode; reversible by its inverse.
    Modal,
    /// Irreversible or resizes the real terminal.
    Destructive,
}

impl ReplayClass {
    /// Parses the db field value.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "safe" => Some(Self::Safe),
            "modal" => Some(Self::Modal),
            "destructive" => Some(Self::Destructive),
            _ => None,
        }
    }

    /// The db field spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Modal => "modal",
            Self::Destructive => "destructive",
        }
    }
}

/// Which replay classes a plan may include. `safe` is always in; everything else is the
/// explicit opt-in the DECSLPP incident rule demands — nothing `modal`/`destructive` reaches a
/// live terminal blind.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AllowedClasses {
    /// Include `replay = "modal"` entries (e.g. the DECRQM mode queries).
    pub modal: bool,
    /// Include `replay = "destructive"` entries.
    pub destructive: bool,
}

impl AllowedClasses {
    /// The default gate: safe entries only.
    pub const SAFE_ONLY: Self = Self {
        modal: false,
        destructive: false,
    };

    /// Whether `class` passes this gate.
    #[must_use]
    pub const fn allows(self, class: ReplayClass) -> bool {
        match class {
            ReplayClass::Safe => true,
            ReplayClass::Modal => self.modal,
            ReplayClass::Destructive => self.destructive,
        }
    }
}

/// One entry the runner will probe: its id, the reply id it `responds` with, the exact bytes to
/// send, and the family fixture directory the minted reply fixture belongs in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeSpec {
    /// The query entry id.
    pub query_id: String,
    /// The reply entry id (`responds` target) whose fixtures array gains the minted fixture.
    pub reply_id: String,
    /// Raw bytes to write to the terminal.
    pub query_bytes: Vec<u8>,
    /// Fixture directory family, e.g. `ecma48`, derived from the query entry's own fixture path.
    pub family_dir: String,
    /// How `query_bytes` was constructed.
    pub source: ProbeSource,
    /// The entry's replay class, so the runner can enforce its gate independently of the plan
    /// builder's.
    pub replay: ReplayClass,
}

/// An entry that carries a `responds` link but is deliberately not probed, with the reason — the
/// deliverable's "skip honestly" rule. Silence is data; *unprobeable* is a different datum: the
/// probe was never sent because sending it is unsafe or ill-defined.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unprobeable {
    /// The query entry id.
    pub query_id: String,
    /// The reply entry id (`responds` target).
    pub reply_id: String,
    /// Why the entry is not probed.
    pub reason: String,
}

/// A query entry excluded from this run by its replay class (the DECSLPP incident rule): a real
/// query the run chose not to send blind, recorded so results say "skipped, because modal" rather
/// than omitting it (which the matrix would read as "no evidence").
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Skipped {
    /// The query entry id.
    pub query_id: String,
    /// The reply entry id (`responds` target).
    pub reply_id: String,
    /// The replay class that excluded it.
    pub class: ReplayClass,
}

/// The runner's probe plan: what will be sent, what is skipped unsafe, and what is skipped by
/// replay class. Every eligible query entry lands in exactly one of the three lists, so a results
/// file can account for all of them.
#[derive(Clone, Debug, Default)]
pub struct ProbePlan {
    /// Entries that will be probed.
    pub specs: Vec<ProbeSpec>,
    /// Entries deliberately not probed because sending them is unsafe or ill-defined.
    pub unprobeable: Vec<Unprobeable>,
    /// Query entries excluded by their replay class under this run's opt-in gate.
    pub skipped: Vec<Skipped>,
}

/// A per-entry support verdict (results schema v2). The runner produces the first four
/// automatically; `Unsupported` is the one negative-evidence verdict it emits, for a reply that
/// merely echoes the query (the terminal passing input through rather than answering).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// The target answered with a genuine (non-echo) reply — it implements the query.
    Supported,
    /// Bytes came back but they are not a real reply (they echo the query) — evidence the query
    /// is not implemented, distinct from pure silence.
    Unsupported,
    /// The target was silent before the deadline. Absence of a reply is data, not proof of
    /// non-support — kept separate from `unsupported`.
    NoReply,
    /// A query the harness will not send blind (side-effecting or ill-defined).
    Unprobeable,
    /// A real query this run excluded by its replay class; carries the class.
    Skipped(ReplayClass),
}

impl Verdict {
    /// The `verdict` field spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::NoReply => "no-reply",
            Self::Unprobeable => "unprobeable",
            Self::Skipped(_) => "skipped",
        }
    }

    /// Every valid `verdict` field value, for validation.
    pub const ALL_STRS: [&'static str; 5] = [
        "supported",
        "unsupported",
        "no-reply",
        "unprobeable",
        "skipped",
    ];
}

/// How the target's version string was obtained, recorded so a reader knows how much to trust it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionSource {
    /// The terminal named itself via XTVERSION — authoritative.
    XtVersion,
    /// The adapter's out-of-band probe (`tmux -V`, `betamax --version`) — a fallback.
    Hint,
    /// Neither produced a version.
    None,
}

impl VersionSource {
    /// The `version_source` field spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::XtVersion => "xtversion",
            Self::Hint => "hint",
            Self::None => "none",
        }
    }
}

/// Run metadata the results file records beside its rows: how the target is hosted, the geometry
/// it ran under, the runner build, and how the version was resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResultsMeta {
    /// Adapter-kind spelling for the `adapter` field (`in-process`/`pty-headless`/`attended`).
    pub adapter: String,
    /// How `version` was obtained.
    pub version_source: VersionSource,
    /// Session columns.
    pub cols: u16,
    /// Session rows.
    pub rows: u16,
    /// The runner build, e.g. `qdb 0.0.0`.
    pub runner: String,
}

impl ResultsMeta {
    /// The current `qdb` build string for the `runner` field.
    #[must_use]
    pub fn runner_version() -> String {
        format!("qdb {}", env!("CARGO_PKG_VERSION"))
    }
}

/// Query entries that answer a `responds` reply but whose probe would be side-effecting or
/// ill-defined to send blind. `replay=safe` classifies them as reversible output, but *safe to
/// replay* is not *a pure query*: `RequestUpload` opens an interactive upload negotiation and
/// `Button` registers a UI element whose reply only arrives on a later click. We record them
/// unprobeable with a reason rather than guessing a benign form. Keyed by entry id.
fn unprobeable_reasons() -> BTreeMap<&'static str, &'static str> {
    let mut m = BTreeMap::new();
    m.insert(
        "iterm2.osc1337.request_upload",
        "RequestUpload starts an interactive file-upload negotiation, not a pure reply query; \
         sending it blind would hang or prompt. Probe construction is ambiguous.",
    );
    m.insert(
        "iterm2.osc1337.button_custom",
        "Button registers a custom UI button; its `responds` event only fires on a later user \
         click, so a blind send yields no synchronous reply. Probe construction is ambiguous.",
    );
    m
}

impl ProbePlan {
    /// Builds the probe plan for a target from the database: every entry that has a `responds`
    /// link and a replay class `allowed` admits (`safe` always; `modal`/`destructive` only by
    /// explicit opt-in — never blind), minus the honestly-unprobeable set, optionally filtered
    /// to `only` entry ids.
    ///
    /// Query bytes come from the entry's own query fixture: it already holds complete, sendable
    /// bytes (see the harvested fixtures), so unescaping it is the single source of truth and no
    /// second byte-construction opinion can drift from the recorded command form.
    #[must_use]
    pub fn build(db: &Database, only: &[String], allowed: AllowedClasses) -> Self {
        let reasons = unprobeable_reasons();
        let mut plan = ProbePlan::default();

        for entry in db.entries() {
            let Some(reply_id) = &entry.responds else {
                continue;
            };
            if !only.is_empty() && !only.iter().any(|id| id == &entry.id) {
                continue;
            }
            // `qdb validate` guarantees the class parses; skip defensively if it ever doesn't.
            let Some(class) = ReplayClass::parse(&entry.replay) else {
                continue;
            };
            // Safety gate (the DECSLPP incident rule): a class the run did not opt into is
            // recorded as skipped, not omitted, so results account for the whole query surface.
            if !allowed.allows(class) {
                plan.skipped.push(Skipped {
                    query_id: entry.id.clone(),
                    reply_id: reply_id.clone(),
                    class,
                });
                continue;
            }
            if let Some(reason) = reasons.get(entry.id.as_str()) {
                plan.unprobeable.push(Unprobeable {
                    query_id: entry.id.clone(),
                    reply_id: reply_id.clone(),
                    reason: (*reason).to_string(),
                });
                continue;
            }
            match probe_bytes(entry) {
                Ok((bytes, family_dir)) => plan.specs.push(ProbeSpec {
                    query_id: entry.id.clone(),
                    reply_id: reply_id.clone(),
                    query_bytes: bytes,
                    family_dir,
                    source: ProbeSource::FixtureBytes,
                    replay: class,
                }),
                Err(reason) => plan.unprobeable.push(Unprobeable {
                    query_id: entry.id.clone(),
                    reply_id: reply_id.clone(),
                    reason,
                }),
            }
        }
        plan
    }
}

/// Derives an entry's fixture family directory from its first fixture path. `query_bytes` is left
/// empty here — the pure plan cannot read files; the binary fills bytes with
/// [`ProbePlan::read_bytes`] once it has the repo on disk, and tests build specs directly.
///
/// Returns `Err(reason)` (unprobeable) when the entry has no fixture to source bytes from, since we
/// refuse to fabricate query bytes — the whole point of the quarantine.
fn probe_bytes(entry: &Sequence) -> Result<(Vec<u8>, String), String> {
    let Some(fixture) = entry.fixtures.first() else {
        return Err("no query fixture to source probe bytes from".to_string());
    };
    let family_dir = fixture
        .strip_prefix("fixtures/")
        .and_then(|rest| rest.split('/').next())
        .ok_or_else(|| format!("fixture path {fixture:?} is not under fixtures/<family>/"))?
        .to_string();
    Ok((Vec::new(), family_dir))
}

impl ProbePlan {
    /// Fills each spec's `query_bytes` by reading and unescaping its source fixture, resolving the
    /// path against `repo_root`. The pure `build` leaves bytes empty; the binary calls this once
    /// it has the repo on disk. Returns the fixture-relative paths it read, for logging.
    ///
    /// # Errors
    ///
    /// Returns an error naming the first fixture that cannot be read or has no payload.
    pub fn read_bytes(&mut self, repo_root: &std::path::Path, db: &Database) -> Result<(), String> {
        let fixtures: BTreeMap<&str, &str> = db
            .entries()
            .filter_map(|e| Some((e.id.as_str(), e.fixtures.first()?.as_str())))
            .collect();
        for spec in &mut self.specs {
            let rel = fixtures
                .get(spec.query_id.as_str())
                .ok_or_else(|| format!("no fixture recorded for {}", spec.query_id))?;
            let bytes =
                std::fs::read(repo_root.join(rel)).map_err(|e| format!("reading {rel}: {e}"))?;
            let payload = escape::payload_after_header(&bytes)
                .ok_or_else(|| format!("fixture {rel} has no header line"))?;
            spec.query_bytes = escape::unescape(payload);
        }
        Ok(())
    }
}

/// The status of one query after the probe ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeStatus {
    /// The terminal replied with at least one byte before the timeout.
    Answered,
    /// The terminal was still silent when the timeout elapsed.
    Timeout,
}

/// One probed query's outcome, recorded by the runner: the entry id, the escaped raw reply
/// bytes, and the status. Timestamp and identity are recorded once per run in [`ProbeReport`],
/// not per line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeLine {
    /// The query entry id that was sent.
    pub query_id: String,
    /// The reply entry id (`responds` target) the reply belongs to.
    pub reply_id: String,
    /// Raw reply bytes, escaped per `FORMAT.md` (empty string when none arrived).
    pub reply_escaped: String,
    /// Number of raw reply bytes captured.
    pub reply_len: usize,
    /// Whether the reply arrived or the probe timed out.
    pub status: ProbeStatus,
    /// Reply bytes (escaped) that arrived only after the deadline had already declared this
    /// query silent — recorded as data on the query they belong to, never attributed to the
    /// next one. Absent for on-time replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_reply_escaped: Option<String>,
    /// The "reply" is byte-identical to the query — almost certainly echo, the exact
    /// fabrication failure mode the quarantine exists for. A suspect line never mints a
    /// fixture.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub echo_suspect: bool,
}

/// The terminal's identity as probed: DA1 and XTVERSION replies (raw, escaped), plus any
/// out-of-band version string the adapter supplied (e.g. `tmux -V`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The target kind, e.g. `tmux` or `betamax`.
    pub target: String,
    /// Escaped raw DA1 reply (`CSI c` -> primary device attributes), empty if silent.
    #[serde(default)]
    pub da1_escaped: String,
    /// Escaped raw XTVERSION reply (`CSI > q`), empty if silent.
    #[serde(default)]
    pub xtversion_escaped: String,
    /// A best-effort human version string (from XTVERSION payload or `tmux -V`).
    #[serde(default)]
    pub version: String,
}

/// The whole probe run: identity, a UTC timestamp the orchestrator stamps, and one line per query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    /// Terminal identity record.
    pub identity: Identity,
    /// RFC-3339-ish UTC timestamp the orchestrator passes in (`date -u +%FT%TZ`).
    pub timestamp: String,
    /// One result per probed query.
    pub lines: Vec<ProbeLine>,
}

/// A minted artifact: a repo-relative path and its full file contents, ready to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    /// Path relative to the repo root.
    pub path: String,
    /// Full file contents.
    pub contents: String,
}

/// Mints the per-target, per-entry sidecar JSON files under `db/captures/<target>/<id>.json`.
///
/// Every probed entry gets a sidecar — answered *and* timed-out — because silence is data the
/// results and caniuse seed must record (design 05: a report entry needs a ref or a live capture;
/// a recorded timeout is the capture that says "this target does not answer this query").
#[must_use]
pub fn mint_sidecars(report: &ProbeReport) -> Vec<Artifact> {
    let target = &report.identity.target;
    report
        .lines
        .iter()
        .map(|line| {
            let mut value = serde_json::json!({
                "query_id": line.query_id,
                "reply_id": line.reply_id,
                "target": target,
                "identity": report.identity,
                "timestamp": report.timestamp,
                "status": line.status,
                "reply_len": line.reply_len,
                "reply_escaped": line.reply_escaped,
            });
            // Anomaly fields appear only when set, so clean captures keep the M7-S2 sidecar
            // byte-for-byte (see db/captures/FORMAT.md).
            if let (Some(obj), Some(late)) = (value.as_object_mut(), &line.late_reply_escaped) {
                obj.insert("late_reply_escaped".to_string(), late.as_str().into());
            }
            if line.echo_suspect {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("echo_suspect".to_string(), true.into());
                }
            }
            // Pretty-print for review, trailing newline for a clean diff. Serializing a
            // `serde_json::Value` never fails; default to empty rather than carry a panic.
            let json = serde_json::to_string_pretty(&value).unwrap_or_default();
            Artifact {
                path: format!("db/captures/{target}/{}.json", line.query_id),
                contents: format!("{json}\n"),
            }
        })
        .collect()
}

/// Mints the report-direction fixture for one answered line, with the quarantine-replacing
/// `origin=capture:<target>-<version>` header.
///
/// Path: `fixtures/<family>/<name>_report_capture_<target>.seq`, where `<family>` is the query
/// entry's fixture family and `<name>` is the reply id's mnemonic tail — a stable, collision-free
/// name derived from the reply the fixture pins.
///
/// An echo-suspect line never mints: bytes that merely mirror the query are the fabrication
/// failure mode the quarantine exists for, and a fixture is a trust artifact.
#[must_use]
pub fn mint_fixture(spec: &ProbeSpec, line: &ProbeLine, report: &ProbeReport) -> Option<Artifact> {
    if line.status != ProbeStatus::Answered || line.echo_suspect {
        return None;
    }
    let target = &report.identity.target;
    let name = fixture_name(&spec.reply_id, target);
    let path = format!("fixtures/{}/{name}.seq", spec.family_dir);
    let contents = format!(
        "#! direction=terminal-to-host origin=capture:{} date={}\n{}\n",
        capture_origin(target, &report.identity.version),
        report.timestamp_date(),
        line.reply_escaped,
    );
    Some(Artifact { path, contents })
}

/// Builds the `<target>-<version>` origin slug, deduping when the version string already leads with
/// the target name. tmux's XTVERSION reply is literally `tmux 3.7b`, so `tmux` + that version would
/// read `tmux-tmux-3.7b`; we collapse it to `tmux-3.7b`. betamax hosts ghostty (`libghostty`),
/// which shares no prefix, so it stays `betamax-libghostty`.
fn capture_origin(target: &str, version: &str) -> String {
    let slug = version_slug(version);
    let deduped = slug
        .strip_prefix(&format!("{target}-"))
        .filter(|_| slug.starts_with(&format!("{target}-")))
        .unwrap_or(&slug);
    if deduped.is_empty() {
        target.to_string()
    } else {
        format!("{target}-{deduped}")
    }
}

/// The repo-relative fixture path a line would mint to, without needing the bytes — used to add
/// the path to the reply entry's `fixtures` array even before writing the file.
#[must_use]
pub fn fixture_path(spec: &ProbeSpec, target: &str) -> String {
    format!(
        "fixtures/{}/{}.seq",
        spec.family_dir,
        fixture_name(&spec.reply_id, target)
    )
}

/// Derives the fixture base name from a reply id and target: the reply id with dots joined by
/// underscores, then `_report_capture_<target>`.
fn fixture_name(reply_id: &str, target: &str) -> String {
    let stem = reply_id.replace('.', "_");
    format!("{stem}_report_capture_{target}")
}

/// Slugifies a version string for a fixture origin header: keep alphanumerics, dots, and dashes;
/// collapse everything else to a dash; default to `unknown` when empty.
fn version_slug(version: &str) -> String {
    if version.trim().is_empty() {
        return "unknown".to_string();
    }
    let mut out = String::new();
    for c in version.trim().chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

impl ProbeReport {
    /// The date portion (`YYYY-MM-DD`) of the run timestamp for fixture `date=` headers.
    #[must_use]
    pub fn timestamp_date(&self) -> &str {
        self.timestamp.split('T').next().unwrap_or(&self.timestamp)
    }
}

/// The support verdict a probed line earned: a genuine reply is `Supported`, an echo is
/// `Unsupported` (bytes came, but they only mirror the query), silence is `NoReply`.
#[must_use]
pub fn verdict_of(line: &ProbeLine) -> Verdict {
    match line.status {
        ProbeStatus::Answered if line.echo_suspect => Verdict::Unsupported,
        ProbeStatus::Answered => Verdict::Supported,
        ProbeStatus::Timeout => Verdict::NoReply,
    }
}

/// Renders the conformance results file (schema v2) for one target: `db/results/<target>.toml`.
///
/// Run metadata (adapter kind, geometry, runner build, how the version was obtained) heads the
/// file; then one `[[result]]` per query entry carrying a support verdict — probed entries
/// (`supported`/`unsupported`/`no-reply`), `unprobeable` entries, and replay-class-`skipped`
/// entries. Nothing is omitted: a query the runner did not send is recorded with why, so the
/// matrix distinguishes "we chose not to probe" from "no evidence at all". `qdb validate` checks
/// every id exists and every verdict is valid. See `db/README.md`, "Results schema".
#[must_use]
pub fn render_results(report: &ProbeReport, plan: &ProbePlan, meta: &ResultsMeta) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Conformance results for {} — generated by qdb, do not hand-edit.",
        report.identity.target
    );
    let _ = writeln!(
        out,
        "# Machines write support claims; humans write entries and citations (design 05)."
    );
    let _ = writeln!(out, "target = {:?}", report.identity.target);
    let _ = writeln!(out, "version = {:?}", report.identity.version);
    let _ = writeln!(out, "version_source = {:?}", meta.version_source.as_str());
    let _ = writeln!(out, "adapter = {:?}", meta.adapter);
    let _ = writeln!(out, "captured = {:?}", report.timestamp);
    let _ = writeln!(out, "runner = {:?}", meta.runner);
    let _ = writeln!(
        out,
        "geometry = {{ cols = {}, rows = {} }}",
        meta.cols, meta.rows
    );

    // Probed entries: the verdict follows from the wire outcome.
    for line in &report.lines {
        let _ = writeln!(out);
        let _ = writeln!(out, "[[result]]");
        let _ = writeln!(out, "id = {:?}", line.query_id);
        let _ = writeln!(out, "reply_id = {:?}", line.reply_id);
        let _ = writeln!(out, "verdict = {:?}", verdict_of(line).as_str());
        let _ = writeln!(out, "reply_len = {}", line.reply_len);
    }
    // Entries the harness refuses to send blind.
    for u in &plan.unprobeable {
        let _ = writeln!(out);
        let _ = writeln!(out, "[[result]]");
        let _ = writeln!(out, "id = {:?}", u.query_id);
        let _ = writeln!(out, "reply_id = {:?}", u.reply_id);
        let _ = writeln!(out, "verdict = \"unprobeable\"");
        let _ = writeln!(out, "reply_len = 0");
    }
    // Real queries this run excluded by replay class.
    for s in &plan.skipped {
        let _ = writeln!(out);
        let _ = writeln!(out, "[[result]]");
        let _ = writeln!(out, "id = {:?}", s.query_id);
        let _ = writeln!(out, "reply_id = {:?}", s.reply_id);
        let _ = writeln!(out, "verdict = \"skipped\"");
        let _ = writeln!(out, "skipped_class = {:?}", s.class.as_str());
        let _ = writeln!(out, "reply_len = 0");
    }
    out
}

/// Adds `new_fixture` to the `fixtures = [...]` array of the entry with id `entry_id` in a family
/// TOML file's text, preserving all other formatting. Returns the edited text, or `None` if the
/// entry or its `fixtures` array is not found (a signal to the caller, not a silent no-op).
///
/// Scripted TOML edit rather than parse-and-reserialize: the family files are hand-aligned record
/// cards (design 05's review-ability constraint), and `toml` reserialization would flatten that
/// alignment across every unrelated entry. We touch exactly the one array line.
#[must_use]
pub fn add_fixture_to_entry(text: &str, entry_id: &str, new_fixture: &str) -> Option<String> {
    let id_line = format!("id          = {entry_id:?}");
    let start = text.find(&id_line)?;
    // Search within this entry's block only: from its id line to the next `[[sequence]]` or EOF.
    let block_end = text[start..]
        .find("\n[[sequence]]")
        .map_or(text.len(), |rel| start + rel);
    let block = &text[start..block_end];
    let fx_rel = block.find("\nfixtures")?;
    let fx_abs = start + fx_rel + 1; // skip the leading newline
    let line_end = text[fx_abs..]
        .find('\n')
        .map_or(text.len(), |rel| fx_abs + rel);
    let fx_line = &text[fx_abs..line_end];

    // Idempotence: if already present, return the text unchanged.
    if fx_line.contains(&format!("{new_fixture:?}")) {
        return Some(text.to_string());
    }

    let open = fx_line.find('[')?;
    let close = fx_line.rfind(']')?;
    let inner = fx_line[open + 1..close].trim();
    let rebuilt = if inner.is_empty() {
        format!("fixtures    = [{new_fixture:?}]")
    } else {
        format!("fixtures    = [{inner}, {new_fixture:?}]")
    };

    let mut edited = String::with_capacity(text.len() + rebuilt.len());
    edited.push_str(&text[..fx_abs]);
    edited.push_str(&rebuilt);
    edited.push_str(&text[line_end..]);
    Some(edited)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(target: &str, lines: Vec<ProbeLine>) -> ProbeReport {
        ProbeReport {
            identity: Identity {
                target: target.to_string(),
                da1_escaped: "\\e[?1;2c".to_string(),
                xtversion_escaped: String::new(),
                version: "3.7b".to_string(),
            },
            timestamp: "2026-07-07T04:00:00Z".to_string(),
            lines,
        }
    }

    fn answered(query: &str, reply: &str, escaped: &str) -> ProbeLine {
        ProbeLine {
            query_id: query.to_string(),
            reply_id: reply.to_string(),
            reply_escaped: escaped.to_string(),
            reply_len: escape::unescape(escaped.as_bytes()).len(),
            status: ProbeStatus::Answered,
            late_reply_escaped: None,
            echo_suspect: false,
        }
    }

    fn timed_out(query: &str, reply: &str) -> ProbeLine {
        ProbeLine {
            query_id: query.to_string(),
            reply_id: reply.to_string(),
            reply_escaped: String::new(),
            reply_len: 0,
            status: ProbeStatus::Timeout,
            late_reply_escaped: None,
            echo_suspect: false,
        }
    }

    #[test]
    fn sidecar_minted_for_answered_and_timeout() {
        let r = report(
            "tmux",
            vec![
                answered("csi.dsr.cursor_position", "csi.cpr", "\\e[24;1R"),
                timed_out("osc.11.background_query", "osc.11.background_report"),
            ],
        );
        let sidecars = mint_sidecars(&r);
        assert_eq!(sidecars.len(), 2);
        assert_eq!(
            sidecars[0].path,
            "db/captures/tmux/csi.dsr.cursor_position.json"
        );
        assert!(sidecars[0].contents.contains("\"status\": \"answered\""));
        assert!(sidecars[1].contents.contains("\"status\": \"timeout\""));
        assert!(sidecars[1].contents.contains("\"reply_len\": 0"));
    }

    #[test]
    fn fixture_minted_only_when_answered() {
        let spec = ProbeSpec {
            query_id: "csi.dsr.cursor_position".to_string(),
            reply_id: "csi.cpr".to_string(),
            query_bytes: b"\x1b[6n".to_vec(),
            family_dir: "ecma48".to_string(),
            source: ProbeSource::FixtureBytes,
            replay: ReplayClass::Safe,
        };
        let r = report(
            "tmux",
            vec![answered("csi.dsr.cursor_position", "csi.cpr", "\\e[24;1R")],
        );
        let fx = mint_fixture(&spec, &r.lines[0], &r).unwrap();
        assert_eq!(fx.path, "fixtures/ecma48/csi_cpr_report_capture_tmux.seq");
        assert_eq!(
            fx.contents,
            "#! direction=terminal-to-host origin=capture:tmux-3.7b date=2026-07-07\n\\e[24;1R\n"
        );

        let timeout_line = timed_out("csi.dsr.cursor_position", "csi.cpr");
        assert!(mint_fixture(&spec, &timeout_line, &r).is_none());

        // An echo-suspect "reply" never becomes a fixture — that is the fabrication guard.
        let mut echo_line = answered("csi.dsr.cursor_position", "csi.cpr", "\\e[6n");
        echo_line.echo_suspect = true;
        assert!(mint_fixture(&spec, &echo_line, &r).is_none());
    }

    #[test]
    fn fixture_path_matches_minted_path() {
        let spec = ProbeSpec {
            query_id: "osc.11.background_query".to_string(),
            reply_id: "osc.11.background_report".to_string(),
            query_bytes: vec![],
            family_dir: "osc".to_string(),
            source: ProbeSource::FixtureBytes,
            replay: ReplayClass::Safe,
        };
        assert_eq!(
            fixture_path(&spec, "betamax"),
            "fixtures/osc/osc_11_background_report_report_capture_betamax.seq"
        );
    }

    #[test]
    fn sidecar_anomaly_fields_appear_only_when_set() {
        let clean = report(
            "tmux",
            vec![answered(
                "csi.da.primary",
                "csi.da.primary_report",
                "\\e[?1;2c",
            )],
        );
        let sidecar = &mint_sidecars(&clean)[0];
        assert!(!sidecar.contents.contains("late_reply_escaped"));
        assert!(!sidecar.contents.contains("echo_suspect"));

        let mut late = timed_out("osc.52.clipboard_query", "osc.52.clipboard_report");
        late.late_reply_escaped = Some("\\e]52;c;YQ==\\e\\\\".to_string());
        let mut echo = answered("csi.da.primary", "csi.da.primary_report", "\\e[c");
        echo.echo_suspect = true;
        let anomalous = report("tmux", vec![late, echo]);
        let sidecars = mint_sidecars(&anomalous);
        assert!(
            sidecars[0]
                .contents
                .contains("\"late_reply_escaped\": \"\\\\e]52;c;YQ==\\\\e\\\\\\\\\"")
        );
        assert!(sidecars[1].contents.contains("\"echo_suspect\": true"));
    }

    /// A minimal in-memory database for plan-gating tests.
    fn tiny_db() -> Database {
        use crate::model::{Family, Sequence};
        let entry = |id: &str, replay: &str| Sequence {
            id: id.to_string(),
            name: id.to_string(),
            description: "test entry".to_string(),
            direction: "host-to-terminal".to_string(),
            syntax: None,
            params: vec![],
            refs: vec![],
            fixtures: vec![format!("fixtures/ecma48/{}.seq", id.replace('.', "_"))],
            replay: replay.to_string(),
            responds: Some(format!("{id}_report")),
            notes: None,
            superseded_by: None,
        };
        Database {
            families: vec![Family {
                name: "test".to_string(),
                entries: vec![
                    entry("t.safe_query", "safe"),
                    entry("t.modal_query", "modal"),
                    entry("t.destructive_query", "destructive"),
                ],
            }],
            sources: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn plan_admits_replay_classes_only_by_opt_in() {
        let db = tiny_db();
        let ids = |allowed: AllowedClasses| -> Vec<String> {
            ProbePlan::build(&db, &[], allowed)
                .specs
                .iter()
                .map(|s| s.query_id.clone())
                .collect()
        };

        assert_eq!(ids(AllowedClasses::SAFE_ONLY), ["t.safe_query"]);
        assert_eq!(
            ids(AllowedClasses {
                modal: true,
                destructive: false
            }),
            ["t.safe_query", "t.modal_query"]
        );
        assert_eq!(
            ids(AllowedClasses {
                modal: true,
                destructive: true
            }),
            ["t.safe_query", "t.modal_query", "t.destructive_query"]
        );
        // The spec carries its class for the runner's independent gate.
        let plan = ProbePlan::build(
            &db,
            &[],
            AllowedClasses {
                modal: true,
                destructive: true,
            },
        );
        assert_eq!(plan.specs[1].replay, ReplayClass::Modal);
    }

    #[test]
    fn version_slug_sanitizes() {
        assert_eq!(version_slug("3.7b"), "3.7b");
        assert_eq!(version_slug("ghostty 1.2.0"), "ghostty-1.2.0");
        assert_eq!(version_slug(""), "unknown");
        assert_eq!(version_slug("  "), "unknown");
    }

    #[test]
    fn capture_origin_dedupes_target_prefix() {
        // tmux's XTVERSION says "tmux 3.7b" -> tmux-3.7b, not tmux-tmux-3.7b.
        assert_eq!(capture_origin("tmux", "tmux 3.7b"), "tmux-3.7b");
        // betamax hosts ghostty: no shared prefix, kept whole.
        assert_eq!(
            capture_origin("betamax", "libghostty"),
            "betamax-libghostty"
        );
        // A bare version with no tool name is just target-version.
        assert_eq!(capture_origin("tmux", "3.7b"), "tmux-3.7b");
        // No version at all falls back to the target alone.
        assert_eq!(capture_origin("betamax", ""), "betamax-unknown");
    }

    fn meta() -> ResultsMeta {
        ResultsMeta {
            adapter: "pty-headless".to_string(),
            version_source: VersionSource::XtVersion,
            cols: 120,
            rows: 40,
            runner: "qdb 0.0.0".to_string(),
        }
    }

    #[test]
    fn results_v2_records_every_verdict_class() {
        // One of each source: probed-answered, probed-echo, probed-silent, unprobeable, skipped.
        let r = report(
            "betamax",
            vec![
                answered("csi.da.primary", "csi.da.primary_report", "\\e[?62;c"),
                {
                    let mut echo = answered("csi.dsr.status", "csi.dsr.ok", "\\e[5n");
                    echo.echo_suspect = true;
                    echo
                },
                timed_out("osc.52.clipboard_query", "osc.52.clipboard_report"),
            ],
        );
        let plan = ProbePlan {
            specs: Vec::new(),
            unprobeable: vec![Unprobeable {
                query_id: "iterm2.osc1337.request_upload".to_string(),
                reply_id: "iterm2.osc1337.upload_reply".to_string(),
                reason: "interactive".to_string(),
            }],
            skipped: vec![Skipped {
                query_id: "dec.mode.origin.query".to_string(),
                reply_id: "dec.mode.origin.report".to_string(),
                class: ReplayClass::Modal,
            }],
        };
        let toml = render_results(&r, &plan, &meta());
        assert!(toml.contains("target = \"betamax\""));
        assert!(toml.contains("version_source = \"xtversion\""));
        assert!(toml.contains("adapter = \"pty-headless\""));
        assert!(toml.contains("runner = \"qdb 0.0.0\""));
        assert!(toml.contains("geometry = { cols = 120, rows = 40 }"));
        assert!(toml.contains("verdict = \"supported\""));
        assert!(toml.contains("verdict = \"unsupported\"")); // the echo line
        assert!(toml.contains("verdict = \"no-reply\""));
        assert!(toml.contains("verdict = \"unprobeable\""));
        assert!(toml.contains("verdict = \"skipped\""));
        assert!(toml.contains("skipped_class = \"modal\""));

        // Parseable and shaped as we expect: 3 probed + 1 unprobeable + 1 skipped = 5 rows.
        let parsed: toml::Value = toml::from_str(&toml).unwrap();
        assert_eq!(parsed["result"].as_array().unwrap().len(), 5);
        assert_eq!(parsed["geometry"]["cols"].as_integer(), Some(120));
    }

    #[test]
    fn add_fixture_appends_to_empty_array() {
        let text = "\
[[sequence]]
id          = \"csi.cpr\"
name        = \"CPR\"
fixtures    = []
replay      = \"safe\"

[[sequence]]
id          = \"csi.other\"
fixtures    = []
";
        let out = add_fixture_to_entry(
            text,
            "csi.cpr",
            "fixtures/ecma48/csi_cpr_report_capture_tmux.seq",
        )
        .unwrap();
        assert!(out.contains(
            "fixtures    = [\"fixtures/ecma48/csi_cpr_report_capture_tmux.seq\"]\nreplay"
        ));
        // The sibling entry is untouched.
        assert!(out.contains("id          = \"csi.other\"\nfixtures    = []"));
    }

    #[test]
    fn add_fixture_appends_to_nonempty_array() {
        let text = "\
[[sequence]]
id          = \"csi.cpr\"
fixtures    = [\"fixtures/ecma48/a.seq\"]
replay      = \"safe\"
";
        let out = add_fixture_to_entry(text, "csi.cpr", "fixtures/ecma48/b.seq").unwrap();
        assert!(out.contains(
            "fixtures    = [\"fixtures/ecma48/a.seq\", \"fixtures/ecma48/b.seq\"]\nreplay"
        ));
    }

    #[test]
    fn add_fixture_is_idempotent() {
        let text = "\
[[sequence]]
id          = \"csi.cpr\"
fixtures    = [\"fixtures/ecma48/b.seq\"]
";
        let out = add_fixture_to_entry(text, "csi.cpr", "fixtures/ecma48/b.seq").unwrap();
        assert_eq!(out, text);
    }

    #[test]
    fn add_fixture_missing_entry_returns_none() {
        let text = "[[sequence]]\nid          = \"csi.cpr\"\nfixtures    = []\n";
        assert!(add_fixture_to_entry(text, "csi.nope", "x.seq").is_none());
    }
}
