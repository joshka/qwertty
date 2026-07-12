//! `qdb` — developer tooling for the qwertty sequence database.
//!
//! Commands:
//! - `validate` — check the database against every design-05 grokkability rule.
//! - `generate [--check] [docs|matrix]` — with no positional target, generates both; `docs` writes
//!   the ephemeral markdown reference to `target/qdb-docs/`; `matrix` writes the checked-in caniuse
//!   support table to `db/caniuse.md`. `--check` regenerates to a buffer and fails on drift from
//!   the committed/existing output instead of writing.
//! - `capture --target <name>` — drive a real terminal through the conformance runner with
//!   recording on: mint sidecars, `origin=capture:` fixtures, and the results seed.
//! - `run --target <name>` — the conformance pass: same loop, results seed only. `--allow-modal` /
//!   `--allow-destructive` opt replay classes in; they are never probed blind.
//! - `target-relay` — internal: the in-terminal byte relay the PTY-hosted adapters launch.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{env, fs};

use qdb::model::Database;
use qdb::{generate, validate};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let repo_root = repo_root();
    let db_dir = repo_root.join("db");

    match args.first().map(String::as_str) {
        Some("validate") => cmd_validate(&db_dir, &repo_root),
        Some("generate") => cmd_generate(&db_dir, &repo_root, &args[1..]),
        #[cfg(unix)]
        Some("capture") => cmd_capture(&db_dir, &repo_root, &args[1..]),
        #[cfg(unix)]
        Some("run") => cmd_run(&db_dir, &repo_root, &args[1..]),
        #[cfg(unix)]
        Some("target-relay") => cmd_target_relay(&args[1..]),
        _ => {
            eprintln!(
                "usage: qdb <validate | generate [--check] [docs|matrix] | \
                 capture --target tmux|betamax [--entry <id>...] | \
                 run --target tmux|betamax [--entry <id>...] [--allow-modal] \
                 [--allow-destructive]>"
            );
            ExitCode::FAILURE
        }
    }
}

/// Shared `--target`/`--entry` argument parsing for the capture and run commands.
#[cfg(unix)]
struct DriveArgs {
    target: qdb::orchestrate::TargetKind,
    only: Vec<String>,
    allow_modal: bool,
    allow_destructive: bool,
}

/// Parses `--target`, `--entry`, and (for `run`) the replay-class opt-in flags.
#[cfg(unix)]
fn parse_drive_args(
    cmd: &str,
    rest: &[String],
    allow_class_flags: bool,
) -> Result<DriveArgs, String> {
    use qdb::orchestrate::TargetKind;

    let mut target = None;
    let mut only = Vec::new();
    let mut allow_modal = false;
    let mut allow_destructive = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--target" if i + 1 < rest.len() => {
                target = Some(
                    TargetKind::parse(&rest[i + 1])
                        .ok_or_else(|| format!("qdb {cmd}: unknown target {:?}", rest[i + 1]))?,
                );
                i += 2;
            }
            "--entry" if i + 1 < rest.len() => {
                only.push(rest[i + 1].clone());
                i += 2;
            }
            "--allow-modal" if allow_class_flags => {
                allow_modal = true;
                i += 1;
            }
            "--allow-destructive" if allow_class_flags => {
                allow_destructive = true;
                i += 1;
            }
            other => return Err(format!("qdb {cmd}: unexpected argument {other:?}")),
        }
    }
    let target = target.ok_or_else(|| format!("qdb {cmd}: --target tmux|betamax is required"))?;
    Ok(DriveArgs {
        target,
        only,
        allow_modal,
        allow_destructive,
    })
}

