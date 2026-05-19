# Release Readiness

qwertty is not ready to publish yet. The crate still sets `publish = false`, and the first release
should happen only after the documented product surface, validation gates, and policy boundaries
agree on what qwertty is promising downstream users.

This page is the durable place to understand the current release-readiness bar without reading
issue history.

For the concrete execution checklist maintainers should use when publication work actually starts,
see [Release Checklist](crate::docs#release-checklist).

## What Must Already Exist

Before the first release, qwertty should keep these user-facing artifacts coherent:

- `README.md` should describe the product honestly at the current layer and point readers at the
  docs.rs-facing references.
- command, device, session, input, platform-support, and release-readiness reference pages should
  exist and match the public API surface.
- checked-in examples should cover the smallest important workflows for command encoding, session
  ownership, decoded input, and live Tokio query behavior.
- policy ADRs should record the current support, compatibility, package-boundary, and dependency
  rules that shape the release surface.

Release work should treat missing or contradictory docs as a blocker, not as cleanup for later.

## What The Current Release Surface Covers

The release candidate surface today is:

- runtime-neutral command encoding and protocol parsing in memory;
- Unix terminal device ownership and synchronous session lifecycle;
- optional Tokio-backed async session ownership on Unix;
- typed cursor-position and terminal-status query handling with documented timeout, cancellation,
  preserved-input, wrong-report, unmatched-input, and closed-terminal behavior;
- checked-in examples and docs.rs references for the public workflows above.

The release candidate surface does not yet claim:

- cross-platform live terminal ownership beyond the documented Unix-first boundary;
- a stable multi-runtime async abstraction;
- broader protocol families beyond the currently documented helpers;
- a finalized publication target, versioning milestone, or release automation story.

## Release-Blocking Validation

The first release should keep the current validation gate green:

- CI required checks on every PR;
- `cargo +nightly fmt --all -- --check`;
- `cargo test --workspace --all-features`;
- `cargo test --examples --workspace --all-features`;
- `cargo clippy --workspace --all-features --all-targets -- -D warnings`;
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps`;
- `markdownlint-cli2 "**/*.md"`;
- PTY-backed integration tests for live terminal behavior and query routing.

If a release depends on behavior not covered by those gates, the missing validation should land
before the release change rather than being treated as a follow-up.

## Policy Boundaries That Define Readiness

The current release posture depends on these durable decisions:

- [ADR 0013: Platform Support Policy](../adr/0013-platform-support-policy.md)
- [ADR 0014: Crate and Module Split Policy](../adr/0014-crate-and-module-split-policy.md)
- [ADR 0015: Versioning and Compatibility Policy](../adr/0015-versioning-and-compatibility-policy.md)
- [ADR 0016: Dependency Policy](../adr/0016-dependency-policy.md)
- [ADR 0017: First Release Target](../adr/0017-first-release-target.md)

Release work should not widen platform claims, dependency commitments, or compatibility promises
without updating the relevant ADR and the user-facing references together.

## What Still Needs A Deliberate Release Slice

Changing `publish = false`, choosing a first published version, adding release automation, or
declaring a broader stability promise should each arrive as an explicit release-planning slice.

That later slice should answer:

- what version should be published first;
- which examples and docs are treated as release-blocking;
- which platform and feature combinations are in scope for the first release;
- whether any additional integration or conformance evidence is still missing.

## Related References

- [Release Checklist](crate::docs#release-checklist)
- [Checked-In Examples](crate::docs#checked-in-examples)
- [Platform Support](crate::docs#platform-support)
- [Terminal Session Reference](crate::docs#terminal-session-reference)
- [Tokio Input Ownership And Query Handoff](crate::docs#tokio-input-ownership-and-query-handoff)
