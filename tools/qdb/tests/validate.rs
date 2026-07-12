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

    /// Writes a `db/results/<target>.toml` results seed into this temp repo.
    fn with_results(self, target: &str, body: &str) -> Self {
        let dir = self.root.join("db").join("results");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{target}.toml")), body).unwrap();
        self
    }

    /// Writes an extra fixture at `fixtures/test/<name>.seq` (for capture-origin tests).
    fn with_fixture(self, name: &str, body: &str) -> Self {
        let dir = self.root.join("fixtures").join("test");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{name}.seq")), body).unwrap();
        self
    }

    /// Creates an (empty) `db/captures/<target>/` log directory.
    fn with_capture_dir(self, target: &str) -> Self {
        fs::create_dir_all(self.root.join("db").join("captures").join(target)).unwrap();
        self
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

/// A reply entry whose only fixture is a capture-origin one, for the capture-log rules. The reply
/// id must exist for the query's `responds` link; both live in the same family file.
fn reply_with_capture_fixture() -> String {
    r#"[[sequence]]
id = "test.query"
name = "Test Query"
description = "Asks a thing."
direction = "host-to-terminal"
syntax = "ESC O k"
refs = [{ doc = "ecma48" }]
fixtures = ["fixtures/test/one.seq"]
replay = "safe"
responds = "test.reply"

[[sequence]]
id = "test.reply"
name = "Test Reply"
description = "Answers the thing."
direction = "terminal-to-host"
refs = [{ doc = "ecma48" }]
fixtures = ["fixtures/test/reply_capture_tmux.seq"]
replay = "safe"
"#
    .to_string()
}

const CAPTURE_FIXTURE: &str =
    "#! direction=terminal-to-host origin=capture:tmux-3.7b date=2026-07-07\n\\e[24;1R\n";

#[test]
fn capture_fixture_accepted_with_log_dir() {
    let fx = Fixture::new(&reply_with_capture_fixture(), Some(GOOD_FIXTURE))
        .with_fixture("reply_capture_tmux", CAPTURE_FIXTURE)
        .with_capture_dir("tmux");
    assert!(fx.errors().is_empty(), "{:?}", fx.errors());
}

#[test]
fn capture_fixture_rejected_without_log_dir() {
    // Same capture fixture, but no db/captures/tmux/ directory: the quarantine rule must fire.
    let fx = Fixture::new(&reply_with_capture_fixture(), Some(GOOD_FIXTURE))
        .with_fixture("reply_capture_tmux", CAPTURE_FIXTURE);
    assert!(
        fx.errors().iter().any(|e| e.contains("no capture log dir")),
        "{:?}",
        fx.errors()
    );
}

#[test]
fn fixture_without_origin_header_rejected() {
    let no_origin = "#! direction=host-to-terminal\n\\eOk\n";
    let fx = Fixture::new(&good_entry(), Some(no_origin));
    assert!(
        fx.errors().iter().any(|e| e.contains("no origin= header")),
        "{:?}",
        fx.errors()
    );
}

/// A results-file header carrying every schema-v2 metadata field, for the seed tests below.
const RESULTS_META: &str = "target = \"tmux\"\nversion = \"3.7b\"\nversion_source = \"hint\"\n\
     adapter = \"pty-headless\"\ncaptured = \"2026-07-07T00:00:00Z\"\nrunner = \"qdb 0.0.0\"\n\
     geometry = { cols = 120, rows = 40 }\n";

#[test]
fn results_seed_accepted_when_valid() {
    let body = format!(
        "{RESULTS_META}\n[[result]]\nid = \"test.one\"\nreply_id = \"test.one\"\n\
         verdict = \"supported\"\nreply_len = 6\n\n[[result]]\nid = \"test.one\"\n\
         verdict = \"skipped\"\nskipped_class = \"modal\"\nreply_len = 0\n"
    );
    let fx = Fixture::new(&good_entry(), Some(GOOD_FIXTURE)).with_results("tmux", &body);
    assert!(fx.errors().is_empty(), "{:?}", fx.errors());
}

#[test]
fn results_seed_rejects_missing_metadata() {
    // No adapter/version_source/runner/geometry: schema v2 requires the whole run-hosting block
    // for the attended-cell honesty rule, and validate is the only guard (serde defaults keep a
    // partial file loadable).
    let body = "target = \"tmux\"\nversion = \"3.7b\"\ncaptured = \"2026-07-07T00:00:00Z\"\n\
                \n[[result]]\nid = \"test.one\"\nverdict = \"supported\"\nreply_len = 6\n";
    let fx = Fixture::new(&good_entry(), Some(GOOD_FIXTURE)).with_results("tmux", body);
    let errs = fx.errors();
    for field in ["adapter", "version_source", "runner", "geometry"] {
        assert!(
            errs.iter().any(|e| e.contains(field)),
            "expected a {field} error in {errs:?}"
        );
    }
}

#[test]
fn results_seed_rejects_bad_version_source_and_geometry() {
    let body = "target = \"tmux\"\nversion = \"3.7b\"\nversion_source = \"guess\"\n\
                adapter = \"pty-headless\"\ncaptured = \"2026-07-07T00:00:00Z\"\nrunner = \"qdb 0.0.0\"\n\
                geometry = { cols = \"wide\", rows = 40 }\n\
                \n[[result]]\nid = \"test.one\"\nverdict = \"supported\"\nreply_len = 6\n";
    let fx = Fixture::new(&good_entry(), Some(GOOD_FIXTURE)).with_results("tmux", body);
    let errs = fx.errors();
    assert!(
        errs.iter().any(|e| e.contains("invalid version_source")),
        "{errs:?}"
    );
    assert!(
        errs.iter().any(|e| e.contains("geometry must be")),
        "{errs:?}"
    );
}

#[test]
fn results_seed_rejects_unknown_entry_id() {
    let body = format!(
        "{RESULTS_META}\n[[result]]\nid = \"test.ghost\"\nverdict = \"supported\"\nreply_len = 0\n"
    );
    let fx = Fixture::new(&good_entry(), Some(GOOD_FIXTURE)).with_results("tmux", &body);
    assert!(
        fx.errors().iter().any(|e| e.contains("unknown entry id")),
        "{:?}",
        fx.errors()
    );
}

#[test]
fn results_seed_rejects_invalid_verdict() {
    let body = format!(
        "{RESULTS_META}\n[[result]]\nid = \"test.one\"\nverdict = \"maybe\"\nreply_len = 0\n"
    );
    let fx = Fixture::new(&good_entry(), Some(GOOD_FIXTURE)).with_results("tmux", &body);
    assert!(
        fx.errors().iter().any(|e| e.contains("invalid verdict")),
        "{:?}",
        fx.errors()
    );
}

#[test]
fn results_seed_rejects_skipped_without_class() {
    let body = format!(
        "{RESULTS_META}\n[[result]]\nid = \"test.one\"\nverdict = \"skipped\"\nreply_len = 0\n"
    );
    let fx = Fixture::new(&good_entry(), Some(GOOD_FIXTURE)).with_results("tmux", &body);
    assert!(
        fx.errors().iter().any(|e| e.contains("no skipped_class")),
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
