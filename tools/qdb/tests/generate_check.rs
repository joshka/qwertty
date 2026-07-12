//! Integration tests for `qdb generate --check reference`: it must pass against the real,
//! up-to-date `docs/reference/generated/` tree, and fail when a page is made stale, deleted, or a
//! stale extra `.md` is left behind — the freshness contract CI runs.
//!
//! Runs the actual built binary (`env!("CARGO_BIN_EXE_qdb")`, Cargo's standard integration-test
//! hook) against a scratch copy of the repo tree via `QDB_ROOT`, so the checked-in repo is never
//! touched by a failing-path test.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Copies `db/` and `docs/reference/generated/` into a scratch directory so a test can mutate the
/// generated tree without touching the real checked-in repo. `qdb generate reference` reads `db/`
/// (entries + results) and reads/writes the reference tree under `QDB_ROOT`.
fn scratch_repo() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("qdb-generate-check-{}-{n}", std::process::id()));
    let real_root = repo_root();
    copy_dir(&real_root.join("db"), &root.join("db"));
    copy_dir(
        &real_root.join("docs").join("reference").join("generated"),
        &root.join("docs").join("reference").join("generated"),
    );
    root
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &dest);
        } else {
            fs::copy(&path, &dest).unwrap();
        }
    }
}

fn generated_dir(root: &Path) -> PathBuf {
    root.join("docs").join("reference").join("generated")
}

fn qdb(root: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_qdb"))
        .args(args)
        .env("QDB_ROOT", root)
        .output()
        .expect("qdb binary runs")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

#[test]
fn check_reference_passes_against_up_to_date_tree() {
    let root = scratch_repo();
    let out = qdb(&root, &["generate", "--check", "reference"]);
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn check_reference_fails_against_deliberately_stale_page() {
    let root = scratch_repo();
    let path = generated_dir(&root).join("matrix.md");
    let mut contents = fs::read_to_string(&path).unwrap();
    contents.push_str("\nstale marker that a real render would never produce\n");
    fs::write(&path, contents).unwrap();

    let out = qdb(&root, &["generate", "--check", "reference"]);
    assert!(
        !out.status.success(),
        "expected --check to fail against a deliberately stale matrix.md"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("matrix.md"), "stderr: {stderr}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn check_reference_fails_when_page_missing() {
    let root = scratch_repo();
    fs::remove_file(generated_dir(&root).join("summary.md")).unwrap();

    let out = qdb(&root, &["generate", "--check", "reference"]);
    assert!(
        !out.status.success(),
        "expected --check to fail when a generated page is missing"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn check_reference_fails_on_stale_extra_page() {
    let root = scratch_repo();
    // A page the generator no longer emits (e.g. a removed family) must be caught, not ignored.
    fs::write(generated_dir(&root).join("ghost-family.md"), "orphan\n").unwrap();

    let out = qdb(&root, &["generate", "--check", "reference"]);
    assert!(
        !out.status.success(),
        "expected --check to fail on a stale extra page"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ghost-family.md"), "stderr: {stderr}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn generate_reference_writes_prunes_then_check_accepts() {
    let root = scratch_repo();
    // Start from a stale page and an orphan; regenerate must fix content and prune the orphan.
    fs::write(generated_dir(&root).join("matrix.md"), "stale\n").unwrap();
    fs::write(generated_dir(&root).join("ghost-family.md"), "orphan\n").unwrap();

    let write_out = qdb(&root, &["generate", "reference"]);
    assert!(
        write_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&write_out.stderr)
    );
    assert!(
        !generated_dir(&root).join("ghost-family.md").exists(),
        "regenerate should prune the orphan page"
    );

    let check_out = qdb(&root, &["generate", "--check", "reference"]);
    assert!(
        check_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&check_out.stderr)
    );
    let _ = fs::remove_dir_all(&root);
}
