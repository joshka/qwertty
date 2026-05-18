# ADR 0015: Versioning and Compatibility Policy

## Status

Accepted

## Context

qwertty has grown from a small command and terminal-ownership foundation into a library with
feature-gated Tokio integration, docs.rs-facing reference pages, checked-in examples, and several
durable ADR-backed boundaries. Future changes now affect more than local implementation shape. They
also affect what maintainers, reviewers, and eventual downstream users should treat as part of the
public contract.

At the same time, qwertty is still intentionally pre-release. The crate version is `0.0.0`, the
manifest sets `publish = false`, and the project is still choosing its first external release
surface. That means the library should not pretend to offer a stable 1.x compatibility promise
yet. It still needs a durable policy for how to handle change while the public line is forming.

Without that policy, later work will answer compatibility questions inconsistently:

- when a public API rename is acceptable;
- how optional feature flags affect compatibility expectations;
- whether checked-in examples and rendered docs are treated as part of the public contract;
- what evidence is required before widening or tightening platform and support claims.

## Decision

Treat qwertty as a pre-release `0.x` library with explicit but limited compatibility promises.

The current policy is:

- qwertty may still make breaking changes to public APIs before the first published release when
  the change improves the product shape materially;
- breaking changes are not routine cleanup. They require an issue or PR explanation that states the
  compatibility impact and why the new shape is better;
- public APIs, feature flags, rendered docs, checked-in examples, and documented platform claims
  are all part of the reviewed public contract and must be updated together when one of them
  changes;
- optional feature flags should remain additive where practical. Renaming, removing, or changing a
  feature flag's meaning is a compatibility change even before 1.0;
- examples and docs do not freeze every internal detail, but once qwertty documents a user-facing
  workflow or supported behavior, maintainers should treat that behavior as part of the contract
  until it is intentionally revised;
- support claims widen only when docs, tests, examples, and error behavior agree on the new claim;
- moving away from `0.0.0`, turning `publish` on, or claiming a stronger stability story requires
  a dedicated release-planning slice rather than happening incidentally inside another change.

In practice, this means qwertty should prefer small intentional changes that keep docs, examples,
tests, and issue rationale aligned. Pre-release status lowers the cost of necessary redesign, but
it does not excuse accidental breakage or undocumented contract drift.

The next implementation issue after this policy is
[Add versioning and compatibility policy ADR](https://github.com/joshka/qwertty/issues/154).

## Consequences

- Maintainers have room to improve the API before release without pretending the current shape is
  permanently stable.
- Public changes need explicit compatibility reasoning instead of being treated as harmless because
  the crate is still unpublished.
- Feature flags, examples, and docs stay tied to API review instead of drifting into separate
  informal promises.
- Release planning now has a durable rule for when qwertty can claim a stronger compatibility
  posture.

## Alternatives Considered

### Treat Pre-Release As No Compatibility Promise At All

That would make redesign easy, but it would also make docs, examples, and feature flags unreliable
as planning surfaces. qwertty needs stronger discipline than that even before release.

### Promise Semver-Like Stability Immediately

That would reduce future churn for early readers, but it would freeze several surfaces before the
library has enough downstream evidence to justify them.

### Keep The Policy Only In Development Rules

The development rules are useful implementation guidance, but they are not the durable product
record. Versioning and compatibility decisions belong in an ADR that future maintainers can cite
directly.

## Reference Material

- [Cargo Manifest](../../Cargo.toml)
- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Non-Functional Requirements](../non-functional-requirements.md)
- [Platform Support](../reference/platform-support.md)
- [ADR 0011: Tokio Async Runtime Boundary](0011-tokio-async-runtime-boundary.md)
- [ADR 0013: Platform Support Policy](0013-platform-support-policy.md)
- [ADR 0014: Crate and Module Split Policy](0014-crate-and-module-split-policy.md)
- [Issue #154: Add versioning and compatibility policy ADR](https://github.com/joshka/qwertty/issues/154)
