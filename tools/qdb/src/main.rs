//! `qdb` — developer tooling for the qwertty sequence database.
//!
//! Commands:
//! - `validate` — check the database against every design-05 grokkability rule.
//! - `generate --check docs` — regenerate the markdown reference and fail if it drifts; `generate
//!   docs` writes the pages to `target/qdb-docs/`.

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
        _ => {
            eprintln!("usage: qdb <validate | generate [--check] docs>");
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

/// `qdb generate [--check] docs`: write or verify the markdown reference pages.
fn cmd_generate(db_dir: &Path, repo_root: &Path, rest: &[String]) -> ExitCode {
    let check = rest.iter().any(|a| a == "--check");
    let wants_docs = rest.iter().any(|a| a == "docs");
    if !wants_docs {
        eprintln!("usage: qdb generate [--check] docs");
        return ExitCode::FAILURE;
    }
    let db = match Database::load(db_dir) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("qdb generate: {e}");
            return ExitCode::FAILURE;
        }
    };
    let out_dir = repo_root.join("target").join("qdb-docs");
    let pages = generate::pages(&db);

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
            ExitCode::SUCCESS
        } else {
            for d in &drift {
                eprintln!("qdb generate --check docs: drift in {d}");
            }
            eprintln!("qdb generate --check docs: run `qdb generate docs` to refresh");
            ExitCode::FAILURE
        }
    } else {
        if let Err(e) = fs::create_dir_all(&out_dir) {
            eprintln!("qdb generate: creating {}: {e}", out_dir.display());
            return ExitCode::FAILURE;
        }
        for (name, contents) in &pages {
            let path = out_dir.join(name);
            if let Err(e) = fs::write(&path, contents) {
                eprintln!("qdb generate: writing {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
        println!(
            "qdb generate docs: wrote {} page(s) to {}",
            pages.len(),
            out_dir.display()
        );
        ExitCode::SUCCESS
    }
}
