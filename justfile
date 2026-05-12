set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    just --list

fmt:
    cargo +nightly fmt --all

fmt-check:
    cargo +nightly fmt --all -- --check

metadata:
    cargo metadata --format-version 1 --no-deps

test:
    cargo test --workspace --all-features
    cargo test --examples --workspace --all-features

clippy:
    cargo clippy --workspace --all-features --all-targets -- -D warnings

doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps

markdown:
    markdownlint-cli2 "**/*.md"

check: metadata fmt-check test clippy doc markdown
