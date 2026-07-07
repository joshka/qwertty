//! The capture orchestrator: `qdb capture --target tmux|betamax`.
//!
//! The outer half of the harness. It spawns the target terminal, runs `qdb capture-probe` *inside*
//! it (tmux `send-keys`, or a betamax tape generated on the fly), reads back the probe's JSON
//! report, and mints every artifact: per-entry sidecars, `origin=capture:` reply fixtures, the
//! `db/results/<target>.toml` seed, and the scripted fixture-array edits on the report entries.
//!
//! The probe does the live terminal I/O (`probe.rs`); this module owns process spawning and the
//! filesystem writes. Both sit behind `qdb capture` / `qdb capture-probe` in `main.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::capture::{self, Artifact, ProbePlan, ProbeReport, ProbeStatus};
use crate::model::Database;

/// Which real terminal the orchestrator drives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    /// A tmux pane driven with `send-keys` / `capture-pane`.
    Tmux,
    /// A betamax-hosted headless ghostty-vt, driven with an on-the-fly tape.
    Betamax,
}

impl Target {
    /// Parses the `--target` value.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tmux" => Some(Self::Tmux),
            "betamax" => Some(Self::Betamax),
            _ => None,
        }
    }

    /// The target slug used in artifact paths and origin headers.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Betamax => "betamax",
        }
    }
}

/// Runs a full capture pass for one target and writes every artifact under `repo_root`.
///
/// # Errors
///
/// Returns an error if the target tool is missing, the in-terminal probe cannot be run, its report
/// cannot be read, or an artifact cannot be written.
pub fn run(
    db: &Database,
    repo_root: &Path,
    target: Target,
    only: &[String],
) -> Result<Summary, String> {
    let timestamp = utc_timestamp();
    let out_path = std::env::temp_dir().join(format!("qdb-capture-{}.json", target.slug()));
    let _ = std::fs::remove_file(&out_path);

    match target {
        Target::Tmux => run_tmux(repo_root, only, &timestamp, &out_path)?,
        Target::Betamax => run_betamax(repo_root, only, &timestamp, &out_path)?,
    }

    let json = std::fs::read_to_string(&out_path).map_err(|e| {
        format!(
            "reading probe report {}: {e} (did the probe run?)",
            out_path.display()
        )
    })?;
    let report: ProbeReport =
        serde_json::from_str(json.trim()).map_err(|e| format!("parsing probe report: {e}"))?;

    let plan = ProbePlan::build(db, only);
    let artifacts = mint_all(db, repo_root, &plan, &report)?;
    for artifact in &artifacts {
        write_artifact(repo_root, artifact)?;
    }
    Ok(Summary::from_report(&report, &plan))
}

