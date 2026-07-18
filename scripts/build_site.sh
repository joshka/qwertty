#!/usr/bin/env bash
# Populates site/src/ from the committed conformance reference (docs/reference/generated/) for
# mdBook to build. Idempotent: safe to rerun, always starts from a clean site/src/.
#
# mdBook requires SUMMARY.md to live directly in its source directory, but the generated tree
# already has a summary.md (the compact docs.rs page, src/docs.rs::conformance) — on a
# case-insensitive filesystem (default macOS) those two names collide onto the same file. Two
# things keep this script clear of that: SUMMARY.md is written into site/src/, never into
# docs/reference/generated/, and the copied compact page is renamed away from "summary.md" on
# its way into site/src/.
#
# SUMMARY.md's family list is derived from docs/reference/generated/README.md's own "## Families"
# section (freshness-gated by `qdb generate --check reference`) rather than hand-maintained, so
# adding a family to the database can never let this site's navigation drift out of sync.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
generated="$repo_root/docs/reference/generated"
site_src="$repo_root/site/src"

if [ ! -f "$generated/README.md" ]; then
    echo "build_site.sh: $generated/README.md not found; run 'cargo run -p qdb -- generate reference' first" >&2
    exit 1
fi

rm -rf "$site_src"
mkdir -p "$site_src"

for f in "$generated"/*.md; do
    name="$(basename "$f")"
    if [ "$name" = "summary.md" ]; then
        cp "$f" "$site_src/conformance-counts.md"
    else
        cp "$f" "$site_src/$name"
    fi
done

# The hand-maintained specification index lives in docs/reference/ (a sibling of the generated
# tree, so it is exempt from the generate freshness gate). Copy it in as an appendix page.
cp "$generated/../spec-index.md" "$site_src/spec-index.md"

{
    echo "# Summary"
    echo
    echo "[Introduction](README.md)"
    echo
    echo "- [Support matrix](matrix.md)"
    echo "- [Conformance summary](conformance-counts.md)"
    echo "- [Specification index](spec-index.md)"
    echo
    grep -E '^- \[`' "$generated/README.md"
} > "$site_src/SUMMARY.md"

echo "build_site.sh: wrote $(find "$site_src" -name '*.md' | wc -l | tr -d ' ') page(s) to $site_src"
