//! The conformance orchestrator: `qdb capture` and `qdb run` over the Target adapters.
//!
//! The thin layer between the CLI and the runner: it builds the probe plan from the database,
//! constructs the requested adapter, executes the runner loop, and writes artifacts. Capture
//! mode is the same loop with recording on — it mints sidecars, `origin=capture:` fixtures, the
//! fixture-array edits, and the results seed; run mode writes the results seed alone (the
//! conformance pass — no trust artifacts minted). All minting logic stays pure in
//! `crate::capture`, unit-tested without a terminal.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::capture::{
    self, AllowedClasses, Artifact, ProbePlan, ProbeReport, ProbeStatus, ResultsMeta,
};
use crate::model::Database;
use crate::runner::{self, IdentityCheck, RunnerOptions};
use crate::targets::betamax::BetamaxTarget;
use crate::targets::kitty::KittyTarget;
use crate::targets::tmux::TmuxTarget;
use crate::targets::{AdapterKind, Target};

/// Which adapter the orchestrator drives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetKind {
    /// A detached tmux session hosting the byte relay.
    Tmux,
    /// A betamax-hosted headless ghostty-vt hosting the byte relay via an on-the-fly tape.
    Betamax,
    /// A minimized, task-hidden kitty OS window hosting the byte relay.
    Kitty,
}

impl TargetKind {
    /// Parses the `--target` value.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tmux" => Some(Self::Tmux),
            "betamax" => Some(Self::Betamax),
            "kitty" => Some(Self::Kitty),
            _ => None,
        }
    }

    /// The target slug used in artifact paths and origin headers.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Betamax => "betamax",
            Self::Kitty => "kitty",
        }
    }

    /// Constructs the adapter (nothing launches until the runner starts it).
    #[must_use]
    pub fn make(self) -> Box<dyn Target> {
        match self {
            Self::Tmux => Box::new(TmuxTarget::new()),
            Self::Betamax => Box::new(BetamaxTarget::new()),
            Self::Kitty => Box::new(KittyTarget::new()),
        }
    }

    /// How this target is hosted, for the results `adapter` field. Every wired target is
    /// PTY-hosted (relay-based); the value is per-adapter, not assumed, so a future in-process
    /// adapter reports its own kind.
    #[must_use]
    pub const fn adapter_kind(self) -> AdapterKind {
        match self {
            Self::Tmux | Self::Betamax | Self::Kitty => AdapterKind::PtyHosted,
        }
    }
}

/// Runs a full capture pass for one target and writes every artifact under `repo_root`:
/// sidecars, answered-reply fixtures, fixture-array edits, and the results seed.
///
/// # Errors
///
/// Returns an error if the target tool is missing, the session cannot be established, the
/// transport dies mid-run, or an artifact cannot be written.
pub fn capture(
    db: &Database,
    repo_root: &Path,
    kind: TargetKind,
    only: &[String],
) -> Result<Summary, String> {
    let (plan, report, meta) = execute(db, repo_root, kind, only, AllowedClasses::SAFE_ONLY)?;
    let artifacts = mint_all(db, repo_root, &plan, &report, &meta)?;
    for artifact in &artifacts {
        write_artifact(repo_root, artifact)?;
    }
    Ok(Summary::from_report(&report, &plan))
}

/// Runs the conformance pass for one target: the same loop as capture, recording off — only
/// the `db/results/<target>.toml` seed is written, no fixtures or sidecars minted.
///
/// # Errors
///
/// Returns an error if the target tool is missing, the session cannot be established, the
/// transport dies mid-run, or the results file cannot be written.
pub fn conformance(
    db: &Database,
    repo_root: &Path,
    kind: TargetKind,
    only: &[String],
    allowed: AllowedClasses,
) -> Result<Summary, String> {
    let (plan, report, meta) = execute(db, repo_root, kind, only, allowed)?;
    let results = Artifact {
        path: format!("db/results/{}.toml", report.identity.target),
        contents: capture::render_results(&report, &plan, &meta),
    };
    write_artifact(repo_root, &results)?;
    Ok(Summary::from_report(&report, &plan))
}

/// Builds the plan, runs the loop against the adapter, and surfaces run anomalies on stderr
/// (identity mismatch, strays, teardown failure) so they are visible even when artifacts land.
fn execute(
    db: &Database,
    repo_root: &Path,
    kind: TargetKind,
    only: &[String],
    allowed: AllowedClasses,
) -> Result<(ProbePlan, ProbeReport, ResultsMeta), String> {
    let mut plan = ProbePlan::build(db, only, allowed);
    plan.read_bytes(repo_root, db)?;

    let opts = RunnerOptions {
        allow_modal: allowed.modal,
        allow_destructive: allowed.destructive,
        ..RunnerOptions::default()
    };
    let mut target = kind.make();
    let timestamp = utc_timestamp();
    let outcome = runner::run(target.as_mut(), &plan, &timestamp, &opts)?;

    let meta = ResultsMeta {
        adapter: kind.adapter_kind().as_results_str().to_string(),
        version_source: outcome.version_source,
        cols: opts.cols,
        rows: opts.rows,
        runner: ResultsMeta::runner_version(),
    };

    match &outcome.identity_check {
        IdentityCheck::Verified { .. } => {}
        IdentityCheck::Unverifiable { reason } => {
            eprintln!("qdb {}: identity unverifiable: {reason}", kind.slug());
        }
        IdentityCheck::Mismatch {
            expected,
            wire_name,
        } => {
            return Err(format!(
                "identity mismatch: adapter expected {expected:?} on the wire but the terminal \
                 reported {wire_name:?} — refusing to write results keyed to the wrong terminal"
            ));
        }
    }
    for stray in &outcome.strays {
        eprintln!(
            "qdb {}: stray bytes before first query (recorded nowhere): {stray}",
            kind.slug()
        );
    }
    if let Some(e) = &outcome.teardown_error {
        eprintln!("qdb {}: teardown: {e}", kind.slug());
    }
    Ok((plan, outcome.report, meta))
}

