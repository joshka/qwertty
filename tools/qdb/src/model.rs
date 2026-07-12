//! Data model for the sequence database: entries, sources, and loading.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One control-sequence entry, mirroring the design-05 schema.
#[derive(Debug, Deserialize)]
pub struct Sequence {
    /// Stable namespaced identifier, `family.mnemonic`.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// One plain-English sentence describing what the sequence does.
    pub description: String,
    /// `host-to-terminal`, `terminal-to-host`, or `bidirectional`.
    pub direction: String,
    /// Canonical ECMA-48-notation syntax. Omitted for quarantined reply syntax.
    #[serde(default)]
    pub syntax: Option<String>,
    /// Parameter descriptions, where meaningful.
    #[serde(default)]
    pub params: Vec<Param>,
    /// Citations, resolved against `sources.toml`.
    #[serde(default)]
    pub refs: Vec<Ref>,
    /// Fixture files, relative to the repo root.
    #[serde(default)]
    pub fixtures: Vec<String>,
    /// Replay safety class: `safe`, `modal`, or `destructive`.
    pub replay: String,
    /// Id of the reply sequence, if this entry is a query.
    #[serde(default)]
    pub responds: Option<String>,
    /// Free-form notes.
    #[serde(default)]
    pub notes: Option<String>,
    /// Id of the entry that supersedes this one, if deprecated.
    #[serde(default)]
    pub superseded_by: Option<String>,
}

/// A single parameter of a sequence.
#[derive(Debug, Deserialize)]
pub struct Param {
    /// Parameter name as it appears in the syntax.
    pub name: String,
    /// Parameter kind, e.g. `number`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Default value applied when the parameter is omitted.
    #[serde(default)]
    pub default: Option<toml::Value>,
}

/// A citation reference, keyed into `sources.toml`.
#[derive(Debug, Deserialize)]
pub struct Ref {
    /// Source key resolving against `sources.toml`.
    pub doc: String,
    /// Optional section identifier within the source.
    #[serde(default)]
    pub section: Option<String>,
    /// Optional anchor within the source.
    #[serde(default)]
    pub anchor: Option<String>,
}

/// The top-level table of a family file: `[[sequence]]` entries.
#[derive(Debug, Deserialize)]
struct FamilyFile {
    #[serde(default)]
    sequence: Vec<Sequence>,
}

/// A full citation from `sources.toml`.
#[derive(Debug, Deserialize)]
pub struct Source {
    /// Human-readable title.
    pub title: String,
    /// Canonical URL.
    pub url: String,
    /// Date the source was retrieved and verified.
    pub retrieved: String,
}

/// A loaded family: its db-family name (file stem) and its entries.
pub struct Family {
    /// The family file stem, e.g. `ecma48-csi`.
    pub name: String,
    /// The entries in this family, in file order.
    pub entries: Vec<Sequence>,
}

/// One row of a `db/results/<target>.toml` conformance seed (schema v2): one entry's support
/// verdict for this target.
#[derive(Debug, Clone, Deserialize)]
pub struct ResultRow {
    /// The query entry id.
    pub id: String,
    /// The reply entry id (`responds` target) the row's verdict belongs to.
    #[serde(default)]
    pub reply_id: String,
    /// The support verdict: `supported`, `unsupported`, `no-reply`, `unprobeable`, or `skipped`
    /// (with `skipped_class` set). See `db/README.md`, "Results schema".
    pub verdict: String,
    /// Number of raw reply bytes captured; `0` when nothing genuine arrived.
    #[serde(default)]
    pub reply_len: usize,
    /// The replay class that caused a `skipped` verdict (`modal`/`destructive`); absent for every
    /// other verdict.
    #[serde(default)]
    pub skipped_class: Option<String>,
}

