//! Golden-file test: the generated `kitty-pointer` reference page must match the checked-in
//! golden exactly. This pins the doc generator's output shape and proves determinism.
//!
//! To refresh after an intentional generator change, run `qdb generate docs` and copy
//! `target/qdb-docs/kitty-pointer.md` over `tools/qdb/tests/golden/kitty-pointer.md`.

use std::path::{Path, PathBuf};

use qdb::generate;
use qdb::model::Database;

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

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}
