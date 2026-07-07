//! Golden-file tests: generated output must match its checked-in file exactly. This pins each
//! generator's output shape and proves determinism.
//!
//! To refresh `kitty-pointer.md` after an intentional doc-generator change, run `qdb generate docs`
//! and copy `target/qdb-docs/kitty-pointer.md` over `tools/qdb/tests/golden/kitty-pointer.md`.
//!
//! To refresh `db/caniuse.md` after an intentional matrix-generator or results change, run
//! `cargo run -p qdb -- generate matrix`.

use std::path::{Path, PathBuf};

use qdb::model::Database;
use qdb::{generate, matrix};

#[test]
fn kitty_pointer_page_matches_golden() {
    let root = repo_root();
    let db = Database::load(&root.join("db")).unwrap();
    let family = db
        .families
        .iter()
        .find(|f| f.name == "kitty-pointer")
        .expect("kitty-pointer family present");
    let rendered = generate::render_family(&db, family);
    let golden = include_str!("golden/kitty-pointer.md");
    assert_eq!(
        rendered, golden,
        "generated kitty-pointer page drifted from the golden; refresh it if intentional"
    );
}

/// Pins the checked-in `db/caniuse.md` against a fresh render from the real database and its
/// checked-in `db/results/*.toml` files. This is the "real caniuse.md" golden the deliverable asks
/// for: it must reflect the actual capture run (tmux answered 8, betamax answered 7) and stay
/// byte-for-byte what `qdb generate matrix` would (re)write.
#[test]
fn caniuse_matrix_matches_checked_in_file() {
    let root = repo_root();
    let db = Database::load(&root.join("db")).unwrap();
    let results = Database::load_results(&root).unwrap();
    let rendered = matrix::render(&db, &results);
    let checked_in = std::fs::read_to_string(root.join("db").join("caniuse.md")).unwrap();
    assert_eq!(
        rendered, checked_in,
        "generated caniuse.md drifted from db/caniuse.md; run `cargo run -p qdb -- generate matrix` \
         to refresh"
    );
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}
