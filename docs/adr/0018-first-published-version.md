# ADR 0018: First Published Version

## Status

Accepted

## Context

qwertty now has a clearer publication story than it did when the project began. It has a defined
first release target, a Unix-first platform boundary, a pre-release compatibility policy, a
release-readiness reference page, and a checked-in release checklist that describes what
maintainers should validate before changing `publish = false`.

What is still missing is the actual version target for that first publication. The manifest still
uses `0.0.0` and `publish = false`, which correctly say "not published yet" but do not answer what
the first public release should be called.

That choice affects release planning directly:

- whether the first publication is framed as the initial public contract for qwertty or as a later
  maturity step;
- how maintainers describe the current API and support posture to downstream users;
- whether release work treats the first publication as the beginning of the public line or as a
  follow-on after an already-implied earlier release.

The repo now documents one coherent first-release product: a Unix-first terminal library with a
runtime-neutral synchronous core, an optional Tokio async session surface, checked query-routing
contracts, checked-in examples, and docs.rs-facing reference material. That is enough to justify a
real first public version, but it is still intentionally narrow and explicitly pre-1.0.

## Decision

Use `0.1.0` as qwertty's intended first published version.

This version means:

- the first publication is the first real public contract for the crate, not a later milestone
  after some implied earlier release;
- the crate remains clearly pre-1.0, so maintainers can still evolve APIs intentionally under the
  existing compatibility policy;
- the published version matches the current release target: a coherent Unix-first terminal library
  with both synchronous ownership and the optional Tokio async session surface, not a broader or
  more mature claim than the docs and validation can support today.

Release work should keep `0.2.0` available for a later stage that materially widens or deepens the
public product, such as broader platform reach, a larger protocol surface, or a more mature
integration story.

This ADR records the intended version only. The later publishing slice still owns the actual
manifest change, release notes, and publication execution.

The next implementation issue after this policy is
[Add first published version ADR](https://github.com/joshka/qwertty/issues/174).

## Consequences

- The first publication can be described plainly as qwertty's initial external release.
- The repo does not imply a second-step maturity marker before the first public artifact exists.
- Release planning can distinguish between "first publication" work and later post-`0.1` product
  expansion work.
- The versioning story remains conservative: real public contract, but still pre-1.0 and allowed
  to change intentionally under the current compatibility policy.

## Alternatives Considered

### Use `0.2.0` For The First Publication

That would suggest qwertty had already passed through an earlier public release stage, or that the
current product surface is materially broader or more mature than it is. The repo does not have
that history, and the current Unix-first scope is better described as a strong `0.1` than as an
implied second pre-1.0 milestone.

### Keep `0.0.0` Until A Much Broader Release Exists

That would postpone the version decision, but the repo now has enough durable policy and user
documentation to define a first public contract honestly. Keeping the first published version
undefined would leave release work without a concrete target even after the release target and
checklist already exist.

### Wait To Decide The Version Only In The Publishing PR

That would keep the decision closer to the actual manifest change, but it would also force release
execution work to carry a basic policy decision that should already be settled. The version target
belongs in a durable planning artifact first.

## Reference Material

- [Cargo Manifest](../../Cargo.toml)
- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Release Checklist](../reference/release-checklist.md)
- [Release Readiness](../reference/release-readiness.md)
- [ADR 0015: Versioning and Compatibility Policy](0015-versioning-and-compatibility-policy.md)
- [ADR 0017: First Release Target](0017-first-release-target.md)
- [Issue #174: Add first published version ADR](https://github.com/joshka/qwertty/issues/174)
