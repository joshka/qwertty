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
    # Also run the crate's doctests with DEFAULT features. The all-features run above hides doctests
    # that only compile without the Tokio session (a default build legitimately lacks
    # `TokioTerminalSession`), so a reference `rust` fence using a Tokio-only API would break a real
    # default-feature `cargo test` while passing here. This closes that hole.
    cargo test -p qwertty --doc

clippy:
    cargo clippy --workspace --all-features --all-targets -- -D warnings
    # The all-targets, all-features pass above compiles the library with Tokio and with test cfg,
    # which hides dead code that only a default-feature, non-test build would surface (for example
    # the Tokio-only consumers of the sans-io correlator). This lints the library exactly as a
    # default-feature dependency sees it.
    cargo clippy -p qwertty -- -D warnings

doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
    # The all-features run above hides default-feature doc breakage (for example an intra-doc link
    # that only resolves under `tokio`), the same gate-hole class already closed for default clippy
    # and default doctests. This lints the crate's docs exactly as a default-feature dependency sees
    # them.
    RUSTDOCFLAGS="-D warnings" cargo doc -p qwertty --no-deps

# Build the docs the way docs.rs and a reader see them: all features, `--cfg docsrs` so
# feature-gated items (like TokioTerminalSession) show their "Available on feature" badges. Use this
# to browse, not the `doc` gate above (which ends on a default-feature build that omits the tokio
# surface). Needs nightly for the badges.
docs:
    RUSTDOCFLAGS="--cfg docsrs" cargo +nightly doc -p qwertty --all-features --no-deps

# Build the reader docs and serve them over HTTP so the rustdoc theme persists across pages (unlike
# file://). Browse http://127.0.0.1:8347/qwertty/.
docs-serve: docs
    python3 -m http.server 8347 --bind 127.0.0.1 --directory target/doc

loom:
    RUSTFLAGS="--cfg loom" cargo test --lib loom_

markdown:
    markdownlint-cli2 "**/*.md"

# Spell-check the tree with typos (config in typos.toml). Pure, fast, and a plain `cargo install
# typos-cli` away, so it joins the `check` chain. CI installs it via taiki-e/install-action.
typos:
    typos

# Validate the sequence database: id format, unique ids, ref resolution, fixture existence and
# header/direction agreement, replay class, reply linkage, non-empty descriptions. Pure and fast,
# so it joins the `check` chain.
qdb-validate:
    cargo run -p qdb -- validate

# Verify the checked-in caniuse support matrix (db/caniuse.md) is a byte-for-byte rendering of
# db/results/*.toml + the database entries — no live terminal needed, so it joins the `check`
# chain. Regenerate with `cargo run -p qdb -- generate matrix` when it legitimately drifts.
qdb-generate-check:
    cargo run -p qdb -- generate --check matrix

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

# Cross-compile the library to Windows and wasm with warnings denied, proving the
# platform-neutral surface builds off Unix. Requires the targets (rustup target add
# x86_64-pc-windows-msvc wasm32-unknown-unknown); kept out of the `check` chain because they are
# not guaranteed locally. CI runs this plus the real windows-latest test job.
check-cross:
    cargo clippy -p qwertty --target x86_64-pc-windows-msvc -- -D warnings
    cargo clippy -p qwertty --target wasm32-unknown-unknown -- -D warnings

# Supply-chain and dependency-hygiene recipes mirroring the new CI jobs. Each needs a cargo plugin
# that is not guaranteed locally, so — like `verify-emulators` and `capture` — they skip cleanly
# with an install hint when the tool is absent rather than failing the run. CI installs the tools
# via taiki-e/install-action, so there they always execute. Kept OUT of the `check` chain for the
# same reason; run them individually or via `check-supply-chain`.

# License / advisory / bans / sources policy (deny.toml). Install: cargo install cargo-deny.
deny:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-deny >/dev/null; then
        echo 'cargo-deny not installed; skipping (cargo install cargo-deny)'; exit 0
    fi
    cargo deny check advisories
    cargo deny check bans licenses sources

# Gate the public API against breaking changes vs the published baseline. qwertty is pre-publish, so
# with no baseline this is a no-op today. Install: cargo install cargo-semver-checks.
semver-checks:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-semver-checks >/dev/null; then
        echo 'cargo-semver-checks not installed; skipping (cargo install cargo-semver-checks)'; exit 0
    fi
    # Pre-publish tolerance: no crates.io baseline yet means "not found in registry"; treat that
    # single case as a skip. Any real breaking change (once a baseline exists) still fails.
    output=$(cargo semver-checks check-release -p qwertty 2>&1) && status=0 || status=$?
    echo "$output"
    if [ "$status" -ne 0 ] && echo "$output" | grep -q "not found in registry"; then
        echo 'qwertty not yet published; no semver baseline. Skipping.'; exit 0
    fi
    exit "$status"

# Verify dependency lower bounds are accurate. Install: cargo install cargo-minimal-versions cargo-hack.
minimal-versions:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-minimal-versions >/dev/null; then
        echo 'cargo-minimal-versions not installed; skipping (cargo install cargo-minimal-versions cargo-hack)'; exit 0
    fi
    cargo minimal-versions check --direct --workspace --all-features

# Detect unused dependencies. Install: cargo install cargo-machete.
machete:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-machete >/dev/null; then
        echo 'cargo-machete not installed; skipping (cargo install cargo-machete)'; exit 0
    fi
    cargo machete

# Run every supply-chain / dependency-hygiene recipe. Each skips cleanly if its tool is absent.
check-supply-chain: deny semver-checks minimal-versions machete

check: metadata fmt-check typos test loom clippy doc markdown qdb-validate qdb-generate-check
