//! Validation rules for the sequence database.
//!
//! Each rule maps to a design-05 grokkability requirement: id format, unique ids, ref
//! resolution, fixture existence and header/direction agreement, replay presence, reply
//! linkage, and non-empty descriptions.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::model::Database;

/// Runs every validation rule against `db`, resolving fixture paths relative to `repo_root`.
///
/// Returns the list of human-readable violations; an empty list means the database is valid.
#[must_use]
pub fn run(db: &Database, repo_root: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    let ids: BTreeSet<&str> = db.entries().map(|e| e.id.as_str()).collect();

    check_unique_ids(db, &mut errors);
    for family in &db.families {
        for entry in &family.entries {
            check_id_format(&entry.id, &mut errors);
            check_description(entry, &mut errors);
            check_direction(entry, &mut errors);
            check_replay(entry, &mut errors);
            check_refs(db, entry, &mut errors);
            check_responds(entry, &ids, &mut errors);
            check_fixtures(entry, repo_root, &mut errors);
        }
    }
    check_results(db, repo_root, &ids, &mut errors);
    errors.sort();
    errors
}

/// Rule: ids are unique across the whole database.
fn check_unique_ids(db: &Database, errors: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    for entry in db.entries() {
        if !seen.insert(entry.id.as_str()) {
            errors.push(format!("duplicate id: {}", entry.id));
        }
    }
}

/// Rule: id is `family.mnemonic` — lowercase, dot-separated, at least two segments, each
/// segment made of `[a-z0-9_]` and non-empty.
fn check_id_format(id: &str, errors: &mut Vec<String>) {
    let ok = id.contains('.')
        && id.split('.').all(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        });
    if !ok {
        errors.push(format!(
            "malformed id (want family.mnemonic, lowercase): {id}"
        ));
    }
}

/// Rule: description is present and non-empty.
fn check_description(entry: &crate::model::Sequence, errors: &mut Vec<String>) {
    if entry.description.trim().is_empty() {
        errors.push(format!("empty description: {}", entry.id));
    }
}

/// Rule: direction is one of the three allowed values.
fn check_direction(entry: &crate::model::Sequence, errors: &mut Vec<String>) {
    match entry.direction.as_str() {
        "host-to-terminal" | "terminal-to-host" | "bidirectional" => {}
        other => errors.push(format!("invalid direction {other:?}: {}", entry.id)),
    }
}

/// Rule: replay is present and one of the three classes.
fn check_replay(entry: &crate::model::Sequence, errors: &mut Vec<String>) {
    match entry.replay.as_str() {
        "safe" | "modal" | "destructive" => {}
        other => errors.push(format!("invalid replay {other:?}: {}", entry.id)),
    }
}

/// Rule: every ref key resolves against `sources.toml`.
fn check_refs(db: &Database, entry: &crate::model::Sequence, errors: &mut Vec<String>) {
    for r in &entry.refs {
        if !db.sources.contains_key(&r.doc) {
            errors.push(format!("unresolved ref {:?}: {}", r.doc, entry.id));
        }
    }
}

/// Rule: a `responds` target is an existing entry id.
fn check_responds(entry: &crate::model::Sequence, ids: &BTreeSet<&str>, errors: &mut Vec<String>) {
    if let Some(target) = &entry.responds {
        if !ids.contains(target.as_str()) {
            errors.push(format!("responds target {target:?} missing: {}", entry.id));
        }
    }
    if let Some(target) = &entry.superseded_by {
        if !ids.contains(target.as_str()) {
            errors.push(format!(
                "superseded_by target {target:?} missing: {}",
                entry.id
            ));
        }
    }
}