/// `qdb capture --target tmux|betamax [--entry <id>...]`: drive a real terminal and mint artifacts.
#[cfg(unix)]
fn cmd_capture(db_dir: &Path, repo_root: &Path, rest: &[String]) -> ExitCode {
    use qdb::orchestrate;

    let args = match parse_drive_args("capture", rest, false) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let db = match Database::load(db_dir) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("qdb capture: {e}");
            return ExitCode::FAILURE;
        }
    };
    match orchestrate::capture(&db, repo_root, args.target, &args.only) {
        Ok(s) => {
            println!(
                "qdb capture {}: {} answered, {} silent, {} unprobeable (version {:?})",
                s.target, s.answered, s.silent, s.unprobeable, s.version
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("qdb capture: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `qdb run --target tmux|betamax [--entry <id>...] [--allow-modal] [--allow-destructive]`:
/// the conformance pass — same loop as capture, results seed only.
#[cfg(unix)]
fn cmd_run(db_dir: &Path, repo_root: &Path, rest: &[String]) -> ExitCode {
    use qdb::capture::AllowedClasses;
    use qdb::orchestrate;

    let args = match parse_drive_args("run", rest, true) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let db = match Database::load(db_dir) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("qdb run: {e}");
            return ExitCode::FAILURE;
        }
    };
    let allowed = AllowedClasses {
        modal: args.allow_modal,
        destructive: args.allow_destructive,
    };
    match orchestrate::conformance(&db, repo_root, args.target, &args.only, allowed) {
        Ok(s) => {
            println!(
                "qdb run {}: {} answered, {} silent, {} unprobeable (version {:?})",
                s.target, s.answered, s.silent, s.unprobeable, s.version
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("qdb run: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `qdb target-relay --in <fifo> --out <fifo>`: internal — the in-terminal byte relay the
/// PTY-hosted adapters launch. Not part of the user-facing surface.
#[cfg(unix)]
fn cmd_target_relay(rest: &[String]) -> ExitCode {
    let mut fifo_in = None;
    let mut fifo_out = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--in" if i + 1 < rest.len() => {
                fifo_in = Some(PathBuf::from(&rest[i + 1]));
                i += 2;
            }
            "--out" if i + 1 < rest.len() => {
                fifo_out = Some(PathBuf::from(&rest[i + 1]));
                i += 2;
            }
            other => {
                eprintln!("qdb target-relay: unexpected argument {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }
    let (Some(fifo_in), Some(fifo_out)) = (fifo_in, fifo_out) else {
        eprintln!("qdb target-relay: --in <fifo> and --out <fifo> are required");
        return ExitCode::FAILURE;
    };
    match qdb::targets::relay::run(&fifo_in, &fifo_out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("qdb target-relay: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Locates the repo root: `CARGO_MANIFEST_DIR/../..` (tools/qdb -> root), overridable with
/// `QDB_ROOT` for tests.
fn repo_root() -> PathBuf {
    if let Ok(root) = env::var("QDB_ROOT") {
        return PathBuf::from(root);
    }
    let manifest = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest)
        .ancestors()
        .nth(2)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// `qdb validate`: load and check the database, printing violations.
fn cmd_validate(db_dir: &Path, repo_root: &Path) -> ExitCode {
    let db = match Database::load(db_dir) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("qdb validate: {e}");
            return ExitCode::FAILURE;
        }
    };
    let errors = validate::run(&db, repo_root);
    if errors.is_empty() {
        let n = db.entries().count();
        println!(
            "qdb validate: {n} entries across {} families OK",
            db.families.len()
        );
        ExitCode::SUCCESS
    } else {
        for e in &errors {
            eprintln!("qdb validate: {e}");
        }
        eprintln!("qdb validate: {} violation(s)", errors.len());
        ExitCode::FAILURE
    }
}

/// `qdb generate [--check] [docs|matrix]`: write or verify generated artifacts.
///
/// With no positional target, generates both `docs` and `matrix`. `docs` pages are ephemeral
/// build output (`target/qdb-docs/`, not checked in); `matrix` is the checked-in caniuse support
/// table (`db/caniuse.md`) — `--check` regenerates it to a temp buffer and diffs against the
/// committed file, the same drift-detection pattern `docs` already used.
fn cmd_generate(db_dir: &Path, repo_root: &Path, rest: &[String]) -> ExitCode {
    let check = rest.iter().any(|a| a == "--check");
    let targets: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--check")
        .collect();
    let (wants_docs, wants_matrix) = match targets.as_slice() {
        [] => (true, true),
        [t] if *t == "docs" => (true, false),
        [t] if *t == "matrix" => (false, true),
        _ => {
            eprintln!("usage: qdb generate [--check] [docs|matrix]");
            return ExitCode::FAILURE;
        }
    };

    let db = match Database::load(db_dir) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("qdb generate: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut ok = true;
    if wants_docs {
        ok &= cmd_generate_docs(&db, repo_root, check);
    }
    if wants_matrix {
        ok &= cmd_generate_matrix(&db, repo_root, check);
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Writes or verifies the ephemeral markdown reference pages under `target/qdb-docs/`.
fn cmd_generate_docs(db: &Database, repo_root: &Path, check: bool) -> bool {
    let out_dir = repo_root.join("target").join("qdb-docs");
    let pages = generate::pages(db);

    if check {
        let mut drift = Vec::new();
        for (name, contents) in &pages {
            let path = out_dir.join(name);
            match fs::read_to_string(&path) {
                Ok(existing) if &existing == contents => {}
                _ => drift.push(name.clone()),
            }
        }
        if drift.is_empty() {
            println!(
                "qdb generate --check docs: {} page(s) up to date",
                pages.len()
            );
            true
        } else {
            for d in &drift {
                eprintln!("qdb generate --check docs: drift in {d}");
            }
            eprintln!("qdb generate --check docs: run `qdb generate docs` to refresh");
            false
        }
    } else {
        if let Err(e) = fs::create_dir_all(&out_dir) {
            eprintln!("qdb generate: creating {}: {e}", out_dir.display());
            return false;
        }
        for (name, contents) in &pages {
            let path = out_dir.join(name);
            if let Err(e) = fs::write(&path, contents) {
                eprintln!("qdb generate: writing {}: {e}", path.display());
                return false;
            }
        }
        println!(
            "qdb generate docs: wrote {} page(s) to {}",
            pages.len(),
            out_dir.display()
        );
        true
    }
}

/// Writes or verifies the checked-in caniuse support matrix at `db/caniuse.md`.
fn cmd_generate_matrix(db: &Database, repo_root: &Path, check: bool) -> bool {
    let results = match Database::load_results(repo_root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("qdb generate matrix: {e}");
            return false;
        }
    };
    let contents = qdb::matrix::render(db, &results);
    let path = repo_root.join("db").join("caniuse.md");

    if check {
        match fs::read_to_string(&path) {
            Ok(existing) if existing == contents => {
                println!("qdb generate --check matrix: db/caniuse.md up to date");
                true
            }
            Ok(_) => {
                eprintln!("qdb generate --check matrix: drift in db/caniuse.md");
                eprintln!("qdb generate --check matrix: run `qdb generate matrix` to refresh");
                false
            }
            Err(e) => {
                eprintln!(
                    "qdb generate --check matrix: reading {}: {e}",
                    path.display()
                );
                false
            }
        }
    } else {
        if let Err(e) = fs::write(&path, &contents) {
            eprintln!("qdb generate matrix: writing {}: {e}", path.display());
            return false;
        }
        println!("qdb generate matrix: wrote {}", path.display());
        true
    }
}