/// Session geometry a results run captured under.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Geometry {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// One `db/results/<target>.toml` file (schema v2): a target's run metadata plus its per-entry
/// verdicts.
#[derive(Debug, Clone, Deserialize)]
pub struct ResultsFile {
    /// The target's short name, e.g. `tmux` or `betamax`.
    pub target: String,
    /// The target's version string, as captured (e.g. `tmux 3.7b`, `libghostty`).
    pub version: String,
    /// How `version` was obtained: `xtversion` (the terminal named itself — authoritative),
    /// `hint` (the adapter's out-of-band probe, e.g. `tmux -V`), or `none`.
    #[serde(default)]
    pub version_source: String,
    /// How the target is hosted: `in-process`, `pty-headless`, or `attended`. The attended-cell
    /// honesty rule keys off this (design `conformance-target-interface.md`).
    #[serde(default)]
    pub adapter: String,
    /// UTC timestamp the run ran, RFC-3339-ish (`qdb`'s stamp).
    pub captured: String,
    /// The runner build that produced this file, e.g. `qdb 0.0.0`.
    #[serde(default)]
    pub runner: String,
    /// Session geometry the run used.
    #[serde(default = "default_geometry")]
    pub geometry: Geometry,
    /// One row per entry (probed, unprobeable, or skipped — never omitted).
    #[serde(default, rename = "result")]
    pub results: Vec<ResultRow>,
}

/// The geometry assumed for a results file that predates the field (none ship without it, but
/// deserialization needs a fallback rather than a hard failure the loader can't localize).
fn default_geometry() -> Geometry {
    Geometry { cols: 0, rows: 0 }
}

/// The whole database: every family plus the shared source table.
pub struct Database {
    /// Families sorted by name.
    pub families: Vec<Family>,
    /// Source keys to citations.
    pub sources: BTreeMap<String, Source>,
}

impl Database {
    /// Loads every `db/<family>.toml` file plus `db/sources.toml` from `dir`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be read, or if any file fails to parse.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let sources_path = dir.join("sources.toml");
        let sources_text = fs::read_to_string(&sources_path)
            .map_err(|e| format!("reading {}: {e}", sources_path.display()))?;
        let sources: BTreeMap<String, Source> = toml::from_str(&sources_text)
            .map_err(|e| format!("parsing {}: {e}", sources_path.display()))?;

        let mut paths: Vec<PathBuf> = fs::read_dir(dir)
            .map_err(|e| format!("reading {}: {e}", dir.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "toml")
                    && p.file_name().is_some_and(|n| n != "sources.toml")
            })
            .collect();
        paths.sort();

        let mut families = Vec::new();
        for path in paths {
            let text = fs::read_to_string(&path)
                .map_err(|e| format!("reading {}: {e}", path.display()))?;
            let parsed: FamilyFile =
                toml::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))?;
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            families.push(Family {
                name,
                entries: parsed.sequence,
            });
        }
        Ok(Database { families, sources })
    }

    /// Iterates over every entry across all families.
    pub fn entries(&self) -> impl Iterator<Item = &Sequence> {
        self.families.iter().flat_map(|f| f.entries.iter())
    }

    /// Loads every `db/results/<target>.toml` conformance seed, sorted by target name.
    ///
    /// Returns an empty vec if `db/results/` does not exist (no captures run yet is not an
    /// error — mirrors `qdb validate`'s `check_results`).
    ///
    /// # Errors
    ///
    /// Returns an error if a results file exists but cannot be read or parsed.
    pub fn load_results(repo_root: &Path) -> Result<Vec<ResultsFile>, String> {
        let dir = repo_root.join("db").join("results");
        let Ok(entries) = fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        paths.sort();

        let mut files = Vec::new();
        for path in paths {
            let text = fs::read_to_string(&path)
                .map_err(|e| format!("reading {}: {e}", path.display()))?;
            let parsed: ResultsFile =
                toml::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))?;
            files.push(parsed);
        }
        files.sort_by(|a, b| a.target.cmp(&b.target));
        Ok(files)
    }
}
