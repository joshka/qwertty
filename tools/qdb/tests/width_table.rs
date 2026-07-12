//! Freshness test for the embedded width deviation table: the committed `src/width_table.rs` must
//! match a fresh render from the real `db/width/*.toml` data, byte for byte. This is the guard that
//! keeps the library's table in sync with its db/ source of truth — the lib-level twin of
//! `qdb width-table --check`.
//!
//! To refresh after an intentional width-data or generator change, run `cargo run -p qdb --
//! width-table`.

use std::path::{Path, PathBuf};

#[test]
fn embedded_width_table_matches_db() {
    let root = repo_root();
    let generated = qdb::width::render_rust_table(&root).expect("render width table");
    let committed = std::fs::read_to_string(root.join("src").join("width_table.rs"))
        .expect("read src/width_table.rs");
    assert_eq!(
        generated, committed,
        "src/width_table.rs drifted from db/width/*; run `cargo run -p qdb -- width-table` to refresh"
    );
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}