/// Mints every artifact for a capture run: sidecars, answered-reply fixtures, the fixture-array
/// edits on the report entries, and the results seed. Pure given the report — the live-terminal
/// split is upstream — so the whole minting pass is unit-tested in `capture.rs` and
/// integration-checked here.
fn mint_all(
    db: &Database,
    repo_root: &Path,
    plan: &ProbePlan,
    report: &ProbeReport,
    meta: &ResultsMeta,
) -> Result<Vec<Artifact>, String> {
    let mut artifacts = capture::mint_sidecars(report);

    // Map query id -> spec for fixture family/name.
    let specs: std::collections::BTreeMap<&str, &capture::ProbeSpec> = plan
        .specs
        .iter()
        .map(|s| (s.query_id.as_str(), s))
        .collect();

    // Track pending fixture-array edits per family file, applied cumulatively so multiple answered
    // entries in one file each land.
    let mut file_edits: std::collections::BTreeMap<PathBuf, String> =
        std::collections::BTreeMap::new();

    for line in &report.lines {
        let Some(spec) = specs.get(line.query_id.as_str()) else {
            continue;
        };
        let Some(fixture) = capture::mint_fixture(spec, line, report) else {
            continue; // timeout or echo-suspect: no trust artifact
        };
        artifacts.push(fixture);
        // Add the minted fixture path to the reply entry's `fixtures` array.
        let fixture_rel = capture::fixture_path(spec, report.identity.target.as_str());
        let family_file = family_file_of(db, &spec.reply_id)
            .ok_or_else(|| format!("no family file owns reply id {}", spec.reply_id))?;
        let path = repo_root.join("db").join(format!("{family_file}.toml"));
        let current = match file_edits.get(&path) {
            Some(text) => text.clone(),
            None => std::fs::read_to_string(&path)
                .map_err(|e| format!("reading {}: {e}", path.display()))?,
        };
        let edited = capture::add_fixture_to_entry(&current, &spec.reply_id, &fixture_rel)
            .ok_or_else(|| {
                format!(
                    "could not add fixture to entry {} in {}",
                    spec.reply_id,
                    path.display()
                )
            })?;
        file_edits.insert(path, edited);
    }

    for (path, contents) in file_edits {
        let rel = path
            .strip_prefix(repo_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        artifacts.push(Artifact {
            path: rel,
            contents,
        });
    }

    artifacts.push(Artifact {
        path: format!("db/results/{}.toml", report.identity.target),
        contents: capture::render_results(report, plan, meta),
    });
    Ok(artifacts)
}

/// Finds the family file stem that owns `id`.
fn family_file_of<'a>(db: &'a Database, id: &str) -> Option<&'a str> {
    db.families
        .iter()
        .find(|f| f.entries.iter().any(|e| e.id == id))
        .map(|f| f.name.as_str())
}

/// Writes one artifact, creating parent directories.
fn write_artifact(repo_root: &Path, artifact: &Artifact) -> Result<(), String> {
    let path = repo_root.join(&artifact.path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, &artifact.contents)
        .map_err(|e| format!("writing {}: {e}", path.display()))
}

/// A UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) via `date -u`, falling back to a date-only stamp.
fn utc_timestamp() -> String {
    Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

/// The answered/silent/unprobeable split from one run, for the CLI to print.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Summary {
    /// Target slug.
    pub target: String,
    /// Terminal version string.
    pub version: String,
    /// Number of queries that answered.
    pub answered: usize,
    /// Number of queries that were silent (timed out).
    pub silent: usize,
    /// Number of entries deliberately not probed.
    pub unprobeable: usize,
}

impl Summary {
    /// Builds the split from the report and the plan.
    #[must_use]
    pub fn from_report(report: &ProbeReport, plan: &ProbePlan) -> Self {
        let answered = report
            .lines
            .iter()
            .filter(|l| l.status == ProbeStatus::Answered)
            .count();
        let silent = report.lines.len() - answered;
        Summary {
            target: report.identity.target.clone(),
            version: report.identity.version.clone(),
            answered,
            silent,
            unprobeable: plan.unprobeable.len(),
        }
    }
}
