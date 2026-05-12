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

## Public APIs

Public APIs need practical examples and Rustdoc that explains relevant errors, invariants, safety,
policy, or protocol behavior. Examples should show realistic usage rather than isolated constructor
calls.
