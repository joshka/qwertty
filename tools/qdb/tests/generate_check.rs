//! Integration tests for `qdb generate --check matrix`: it must pass against the real, up-to-date
//! `db/caniuse.md`, and fail when the checked-in file is deliberately made stale — the same
//! drift-detection contract `qdb generate --check docs` already has for the doc pages.
//!
//! Runs the actual built binary (`env!("CARGO_BIN_EXE_qdb")`, Cargo's standard integration-test
//! hook) against a scratch copy of the repo tree, via `QDB_ROOT`, so the checked-in repo is never
//! touched by a failing-path test.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

/// Copies `db/`, `fixtures/`, and `docs/` into a scratch directory so a test can mutate
/// `db/caniuse.md` without touching the real checked-in repo. `qdb`'s `repo_root()` only reads
/// `db/` (via `QDB_ROOT`) for these commands, but the results loader and matrix renderer only touch
/// `db/`, so copying `db/` alone is sufficient.
fn scratch_repo() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("qdb-generate-check-{}-{n}", std::process::id()));
    let real_root = repo_root();
    copy_dir(&real_root.join("db"), &root.join("db"));
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

fn qdb(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_qdb"))
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
fn check_matrix_passes_against_up_to_date_file() {
    let root = scratch_repo();
    let out = qdb(&root, &["generate", "--check", "matrix"]);
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn check_matrix_fails_against_deliberately_stale_file() {
    let root = scratch_repo();
    // Corrupt the checked-in matrix so it no longer matches a fresh render.
    let path = root.join("db").join("caniuse.md");
    let mut contents = fs::read_to_string(&path).unwrap();
    contents.push_str("\nstale marker that a real render would never produce\n");
    fs::write(&path, contents).unwrap();

    let out = qdb(&root, &["generate", "--check", "matrix"]);
    assert!(
        !out.status.success(),
        "expected --check to fail against a deliberately stale caniuse.md"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("drift in db/caniuse.md"),
        "stderr: {stderr}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn check_matrix_fails_when_file_missing() {
    let root = scratch_repo();
    fs::remove_file(root.join("db").join("caniuse.md")).unwrap();

    let out = qdb(&root, &["generate", "--check", "matrix"]);
    assert!(
        !out.status.success(),
        "expected --check to fail when db/caniuse.md does not exist"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn generate_matrix_writes_a_file_check_then_accepts() {
    let root = scratch_repo();
    // Start from a stale file, regenerate, then --check must pass.
    let path = root.join("db").join("caniuse.md");
    fs::write(&path, "stale\n").unwrap();

    let write_out = qdb(&root, &["generate", "matrix"]);
    assert!(write_out.status.success());

    let check_out = qdb(&root, &["generate", "--check", "matrix"]);
    assert!(
        check_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&check_out.stderr)
    );
    let _ = fs::remove_dir_all(&root);
}
