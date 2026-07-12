//! `qdb` — developer tooling for the qwertty sequence database.
//!
//! Commands:
//! - `validate` — check the database against every design-05 grokkability rule.
//! - `generate [--check] [reference]` — writes the committed conformance reference tree under
//!   `docs/reference/generated/` (support matrix, compact summary, one page per family). `--check`
//!   regenerates in memory and fails on any drift — content, a missing page, or a stale extra file
//!   — instead of writing. docs.rs cannot run qdb, so the output is committed and CI-freshness-
//!   checked (playbook §9).
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
        Some("width-table") => cmd_width_table(&repo_root, &args[1..]),
        #[cfg(unix)]
        Some("capture") => cmd_capture(&db_dir, &repo_root, &args[1..]),
        #[cfg(unix)]
        Some("run") => cmd_run(&db_dir, &repo_root, &args[1..]),
        #[cfg(unix)]
        Some("width-probe") => cmd_width_probe(&repo_root, &args[1..]),
        #[cfg(unix)]
        Some("target-relay") => cmd_target_relay(&args[1..]),
        _ => {
            eprintln!(
                "usage: qdb <validate | generate [--check] [reference] | \
                 capture --target tmux|betamax|kitty|alacritty|wezterm|foot|xterm \
                 [--entry <id>...] | \
                 run --target tmux|betamax|kitty|alacritty|wezterm|foot|xterm [--entry <id>...] \
                 [--allow-modal] [--allow-destructive] | \
                 width-probe --target tmux|betamax|kitty|alacritty|wezterm|foot|xterm>"
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
    let target = target.ok_or_else(|| {
        format!("qdb {cmd}: --target tmux|betamax|kitty|alacritty|wezterm|foot|xterm is required")
    })?;
    Ok(DriveArgs {
        target,
        only,
        allow_modal,
        allow_destructive,
    })
}

/// `qdb capture --target tmux|betamax|kitty|alacritty|wezterm|foot|xterm [--entry <id>...]`: drive
/// a real terminal and mint artifacts.
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

/// `qdb run --target tmux|betamax|kitty|alacritty|wezterm|foot|xterm [--entry <id>...]
/// [--allow-modal] [--allow-destructive]`: the conformance pass — same loop as capture, results
/// seed only.
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

/// `qdb width-probe --target <name>`: measure the terminal's grapheme-cluster advances vs the
/// static unicode-width baseline and write the db/-owned deviation table `db/width/<target>.toml`
/// (design 09-width, hybrid mechanism). Observes mode 2027; never enables it.
#[cfg(unix)]
fn cmd_width_probe(repo_root: &Path, rest: &[String]) -> ExitCode {
    use qdb::orchestrate::TargetKind;
    use qdb::width;

    let mut target = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--target" if i + 1 < rest.len() => {
                target = TargetKind::parse(&rest[i + 1]);
                if target.is_none() {
                    eprintln!("qdb width-probe: unknown target {:?}", rest[i + 1]);
                    return ExitCode::FAILURE;
                }
                i += 2;
            }
            other => {
                eprintln!("qdb width-probe: unexpected argument {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }
    let Some(kind) = target else {
        eprintln!(
            "qdb width-probe: --target tmux|betamax|kitty|alacritty|wezterm|foot|xterm is required"
        );
        return ExitCode::FAILURE;
    };

    let mut adapter = kind.make();
    let identity = adapter.identity();
    let timestamp = qdb::orchestrate::utc_timestamp();
    let report = match width::probe(
        adapter.as_mut(),
        kind.slug(),
        kind.adapter_kind().as_results_str(),
        &identity.version_hint,
        &timestamp,
        120,
        40,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("qdb width-probe {}: {e}", kind.slug());
            return ExitCode::FAILURE;
        }
    };

    let path = repo_root
        .join("db")
        .join("width")
        .join(format!("{}.toml", kind.slug()));
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("qdb width-probe: creating {}: {e}", parent.display());
            return ExitCode::FAILURE;
        }
    }
    if let Err(e) = fs::write(&path, width::render_table(&report)) {
        eprintln!("qdb width-probe: writing {}: {e}", path.display());
        return ExitCode::FAILURE;
    }
    let deviations = report
        .measurements
        .iter()
        .filter(|m| m.advance != Some(m.uw))
        .count();
    println!(
        "qdb width-probe {}: {} clusters, {deviations} deviate from unicode-width, 2027 {} (version {:?})",
        report.target,
        report.measurements.len(),
        if report.supports_2027 {
            "recognised"
        } else {
            "unrecognised"
        },
        report.version,
    );
    ExitCode::SUCCESS
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

