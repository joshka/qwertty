# ADR 0017: First Release Target

## Status

Accepted

## Context

qwertty now has a clearer release surface than it did when the project began. It has a Unix-first
terminal ownership story, a Tokio-specific async session owner behind an optional feature, checked
query-routing behavior, docs.rs-facing reference pages, checked-in examples, and durable ADRs for
platform support, compatibility, dependency policy, and package boundaries.

The repository can already say what must be true before publication stops being future work, but it
still does not say what the first intended release is actually trying to deliver. Without that
target, later release work will have to answer the same scope question repeatedly:

- whether the first release is just a low-level foundation snapshot or a user-facing terminal
  library;
- whether Tokio-backed async session ownership belongs in the first published target;
- which roadmap areas should be finished before release and which should remain explicitly
  post-release work.

That scope belongs in a durable decision before publication work starts. Otherwise the repo can
describe readiness gates without describing readiness for what.

## Decision

Set the first release target as a Unix-first terminal library that already includes both the
runtime-neutral synchronous core and the optional Tokio async session surface.

The intended first release target includes:

- runtime-neutral command encoding, protocol value types, and parser surfaces that operate on bytes
  in memory;
- Unix terminal device ownership and synchronous terminal session lifecycle;
- the optional Tokio-backed async terminal session owner on Unix, including decoded input events,
  live cursor-position queries, live terminal-status queries, and the documented timeout,
  cancellation, wrong-report, unmatched-input, and preserved-input contracts;
- the current docs.rs-facing reference pages, checked-in examples, and validation gates that
  document and prove the public behavior above.

The first release target explicitly does not require:

- cross-platform live terminal ownership beyond the documented Unix-first boundary;
- a runtime-agnostic async abstraction;
- broader protocol-family coverage than the currently documented helpers;
- release automation, tagging workflow, or integration-specific adapters such as Ratatui support.

Release planning should treat those deferred areas as post-target work unless another ADR revises
the target intentionally.

The next implementation issue after this policy is
[Add first release target ADR](https://github.com/joshka/qwertty/issues/166).

## Consequences

- qwertty can prepare for a real first release without pretending that every future protocol,
  platform, or integration track must land first.
- Tokio-backed async session ownership is part of the intended first published product, not an
  experimental sidecar outside the release target.
- Deferred areas remain visible as deliberate post-target work rather than vague omissions.
- Later release slices can evaluate readiness against a fixed target instead of re-opening scope
  from scratch.

## Alternatives Considered

### Release Only The Synchronous Foundation First

That would reduce scope, but it would undercut the repo's current product direction. qwertty is
being built as an async-first terminal library, and the Tokio session surface already has docs,
examples, and validation that make it part of the practical product.

### Wait For Cross-Platform Live Support Before Any Release

That would delay publication until a larger support story exists, but the current policy and
documentation already define an honest Unix-first boundary. The first release does not need to
pretend broader platform reach than qwertty can prove.

### Require Broader Protocol And Integration Coverage Before Release

That would produce a larger first release, but it would also keep the initial publication coupled
to several still-open roadmap tracks. The first target should ship the coherent terminal-ownership
core that qwertty already documents well.

## Reference Material

- [Cargo Manifest](../../Cargo.toml)
- [Roadmap](../roadmap.md)
- [Release Readiness](../reference/release-readiness.md)
- [Platform Support](../reference/platform-support.md)
- [ADR 0013: Platform Support Policy](0013-platform-support-policy.md)
- [ADR 0015: Versioning and Compatibility Policy](0015-versioning-and-compatibility-policy.md)
- [ADR 0016: Dependency Policy](0016-dependency-policy.md)
- [Issue #166: Add first release target ADR](https://github.com/joshka/qwertty/issues/166)
