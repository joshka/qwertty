# Rust Style

Rust code should read like technical writing for future maintainers.

## Rules

- Prefer boring, explicit APIs over clever abstractions.
- Keep functions and types small enough to understand locally.
- Prefer caller-before-callee order when it improves top-to-bottom reading.
- Prefer feature-oriented modules and named files.
- Avoid `mod.rs` unless there is a strong reason.
- Do not force one import per line when grouped imports are clearer.
- Keep visibility narrow; do not default to `pub(crate)`.
- Use strong types where semantics matter.
- Treat dependency bumps and feature flags as public integration decisions.
- Add abstractions only when they reduce concepts a reader must hold at once.

## Formatting

Use `cargo +nightly fmt --all` for Rust formatting. The repository uses nightly-only rustfmt
settings to wrap code and doc comments at 100 columns, format Rust code in doc comments, normalize
doc attributes, and keep imports grouped at module granularity. These settings keep API docs and
examples readable while avoiding noisy one-import-per-line churn.

## Public APIs

Public APIs need practical examples and Rustdoc that explains relevant errors, invariants, safety,
policy, or protocol behavior. Examples should show realistic usage rather than isolated constructor
calls. Protocol-facing APIs should explain spec abbreviations, link stable references, and show the
actual bytes emitted or interpreted for representative inputs.