/// Rule: each fixture file exists, its header `direction=` agrees with the entry, and — the
/// quarantine rule made permanent (design 05) — an `origin=capture:` reply fixture has a matching
/// capture log under `db/captures/`, so a captured report can never re-enter without its evidence.
fn check_fixtures(entry: &crate::model::Sequence, repo_root: &Path, errors: &mut Vec<String>) {
    for fx in &entry.fixtures {
        let path = repo_root.join(fx);
        let Ok(text) = fs::read_to_string(&path) else {
            errors.push(format!("missing fixture {fx}: {}", entry.id));
            continue;
        };
        let Some(header) = text.lines().next() else {
            errors.push(format!("empty fixture {fx}: {}", entry.id));
            continue;
        };
        let Some(dir) = fixture_direction(header) else {
            errors.push(format!(
                "fixture {fx} has no direction= header: {}",
                entry.id
            ));
            continue;
        };
        // Bidirectional entries may carry command-form fixtures, always host-to-terminal.
        let agrees = dir == entry.direction
            || (entry.direction == "bidirectional" && dir == "host-to-terminal");
        if !agrees {
            errors.push(format!(
                "fixture {fx} direction {dir:?} != entry direction {:?}: {}",
                entry.direction, entry.id
            ));
        }
        if let Some(origin) = fixture_origin(header) {
            check_capture_origin(entry, fx, origin, repo_root, errors);
        } else {
            errors.push(format!("fixture {fx} has no origin= header: {}", entry.id));
        }
    }
}

/// Rule: an `origin=capture:<target>-<version>` fixture is terminal-to-host and has a capture log
/// under `db/captures/<target>/`. This is R-DB-3's permanent quarantine rule: a live capture's
/// evidence (the sidecar the harness writes) must exist for its fixture to be trusted.
fn check_capture_origin(
    entry: &crate::model::Sequence,
    fx: &str,
    origin: &str,
    repo_root: &Path,
    errors: &mut Vec<String>,
) {
    let Some(rest) = origin.strip_prefix("capture:") else {
        return; // spec:/prototype: origins are validated by the direction rule alone.
    };
    if entry.direction != "terminal-to-host" {
        errors.push(format!(
            "capture-origin fixture {fx} on non-reply entry {} (direction {:?})",
            entry.id, entry.direction
        ));
    }
    // rest is `<target>-<version>`; the target is the segment before the first dash.
    let target = rest.split('-').next().unwrap_or("");
    if target.is_empty() {
        errors.push(format!(
            "fixture {fx} origin capture: has no target: {}",
            entry.id
        ));
        return;
    }
    let captures_dir = repo_root.join("db").join("captures").join(target);
    if !captures_dir.is_dir() {
        errors.push(format!(
            "capture fixture {fx} has no capture log dir db/captures/{target}/: {}",
            entry.id
        ));
    }
}

/// Extracts the `direction=` value from a fixture header line.
fn fixture_direction(header: &str) -> Option<&str> {
    header
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("direction="))
}

/// Extracts the `origin=` value from a fixture header line.
fn fixture_origin(header: &str) -> Option<&str> {
    header
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("origin="))
}

/// Rule: every `db/results/<target>.toml` seed parses, and each `[[result]]` references an existing
/// entry id with a valid status. Machines write these; validation guards them the way the entry
/// rules guard hand-written cards (design 05: entries exist, status valid).
fn check_results(db: &Database, repo_root: &Path, ids: &BTreeSet<&str>, errors: &mut Vec<String>) {
    let dir = repo_root.join("db").join("results");
    let Ok(entries) = fs::read_dir(&dir) else {
        return; // no results seeded yet is not an error.
    };
    let _ = db;
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let Ok(text) = fs::read_to_string(&path) else {
            errors.push(format!("results {name}: cannot read"));
            continue;
        };
        let value: toml::Value = match toml::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                errors.push(format!("results {name}: parse error: {e}"));
                continue;
            }
        };
        let Some(results) = value.get("result").and_then(toml::Value::as_array) else {
            errors.push(format!("results {name}: no [[result]] entries"));
            continue;
        };
        for r in results {
            let id = r.get("id").and_then(toml::Value::as_str).unwrap_or("");
            if id.is_empty() {
                errors.push(format!("results {name}: a [[result]] has no id"));
            } else if !ids.contains(id) {
                errors.push(format!("results {name}: unknown entry id {id:?}"));
            }
            let status = r.get("status").and_then(toml::Value::as_str).unwrap_or("");
            if !matches!(status, "answered" | "silent" | "timeout") {
                errors.push(format!(
                    "results {name}: invalid status {status:?} for {id}"
                ));
            }
        }
    }
}
