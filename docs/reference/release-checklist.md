# Release Checklist

This page is the execution checklist for the first qwertty publication slice.

Use it when release work is actually starting. The goal is not to restate every design decision in
full, but to give maintainers one durable list of checks to run before changing `publish = false`.

## Product And Scope Confirmation

Before publication work starts, confirm that the intended release still matches
[ADR 0017: First Release Target](../adr/0017-first-release-target.md):

- the release is still Unix-first for live terminal ownership;
- the synchronous command, device, and session surface is part of the target;
- the optional Tokio async session surface is part of the target;
- broader platform claims, broader protocol families, release automation, and multi-runtime async
  abstraction are still explicitly deferred.

If the intended release has changed, update the ADR and user-facing references before publishing.

Confirm as part of semver review that capability probing
(`TokioTerminalSession::probe_capabilities`, `Capabilities`) is gated behind the `tokio` feature by
design, not an accidental gap: probing needs a bounded, cancellable, concurrent query bundle, which
is the async session's job. A synchronous, no-runtime probing entry point is out of scope for
`0.1.0` and is not a blocker.

## Public Docs And Examples Review

Re-read the public contract together before publication:

- `README.md`
- [`platform-support.md`](platform-support.md)
- [`release-readiness.md`](release-readiness.md)
- [`capability-model.md`](capability-model.md)
- [`terminal-device.md`](terminal-device.md)
- [`terminal-session.md`](terminal-session.md)
- [`terminal-session-tokio.md`](terminal-session-tokio.md)
- [`terminal-input.md`](terminal-input.md)
- [`terminal-control.md`](terminal-control.md)
- [`examples.md`](examples.md)
- [`release-blocking-examples.md`](release-blocking-examples.md)
- [`tokio-input-ownership.md`](tokio-input-ownership.md)
- [`db/README.md`](../../db/README.md)

Confirm that those docs agree on:

- what qwertty does today, including session lifecycle (sync and Tokio), security policy,
  suspend/resume/handoff, the terminal signals and resize streams, kitty keyboard, capability
  probing, and the sequence database;
- which platforms are supported;
- which examples are the intended starting points;
- which checked-in examples are treated as release-blocking for `0.1.0`, and that every checked-in
  example is indexed in `examples.md` with no orphans in either direction;
- what live query helpers guarantee about timeouts, cancellation, preserved input, wrong-report
  input, unmatched query-shaped input, and closed-terminal failures, for both the synchronous
  blocking query path and the Tokio async query path.

Keep public artifacts free of rewrite or prototype framing. Release-facing docs should read as the
intended product line, not as a migration story from private exploratory work.

## Policy And ADR Review

Reconfirm the current release boundary against the policy ADRs:

- [ADR 0013: Platform Support Policy](../adr/0013-platform-support-policy.md)
- [ADR 0014: Crate And Module Split Policy](../adr/0014-crate-and-module-split-policy.md)
- [ADR 0015: Versioning And Compatibility Policy](../adr/0015-versioning-and-compatibility-policy.md)
- [ADR 0016: Dependency Policy](../adr/0016-dependency-policy.md)
- [ADR 0017: First Release Target](../adr/0017-first-release-target.md)
- [ADR 0018: First Published Version](../adr/0018-first-published-version.md)

Do not widen support claims, dependency commitments, compatibility promises, or package boundaries
in the publishing PR unless the relevant ADR and user-facing docs are updated in the same slice.

## Validation Gate

Run the current release-blocking checks:

```sh
cargo +nightly fmt --all -- --check
cargo test --workspace --all-features
cargo test --doc --workspace --all-features
cargo test -p qwertty --doc
cargo test --examples --workspace --all-features
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo clippy -p qwertty -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
RUSTDOCFLAGS="-D warnings" cargo doc -p qwertty --no-deps
markdownlint-cli2 "**/*.md"
cargo run -p qdb -- validate
cargo run -p qdb -- generate --check matrix
```

The default-feature doctest and doc-build steps (`cargo test -p qwertty --doc` and
`cargo doc -p qwertty --no-deps`) exist because the `--all-features` runs above hide breakage that
only a default-feature (no `tokio`) consumer would see — a `rust` fence or intra-doc link that only
compiles or resolves under `tokio` would otherwise pass this gate and break a default-feature
downstream build. Likewise `cargo clippy -p qwertty -- -D warnings` lints the library exactly as a
default-feature dependency sees it. See the `justfile` `test`, `clippy`, and `doc` recipes.

Also confirm that PTY-backed integration coverage for live terminal behavior and query routing is
still present and green through the normal test suite.

If release confidence depends on behavior that is not covered by those checks, add the missing
validation before publishing instead of treating it as follow-up work.

## Release Slice Checklist

The publishing change itself should answer these questions explicitly:

- What version is being published first?
- Which Cargo features are part of the supported release surface?
- Which platform combinations are supported for that version?
- Which docs and examples are treated as release-blocking?
- Which known gaps are intentionally deferred after the first release?

That release slice should also:

- update `Cargo.toml` consistently with the intended publication decision;
- keep the checked-in docs.rs surface aligned with the published crate surface;
- leave the roadmap and durable GitHub indexes pointing at the next post-release or follow-on work.

## What This Checklist Does Not Approve Automatically

This checklist does not by itself approve:

- broader platform support than the current Unix-first contract;
- a stable multi-runtime async abstraction;
- broader protocol-family claims than the current documented helpers;
- release automation or tagging policy;
- compatibility shims that preserve accidental pre-release shapes.

Those choices need their own explicit planning slice if they become part of the release.

## Related References

- [Release-Blocking Examples](crate::docs#release-blocking-examples)
- [Release Readiness](crate::docs#release-readiness)
- [Platform Support](crate::docs#platform-support)
- [Checked-In Examples](crate::docs#checked-in-examples)
- [Terminal Session Reference](crate::docs#terminal-session-reference)
- [Tokio Input Ownership And Query Handoff](crate::docs#tokio-input-ownership-and-query-handoff)
