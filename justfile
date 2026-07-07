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
    # The all-targets, all-features pass above compiles the library with Tokio and with test cfg,
    # which hides dead code that only a default-feature, non-test build would surface (for example
    # the Tokio-only consumers of the sans-io correlator). This lints the library exactly as a
    # default-feature dependency sees it.
    cargo clippy -p qwertty -- -D warnings

doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps

loom:
    RUSTFLAGS="--cfg loom" cargo test --lib loom_

markdown:
    markdownlint-cli2 "**/*.md"

# Validate the sequence database: id format, unique ids, ref resolution, fixture existence and
# header/direction agreement, replay class, reply linkage, non-empty descriptions. Pure and fast,
# so it joins the `check` chain.
qdb-validate:
    cargo run -p qdb -- validate

# Verify the live query path against real terminal implementations, headless. Uses tmux and
# betamax (headless ghostty) when installed, skipping cleanly otherwise; both type into the
# session while interleaved queries run, exercising the typeahead-survival contract for real.
# Kept out of the `check` chain because the tools are not guaranteed everywhere.
verify-emulators:
    bash scripts/verify_emulators.sh

# Drive the live-capture harness against every installed target, recording reply bytes and identity
# into db/captures/, minting origin=capture: fixtures, and seeding db/results/. Skips a target whose
# tool is missing (like verify-emulators). Kept out of the `check` chain: it needs real terminals
# and mutates checked-in artifacts, so it is a deliberate, reviewed step, not part of the gate.
capture:
    #!/usr/bin/env bash
    set -euo pipefail
    ran_any=0
    if command -v tmux >/dev/null; then
        cargo run -q -p qdb -- capture --target tmux
        ran_any=1
    else
        echo 'tmux not installed; skipping'
    fi
    if command -v betamax >/dev/null; then
        cargo run -q -p qdb -- capture --target betamax
        ran_any=1
    else
        echo 'betamax not installed; skipping'
    fi
    if [ "$ran_any" -eq 0 ]; then
        echo 'no capture target available; nothing captured'
    fi

# Run each fuzz target briefly. Requires nightly + cargo-fuzz, which are not guaranteed locally, so
# this is deliberately kept out of the `check` chain; CI runs it in a dedicated job.
fuzz:
    cargo +nightly fuzz run syntax_reconstruction -- -max_total_time=30 -rss_limit_mb=1024
    cargo +nightly fuzz run syntax_split_equivalence -- -max_total_time=30 -rss_limit_mb=1024
    cargo +nightly fuzz run syntax_no_panic_bounded -- -max_total_time=30 -rss_limit_mb=1024
    cargo +nightly fuzz run correlator_properties -- -max_total_time=30 -rss_limit_mb=1024

check: metadata fmt-check test loom clippy doc markdown qdb-validate
