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

loom:
    RUSTFLAGS="--cfg loom" cargo test --lib loom_

markdown:
    markdownlint-cli2 "**/*.md"

# Verify the live query path against real terminal implementations, headless. Uses tmux and
# betamax (headless ghostty) when installed, skipping cleanly otherwise; both type into the
# session while interleaved queries run, exercising the typeahead-survival contract for real.
# Kept out of the `check` chain because the tools are not guaranteed everywhere.
verify-emulators:
    bash scripts/verify_emulators.sh

# Run each fuzz target briefly. Requires nightly + cargo-fuzz, which are not guaranteed locally, so
# this is deliberately kept out of the `check` chain; CI runs it in a dedicated job.
fuzz:
    cargo +nightly fuzz run syntax_reconstruction -- -max_total_time=30 -rss_limit_mb=1024
    cargo +nightly fuzz run syntax_split_equivalence -- -max_total_time=30 -rss_limit_mb=1024
    cargo +nightly fuzz run syntax_no_panic_bounded -- -max_total_time=30 -rss_limit_mb=1024
    cargo +nightly fuzz run correlator_properties -- -max_total_time=30 -rss_limit_mb=1024

check: metadata fmt-check test loom clippy doc markdown
