# ADR 0014: Crate and Module Split Policy

## Status

Accepted

## Context

qwertty currently ships as one crate with internal modules that separate command encoding, live
terminal ownership, async session behavior, input decoding, and user-facing documentation. That
shape is still small enough to understand locally, and most current work shares one product story:
an async-first terminal library that owns the session boundary well.

At the same time, future work will add pressure to split package boundaries. Protocol helpers,
runtime-specific integration, test fixtures, and support policy all create natural places where a
crate split can look attractive before the project has proved that consumers, dependencies, or
ownership boundaries are actually different.

The architecture guide already says that qwertty should start small and split only when the split
improves ownership, stability, dependency isolation, or audience clarity. That rule should not
live only in overview prose. The long-term project goal explicitly calls out crate and module split
decisions as durable boundaries that future maintainers should be able to find without chat
history.

Without an ADR, later work can drift into crate splits that organize the tree but do not make the
product easier to use, validate, version, or maintain.

## Decision

Keep new functionality inside the `qwertty` crate by default.

A module should become a crate only when the split is justified by concrete evidence in one or more
of these dimensions:

- it serves an independent audience that could reasonably depend on that surface without adopting
  the rest of qwertty;
- it needs a meaningfully different dependency set that should not flow into the main crate by
  default;
- it needs its own stability policy, release cadence, or compatibility story;
- it has a different ownership boundary that would make review, maintenance, or validation clearer
  as a separate package.

Do not split a crate for naming neatness, directory organization, speculative reuse, or because a
layer sounds architectural on paper. Small protocol families, helper types, and runtime-specific
implementation details should begin as modules until they prove a stronger package boundary.

When a crate split is proposed, the supporting issue or ADR should explain:

- who the separate audience is, if any;
- what dependency or feature isolation the split creates;
- what public stability or release boundary changes with the split;
- what validation and documentation surface moves with the new package;
- why a module inside `qwertty` is no longer the better local choice.

The next implementation issue after this policy is
[Add crate and module split policy ADR](https://github.com/joshka/qwertty/issues/150).

## Consequences

- qwertty keeps one clear package boundary until a different shape is proven by users,
  dependencies, or ownership needs.
- Future protocol, runtime, and test support can grow as modules first, which keeps naming and
  versioning decisions reversible while behavior is still being proven.
- Crate splits need explicit justification in project planning instead of happening as local code
  organization preferences.
- Docs, examples, CI, and release notes can stay centered on one main crate until a separate
  package has its own real audience and contract.

## Alternatives Considered

### Split Layers Into Crates As Soon As They Exist

That would make the architecture diagram look more explicit, but it would freeze package names,
dependency edges, and public boundaries before qwertty has proved that users need those packages
separately.

### Keep The Rule Only In Architecture Overview Docs

The architecture guide is a good summary, but it is not a durable decision record. Later package
boundary debates need one ADR that states the policy and the evidentiary bar directly.

### Prefer Crate Splits For Reuse Potential

Speculative reuse is not enough. A new crate adds versioning, docs, CI, release, and compatibility
overhead. qwertty should pay that cost only when the reuse story is concrete and externally useful.

## Reference Material

- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Platform Support](../reference/platform-support.md)
- [ADR 0001: Async-First Terminal I/O Boundary](0001-async-first-terminal-io-boundary.md)
- [ADR 0011: Tokio Async Runtime Boundary](0011-tokio-async-runtime-boundary.md)
- [ADR 0012: Terminal Query Routing Boundary](0012-terminal-query-routing-boundary.md)
- [ADR 0013: Platform Support Policy](0013-platform-support-policy.md)
- [Issue #150: Add crate and module split policy ADR](https://github.com/joshka/qwertty/issues/150)
