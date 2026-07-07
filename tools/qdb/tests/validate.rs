//! Unit tests for each `qdb validate` rule.
//!
//! Each test builds a tiny in-memory database in a temp directory and asserts that the rule
//! under test fires (or does not). The database is loaded through the real `Database::load`
//! path so parsing and validation are exercised together.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use qdb::model::Database;
use qdb::validate;

/// A throwaway directory holding a `db/` tree and one fixture, cleaned up on drop.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// Builds a temp repo root with `db/sources.toml`, one family file, and an optional
    /// host-to-terminal fixture at `fixtures/test/one.seq`.
    fn new(family_toml: &str, fixture: Option<&str>) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("qdb-test-{}-{n}", std::process::id()));
        fs::create_dir_all(root.join("db")).unwrap();
        fs::write(
            root.join("db").join("sources.toml"),
            "[ecma48]\ntitle = \"ECMA-48\"\nurl = \"https://example.test/\"\nretrieved = \"2026-07-06\"\n",
        )
        .unwrap();
        fs::write(root.join("db").join("test.toml"), family_toml).unwrap();
        if let Some(body) = fixture {
            fs::create_dir_all(root.join("fixtures").join("test")).unwrap();
            fs::write(root.join("fixtures").join("test").join("one.seq"), body).unwrap();
        }
        Fixture { root }
    }

    fn errors(&self) -> Vec<String> {
        let db = Database::load(&self.root.join("db")).unwrap();
        validate::run(&db, &self.root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const GOOD_FIXTURE: &str = "#! direction=host-to-terminal origin=spec:ecma48\n\\eOk\n";

/// A minimal well-formed entry that references the temp fixture.
fn good_entry() -> String {
    r#"[[sequence]]
id = "test.one"
name = "Test One"
description = "Does a thing."
direction = "host-to-terminal"
syntax = "ESC O k"
refs = [{ doc = "ecma48" }]
fixtures = ["fixtures/test/one.seq"]
replay = "safe"
"#
    .to_string()
}

#[test]
fn accepts_a_well_formed_entry() {
    let fx = Fixture::new(&good_entry(), Some(GOOD_FIXTURE));
    assert!(fx.errors().is_empty(), "{:?}", fx.errors());
}

#[test]
fn rejects_unresolved_ref() {
    let toml = good_entry().replace("doc = \"ecma48\"", "doc = \"nope\"");
    let fx = Fixture::new(&toml, Some(GOOD_FIXTURE));
    assert!(
        fx.errors().iter().any(|e| e.contains("unresolved ref")),
        "{:?}",
        fx.errors()
    );
}

#[test]
fn rejects_duplicate_id() {
    let toml = format!("{}\n{}", good_entry(), good_entry());
    let fx = Fixture::new(&toml, Some(GOOD_FIXTURE));
    assert!(
        fx.errors().iter().any(|e| e.contains("duplicate id")),
        "{:?}",
        fx.errors()
    );
}

#[test]
fn rejects_missing_fixture() {
    // Entry references the fixture but the file is not written.
    let fx = Fixture::new(&good_entry(), None);
    assert!(
        fx.errors().iter().any(|e| e.contains("missing fixture")),
        "{:?}",
        fx.errors()
    );
}

#[test]
fn rejects_direction_mismatch() {
    // Fixture header says terminal-to-host; entry says host-to-terminal.
    let bad = "#! direction=terminal-to-host origin=spec:ecma48\n\\eOk\n";
    let fx = Fixture::new(&good_entry(), Some(bad));
    assert!(
        fx.errors()
            .iter()
            .any(|e| e.contains("direction") && e.contains("!=")),
        "{:?}",
        fx.errors()
    );
}

#[test]
fn rejects_malformed_id() {
    let toml = good_entry().replace("id = \"test.one\"", "id = \"Test.One\"");
    let fx = Fixture::new(&toml, Some(GOOD_FIXTURE));
    assert!(
        fx.errors().iter().any(|e| e.contains("malformed id")),
        "{:?}",
        fx.errors()
    );
}

#[test]
fn rejects_empty_description() {
    let toml = good_entry().replace("Does a thing.", "");
    let fx = Fixture::new(&toml, Some(GOOD_FIXTURE));
    assert!(
        fx.errors().iter().any(|e| e.contains("empty description")),
        "{:?}",
        fx.errors()
    );
}

#[test]
fn rejects_bad_replay_class() {
    let toml = good_entry().replace("replay = \"safe\"", "replay = \"maybe\"");
    let fx = Fixture::new(&toml, Some(GOOD_FIXTURE));
    assert!(
        fx.errors().iter().any(|e| e.contains("invalid replay")),
        "{:?}",
        fx.errors()
    );
}

#[test]
fn rejects_bad_direction_value() {
    let toml = good_entry().replace("host-to-terminal", "sideways");
    // Fixture direction now also mismatches, but the direction-value rule must fire.
    let fx = Fixture::new(&toml, Some(GOOD_FIXTURE));
    assert!(
        fx.errors().iter().any(|e| e.contains("invalid direction")),
        "{:?}",
        fx.errors()
    );
}

#[test]
fn rejects_dangling_responds_target() {
    let toml = good_entry().replace(
        "replay = \"safe\"\n",
        "replay = \"safe\"\nresponds = \"test.ghost\"\n",
    );
    let fx = Fixture::new(&toml, Some(GOOD_FIXTURE));
    assert!(
        fx.errors()
            .iter()
            .any(|e| e.contains("responds target") && e.contains("missing")),
        "{:?}",
        fx.errors()
    );
}

/// The real database in the repo must validate clean.
#[test]
fn real_database_validates() {
    let root = repo_root();
    let db = Database::load(&root.join("db")).unwrap();
    let errors = validate::run(&db, &root);
    assert!(errors.is_empty(), "{errors:?}");
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}
