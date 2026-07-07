//! The live-capture harness core (design 05, "The live-capture harness").
//!
//! `qdb capture` drives one target terminal with the query entries, records raw reply bytes and an
//! identity probe, and mints report-direction fixtures with `origin=capture:` headers — the
//! quarantine replacement — plus a conformance results seed, all in one pass. This module is the
//! pure core: it decides *what* to probe (from the database), models the probe's JSON output, and
//! mints every artifact from that output. The live terminal I/O lives in the binary (`probe.rs`);
//! everything here is exercised by unit tests feeding canned probe output, no terminal needed.
//!
//! The runner is the first partial consumer of the conformance Target interface
//! (`conformance-target-interface.md`): a probe *feeds* query bytes and *drains* reply bytes with a
//! deadline. It does not yet grow the full `Target` trait — this is the `feed`/`drain_output` core
//! the OQ-1 one-shot API later packages.

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
}

/// An entry that carries a `responds` link but is deliberately not probed, with the reason — the
/// deliverable's "skip honestly" rule. Silence is data; *unprobeable* is a different datum: the
/// probe was never sent because sending it is unsafe or ill-defined.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unprobeable {
    /// The query entry id.
    pub query_id: String,
    /// Why the entry is not probed.
    pub reason: String,
}

/// The runner's probe plan: what will be sent, and what is skipped with a reason.
#[derive(Clone, Debug, Default)]
pub struct ProbePlan {
    /// Entries that will be probed.
    pub specs: Vec<ProbeSpec>,
    /// Entries deliberately not probed, with reasons.
    pub unprobeable: Vec<Unprobeable>,
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
    /// link and `replay = "safe"` (queries only — never `modal`/`destructive`), minus the
    /// honestly-unprobeable set, optionally filtered to `only` entry ids.
    ///
    /// Query bytes come from the entry's own query fixture: it already holds complete, sendable
    /// bytes (see the harvested fixtures), so unescaping it is the single source of truth and no
    /// second byte-construction opinion can drift from the recorded command form.
    #[must_use]
    pub fn build(db: &Database, only: &[String]) -> Self {
        let reasons = unprobeable_reasons();
        let mut plan = ProbePlan::default();

        for entry in db.entries() {
            let Some(reply_id) = &entry.responds else {
                continue;
            };
            if !only.is_empty() && !only.iter().any(|id| id == &entry.id) {
                continue;
            }
            // Safety gate: only queries. Modal/destructive replay classes never reach a live
            // terminal blind (the DECSLPP incident rule); modal DECRQM mode queries are excluded
            // here, not marked unprobeable, because the reason is the replay class, not the entry.
            if entry.replay != "safe" {
                continue;
            }
            if let Some(reason) = reasons.get(entry.id.as_str()) {
                plan.unprobeable.push(Unprobeable {
                    query_id: entry.id.clone(),
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
                }),
                Err(reason) => plan.unprobeable.push(Unprobeable {
                    query_id: entry.id.clone(),
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

/// One JSON line the probe emits per query: the entry id, the escaped raw reply bytes, and the
/// status. Timestamp and identity are recorded once per run in [`ProbeReport`], not per line.
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
}

/// The terminal's identity as probed: DA1 and XTVERSION replies (raw, escaped), plus any
/// out-of-band version string the orchestrator supplied (e.g. `tmux -V`).
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
            let value = serde_json::json!({
                "query_id": line.query_id,
                "reply_id": line.reply_id,
                "target": target,
                "identity": report.identity,
                "timestamp": report.timestamp,
                "status": line.status,
                "reply_len": line.reply_len,
                "reply_escaped": line.reply_escaped,
            });
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
#[must_use]
pub fn mint_fixture(spec: &ProbeSpec, line: &ProbeLine, report: &ProbeReport) -> Option<Artifact> {
    if line.status != ProbeStatus::Answered {
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

/// Renders the conformance results seed for one target: `db/results/<target>.toml`.
///
/// One `[[result]]` per probed entry with `status = answered|silent|timeout` and the reply length —
/// the first caniuse datum. "Silent" and "timeout" are the same wire event (no bytes before the
/// deadline); we record `timeout` uniformly and note the deadline, letting the renderer read it as
/// "this target does not answer this query". `qdb validate` checks entries exist and status is
/// valid.
#[must_use]
pub fn render_results(report: &ProbeReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Conformance results for {} — generated by `qdb capture`, do not hand-edit.",
        report.identity.target
    );
    let _ = writeln!(
        out,
        "# Machines write support claims; humans write entries and citations (design 05)."
    );
    let _ = writeln!(out, "target = {:?}", report.identity.target);
    let _ = writeln!(out, "version = {:?}", report.identity.version);
    let _ = writeln!(out, "captured = {:?}", report.timestamp);
    for line in &report.lines {
        let status = match line.status {
            ProbeStatus::Answered => "answered",
            ProbeStatus::Timeout => "timeout",
        };
        let _ = writeln!(out);
        let _ = writeln!(out, "[[result]]");
        let _ = writeln!(out, "id = {:?}", line.query_id);
        let _ = writeln!(out, "reply_id = {:?}", line.reply_id);
        let _ = writeln!(out, "status = {status:?}");
        let _ = writeln!(out, "reply_len = {}", line.reply_len);
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
        }
    }

    fn timed_out(query: &str, reply: &str) -> ProbeLine {
        ProbeLine {
            query_id: query.to_string(),
            reply_id: reply.to_string(),
            reply_escaped: String::new(),
            reply_len: 0,
            status: ProbeStatus::Timeout,
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
    }

    #[test]
    fn fixture_path_matches_minted_path() {
        let spec = ProbeSpec {
            query_id: "osc.11.background_query".to_string(),
            reply_id: "osc.11.background_report".to_string(),
            query_bytes: vec![],
            family_dir: "osc".to_string(),
            source: ProbeSource::FixtureBytes,
        };
        assert_eq!(
            fixture_path(&spec, "betamax"),
            "fixtures/osc/osc_11_background_report_report_capture_betamax.seq"
        );
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

    #[test]
    fn results_lists_every_probed_entry() {
        let r = report(
            "betamax",
            vec![
                answered("csi.da.primary", "csi.da.primary_report", "\\e[?62;c"),
                timed_out("osc.52.clipboard_query", "osc.52.clipboard_report"),
            ],
        );
        let toml = render_results(&r);
        assert!(toml.contains("target = \"betamax\""));
        assert!(toml.contains("id = \"csi.da.primary\""));
        assert!(toml.contains("status = \"answered\""));
        assert!(toml.contains("id = \"osc.52.clipboard_query\""));
        assert!(toml.contains("status = \"timeout\""));
        // Parseable and shaped as we expect.
        let parsed: toml::Value = toml::from_str(&toml).unwrap();
        assert_eq!(parsed["result"].as_array().unwrap().len(), 2);
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
