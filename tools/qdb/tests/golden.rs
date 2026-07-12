//! Golden-file test: the committed `docs/reference/generated/` tree must match a fresh render from
//! the real database and its `db/results/*.toml` files, byte for byte. This pins the generated
//! output shape and proves determinism — the lib-level twin of the `qdb generate --check reference`
//! freshness gate CI runs.
//!
//! To refresh after an intentional generator or data change, run
//! `cargo run -p qdb -- generate reference`.

use std::path::{Path, PathBuf};

use qdb::generate;
use qdb::model::Database;

#[test]
fn reference_tree_matches_checked_in() {
    let root = repo_root();
    let db = Database::load(&root.join("db")).unwrap();
    let results = Database::load_results(&root).unwrap();
    let pages = generate::reference(&db, &results);
    let dir = root.join(generate::OUTPUT_DIR);

    assert!(
        pages.iter().any(|(n, _)| n == "matrix.md"),
        "reference tree must include the support matrix"
    );
    for (name, contents) in &pages {
        let committed = std::fs::read_to_string(dir.join(name)).unwrap_or_else(|_| {
            panic!("missing generated page {name}; run `cargo run -p qdb -- generate reference`")
        });
        assert_eq!(
            *contents, committed,
            "generated {name} drifted from the checked-in tree; \
             run `cargo run -p qdb -- generate reference` to refresh"
        );
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}