/// Mints every artifact for a run: sidecars, answered-reply fixtures, the fixture-array edits on
/// the report entries, and the results seed. Pure given the report — the live-terminal split is
/// upstream — so the whole minting pass is unit-tested in `capture.rs` and integration-checked
/// here.
fn mint_all(
    db: &Database,
    repo_root: &Path,
    plan: &ProbePlan,
    report: &ProbeReport,
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
        if line.status != ProbeStatus::Answered {
            continue;
        }
        let Some(spec) = specs.get(line.query_id.as_str()) else {
            continue;
        };
        if let Some(fixture) = capture::mint_fixture(spec, line, report) {
            artifacts.push(fixture);
        }
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
        contents: capture::render_results(report),
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

/// Runs the probe inside a fresh tmux pane. Reuses `scripts/verify_emulators.sh`'s pattern:
/// detached session, `send-keys` the command, wait, then tear down. The probe writes JSON to
/// `out_path`, which is on the same filesystem tmux's shell sees.
fn run_tmux(
    repo_root: &Path,
    only: &[String],
    timestamp: &str,
    out_path: &Path,
) -> Result<(), String> {
    require_tool("tmux")?;
    // `tmux -V` prints "tmux 3.7b" — a fallback identity if the pane answers no XTVERSION. The
    // redundant `tmux ` prefix is deduped when the origin slug is built (`capture_origin`).
    let version = tool_version(&["tmux", "-V"]);
    let bin = build_probe(repo_root)?;
    let session = "qdb-capture";
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", session])
        .output();
    run_ok(Command::new("tmux").args([
        "new-session",
        "-d",
        "-s",
        session,
        "-x",
        "120",
        "-y",
        "40",
    ]))?;

    let script = write_probe_script(&bin, repo_root, only, "tmux", &version, timestamp, out_path)?;
    // Run the script by path: send-keys word-splits, so a single unquoted path is robust where an
    // inline multi-arg command is not (the `tail: unrecognized option --out` failure mode).
    run_ok(Command::new("tmux").args([
        "send-keys",
        "-t",
        session,
        &format!("bash {}", shell_quote(&script.to_string_lossy())),
        "Enter",
    ]))?;

    // Poll for the report file to appear, up to a generous ceiling.
    wait_for_file(out_path, std::time::Duration::from_secs(60));
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", session])
        .output();
    Ok(())
}

/// Runs the probe inside a betamax-hosted ghostty-vt via an on-the-fly tape. betamax's shell starts
/// in `$HOME` (a known gotcha), so the generated script `cd`s to the repo first; children have a
/// controlling terminal, so the probe opens `/dev/tty` normally.
fn run_betamax(
    repo_root: &Path,
    only: &[String],
    timestamp: &str,
    out_path: &Path,
) -> Result<(), String> {
    require_tool("betamax")?;
    let version = tool_version(&["betamax", "--version"]);
    let bin = build_probe(repo_root)?;
    let script = write_probe_script(
        &bin, repo_root, only, "betamax", &version, timestamp, out_path,
    )?;

    // The tape types one word (the script path) and waits for its completion marker — no fragile
    // multi-arg Type line to word-split.
    let tape = format!(
        "Output {out}.gif\nSet Shell \"bash\"\nSet Width 1000\nSet Height 700\n\
         Type \"bash {script}\"\nEnter\nWait+Screen@60s \"QDB_PROBE_DONE\"\n",
        out = out_path.display(),
        script = script.display(),
    );
    let tape_path = std::env::temp_dir().join("qdb-capture-betamax.tape");
    std::fs::write(&tape_path, &tape)
        .map_err(|e| format!("writing tape {}: {e}", tape_path.display()))?;
    run_ok(Command::new("betamax").arg("run").arg(&tape_path))?;
    wait_for_file(out_path, std::time::Duration::from_secs(5));
    Ok(())
}

/// Writes a small shell script that runs `qdb capture-probe` inside the target and echoes a
/// completion marker the driver waits on. Returns the script path. Writing a script rather than an
/// inline command keeps quoting out of tmux's `send-keys` and betamax's `Type`, both of which
/// word-split their argument.
fn write_probe_script(
    bin: &Path,
    repo_root: &Path,
    only: &[String],
    target: &str,
    version: &str,
    timestamp: &str,
    out_path: &Path,
) -> Result<PathBuf, String> {
    let mut cmd = format!(
        "cd {root} && QDB_ROOT={root} {bin} capture-probe --target {target} \
         --version {version} --timestamp {timestamp} --out {out}",
        root = shell_quote(&repo_root.to_string_lossy()),
        bin = shell_quote(&bin.to_string_lossy()),
        version = shell_quote(version),
        timestamp = shell_quote(timestamp),
        out = shell_quote(&out_path.to_string_lossy()),
    );
    for id in only {
        cmd.push_str(" --entry ");
        cmd.push_str(&shell_quote(id));
    }
    let body = format!("#!/bin/bash\n{cmd}\necho QDB_PROBE_DONE\n");
    let path = std::env::temp_dir().join(format!("qdb-probe-{target}.sh"));
    std::fs::write(&path, body).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(path)
}

/// Single-quotes a string for the shell, escaping embedded single quotes.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Builds the `qdb` binary (release-free debug is fine) and returns its path, so the in-terminal
/// probe runs the just-built code rather than whatever is on `$PATH`.
fn build_probe(repo_root: &Path) -> Result<PathBuf, String> {
    run_ok(
        Command::new("cargo")
            .current_dir(repo_root)
            .args(["build", "--quiet", "-p", "qdb", "--bin", "qdb"]),
    )?;
    Ok(repo_root.join("target/debug/qdb"))
}

/// Runs a command and errors on non-zero exit, surfacing stderr.
fn run_ok(cmd: &mut Command) -> Result<(), String> {
    let output = cmd.output().map_err(|e| format!("spawning {cmd:?}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// Returns the trimmed stdout of a version command, or empty on failure.
fn tool_version(argv: &[&str]) -> String {
    Command::new(argv[0])
        .args(&argv[1..])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Errors if `tool` is not on `$PATH`.
fn require_tool(tool: &str) -> Result<(), String> {
    which(tool)
        .map(|_| ())
        .ok_or_else(|| format!("{tool} is not installed"))
}

/// Resolves a tool on `$PATH`.
fn which(tool: &str) -> Option<PathBuf> {
    Command::new("sh")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .filter(|p| !p.as_os_str().is_empty())
}

/// Waits up to `timeout` for `path` to exist, polling.
///
/// This is a live-terminal driver, not the sans-io core, so it owns a real wall-clock deadline —
/// exactly the case `clippy.toml` carves out for an explicit `#[allow]` at the call site.
fn wait_for_file(path: &Path, timeout: std::time::Duration) {
    #[allow(clippy::disallowed_methods)]
    let deadline = std::time::Instant::now() + timeout;
    #[allow(clippy::disallowed_methods)]
    while std::time::Instant::now() < deadline {
        if path.exists() {
            // Give the writer a beat to finish flushing.
            std::thread::sleep(std::time::Duration::from_millis(200));
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
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