/// `qdb generate [--check] [reference]`: write or verify the committed reference tree under
/// `docs/reference/generated/`.
///
/// `reference` is the only target (and the default). `--check` renders every page in memory and
/// fails on any drift — a changed page, a missing one, or a stale extra `.md` left in the
/// directory — the freshness gate CI runs. Without `--check`, it writes the tree, pruning any
/// stale `.md` files so the committed directory is exactly the generated set.
fn cmd_generate(db_dir: &Path, repo_root: &Path, rest: &[String]) -> ExitCode {
    let check = rest.iter().any(|a| a == "--check");
    let targets: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--check")
        .collect();
    match targets.as_slice() {
        [] => {}
        [t] if *t == "reference" => {}
        _ => {
            eprintln!("usage: qdb generate [--check] [reference]");
            return ExitCode::FAILURE;
        }
    }

    let db = match Database::load(db_dir) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("qdb generate: {e}");
            return ExitCode::FAILURE;
        }
    };
    let results = match Database::load_results(repo_root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("qdb generate: {e}");
            return ExitCode::FAILURE;
        }
    };

    let pages = generate::reference(&db, &results);
    let out_dir = repo_root.join(generate::OUTPUT_DIR);

    if check {
        cmd_generate_check(&out_dir, &pages)
    } else {
        cmd_generate_write(&out_dir, &pages)
    }
}

/// `qdb width-table [--check]`: write or verify the embedded `src/width_table.rs`, generated from
/// the `db/width/*.toml` deviation tables (the C2 embedding). `--check` fails on drift instead of
/// writing — the freshness gate keeping the library's table in sync with the db/ source of truth.
fn cmd_width_table(repo_root: &Path, rest: &[String]) -> ExitCode {
    let check = rest.iter().any(|a| a == "--check");
    if rest.iter().any(|a| a != "--check") {
        eprintln!("usage: qdb width-table [--check]");
        return ExitCode::FAILURE;
    }
    let generated = match qdb::width::render_rust_table(repo_root) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("qdb width-table: {e}");
            return ExitCode::FAILURE;
        }
    };
    let path = repo_root.join("src").join("width_table.rs");
    if check {
        match fs::read_to_string(&path) {
            Ok(existing) if existing == generated => {
                println!("qdb width-table --check: src/width_table.rs up to date");
                ExitCode::SUCCESS
            }
            Ok(_) => {
                eprintln!("qdb width-table --check: src/width_table.rs drifted from db/width/*");
                eprintln!("qdb width-table --check: run `qdb width-table` to refresh");
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("qdb width-table --check: reading {}: {e}", path.display());
                ExitCode::FAILURE
            }
        }
    } else if let Err(e) = fs::write(&path, &generated) {
        eprintln!("qdb width-table: writing {}: {e}", path.display());
        ExitCode::FAILURE
    } else {
        println!("qdb width-table: wrote {}", path.display());
        ExitCode::SUCCESS
    }
}

/// Whether a directory entry is a Markdown page (used to scope stale-extra detection to `.md`
/// files, leaving any non-generated file kinds alone).
fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|x| x.to_str()) == Some("md")
}

/// Verifies the committed tree matches the generated pages exactly — content, presence, and no
/// stale extras. Returns SUCCESS only when the directory is byte-for-byte the generated set.
fn cmd_generate_check(out_dir: &Path, pages: &[(String, String)]) -> ExitCode {
    let mut drift = Vec::new();
    for (name, contents) in pages {
        match fs::read_to_string(out_dir.join(name)) {
            Ok(existing) if &existing == contents => {}
            Ok(_) => drift.push(format!("{name} (content differs)")),
            Err(_) => drift.push(format!("{name} (missing)")),
        }
    }
    // Stale extras: a committed `.md` the generator no longer emits (e.g. a removed family).
    let expected: std::collections::BTreeSet<&str> =
        pages.iter().map(|(n, _)| n.as_str()).collect();
    if let Ok(entries) = fs::read_dir(out_dir) {
        for e in entries.filter_map(Result::ok) {
            if !is_markdown(&e.path()) {
                continue;
            }
            if let Some(name) = e.file_name().to_str() {
                if !expected.contains(name) {
                    drift.push(format!("{name} (stale extra)"));
                }
            }
        }
    }
    if drift.is_empty() {
        println!(
            "qdb generate --check reference: {} page(s) up to date",
            pages.len()
        );
        ExitCode::SUCCESS
    } else {
        drift.sort();
        for d in &drift {
            eprintln!("qdb generate --check reference: drift in {d}");
        }
        eprintln!("qdb generate --check reference: run `qdb generate reference` to refresh");
        ExitCode::FAILURE
    }
}

/// Writes the reference tree, pruning any stale `.md` files so the directory is exactly the
/// generated set.
fn cmd_generate_write(out_dir: &Path, pages: &[(String, String)]) -> ExitCode {
    if let Err(e) = fs::create_dir_all(out_dir) {
        eprintln!("qdb generate: creating {}: {e}", out_dir.display());
        return ExitCode::FAILURE;
    }
    let expected: std::collections::BTreeSet<&str> =
        pages.iter().map(|(n, _)| n.as_str()).collect();
    if let Ok(entries) = fs::read_dir(out_dir) {
        for e in entries.filter_map(Result::ok) {
            if !is_markdown(&e.path()) {
                continue;
            }
            if let Some(name) = e.file_name().to_str() {
                if !expected.contains(name) {
                    if let Err(err) = fs::remove_file(e.path()) {
                        eprintln!("qdb generate: pruning {name}: {err}");
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
    }
    for (name, contents) in pages {
        let path = out_dir.join(name);
        if let Err(e) = fs::write(&path, contents) {
            eprintln!("qdb generate: writing {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }
    println!(
        "qdb generate reference: wrote {} page(s) to {}",
        pages.len(),
        out_dir.display()
    );
    ExitCode::SUCCESS
}
