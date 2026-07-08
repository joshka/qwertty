# Contributing

Thanks for working on qwertty. Contributions should favor small, reviewable changes that make the
library more reliable, easier to maintain, or closer to the documented terminal behavior.

If your contribution is not straightforward, please open an issue to discuss the change before
making it.

## Project Shape

qwertty is a Rust workspace with a single publishable crate:

- `qwertty` (repository root): the published terminal library.
- `tools/qdb`: an unpublished developer tool for the sequence database. It shares no semver with the
  library and is not part of the public API surface.

qwertty is Unix-first. It ships a synchronous session owner and an optional Tokio-backed async
session owner behind the `tokio` feature. Keep terminal ownership, ordered output, and policy
behavior in mind when changing shared code.

## Development Setup

This is a standard Rust project managed with [rustup].

```sh
git clone https://github.com/joshka/qwertty
cd qwertty
cargo test --workspace --all-features
```

Rust formatting uses nightly rustfmt because `rustfmt.toml` enables unstable formatting options:

```sh
rustup toolchain install nightly --component rustfmt
```

The `rust-version` in `Cargo.toml` is a compatibility floor, not a separately tested MSRV lane. It
should move only when qwertty code or dependency requirements need a newer compiler.

## Before Opening a Pull Request

Run the local gate:

```sh
just check
```

List available recipes with:

```sh
just --list
```

## Useful Checks

Use narrower commands while iterating:

```sh
cargo +nightly fmt --all
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo test --workspace --all-features
cargo run -p qdb -- validate
markdownlint-cli2 "**/*.md"
```

## Commit Messages

qwertty does not use Conventional Commits. Write clear, imperative commit and pull-request titles
such as `Add resize fallback stream` or `Fix lone-Escape flush timing`. Version bumps are detected
from the public API diff by release-plz (via cargo-semver-checks), not from commit subjects.

## Documentation Style

Markdown is linted with `markdownlint-cli2`.

- Wrap prose at 100 columns.
- Never wrap code blocks or tables.
- Use fenced code blocks with a language when the language is known.
- Keep Markdown tables aligned with a leading-and-trailing pipe style.

Rust documentation should explain non-obvious behavior, return values, edge cases, errors,
invariants, safety, policy, and protocol rationale. The goal is useful long-term maintenance
context.

## Release Notes

User-visible changes should update [CHANGELOG.md](CHANGELOG.md) by hand in keep-a-changelog format.
Keep entries short and focused on behavior: new public API, changed defaults, supported features,
important bug fixes, and known limitations.

[rustup]: https://rustup.rs/
