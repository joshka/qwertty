# ADR 0016: Dependency Policy

## Status

Accepted

## Context

qwertty is still a small pre-release library with a narrow dependency set, one optional runtime
integration, and a Unix-first live terminal boundary. That small shape is part of the product: it
keeps command encoding, terminal ownership, parser behavior, and release planning easy to review
locally.

At the same time, future work will keep creating pressure to add dependencies. Runtime adapters,
protocol helpers, integration examples, release tooling, and test support all create places where a
new crate can look cheaper than a local implementation. Dependency bumps carry similar risk. They
can change platform reach, public behavior, feature contracts, or downstream integration shape even
when source changes in qwertty itself are small.

The repo already has durable policies for runtime shape, platform support, package boundaries, and
pre-release compatibility. It does not yet have one place that answers questions like these:

- when a new dependency is justified instead of a local implementation;
- when a dependency should be optional behind a feature flag;
- how wide version requirements should be for public Rust dependencies;
- what review bar applies to dependency upgrades that may change public behavior.

Without that policy, dependency decisions can drift into ad hoc convenience choices that increase
maintenance cost faster than the product proves the need.

## Decision

Keep qwertty's dependency surface intentionally small and add new dependencies only when they buy a
clear product or maintenance advantage.

The current policy is:

- prefer local implementation when the behavior is small, product-specific, and easier to review
  than another public dependency;
- add a dependency when it provides proven domain behavior, platform support, safety, or validation
  that qwertty should not re-create locally;
- make a dependency optional when it exists to support a distinct public integration surface, such
  as a runtime-specific owner, and keep that feature disabled by default unless the integration is
  part of the base contract;
- choose the widest honest semver-compatible version requirement that preserves qwertty's intended
  public behavior and downstream integration shape;
- treat dependency additions, removals, feature-flag changes, and version bumps that may affect
  public behavior, platform claims, or release risk as compatibility-relevant changes that need
  explicit issue or PR reasoning;
- keep maintenance-only dependency updates separate from changes that alter public behavior,
  feature contracts, platform reach, or dependency policy itself;
- do not widen the default dependency footprint, platform matrix, or release claims incidentally as
  a side effect of adding one convenience library.

In practice, this means qwertty should keep core command, parser, and session behavior dependent on
as few external crates as practical, while still using specialized upstream crates when they are
the honest low-risk owner of platform or runtime behavior.

The next implementation issue after this policy is
[Add dependency policy ADR](https://github.com/joshka/qwertty/issues/158).

## Consequences

- Dependency additions need product-level justification instead of being treated as harmless local
  convenience.
- Optional integrations, such as Tokio today, remain explicit at the public feature boundary.
- Version requirements and upgrade decisions stay connected to compatibility and release planning.
- Core library surfaces can stay easier to audit, test, and document because dependency growth is
  deliberate.

## Alternatives Considered

### Prefer Dependencies Whenever They Reduce Local Code

That would often lower short-term implementation cost, but it would also raise review, release,
platform, and compatibility cost before qwertty has proved that broader surface area is worth it.

### Avoid New Dependencies Almost Entirely

That would keep the manifest small, but it would force qwertty to re-create upstream runtime or
platform behavior that established crates already own better.

### Keep Dependency Rules Only In Development Guidance

The development rules are useful implementation advice, but dependency policy also shapes the
public product and release surface. It needs a durable ADR that future maintainers can cite
directly.

## Reference Material

- [Cargo Manifest](../../Cargo.toml)
- [Architecture](../architecture.md)
- [Roadmap](../roadmap.md)
- [Non-Functional Requirements](../non-functional-requirements.md)
- [ADR 0011: Tokio Async Runtime Boundary](0011-tokio-async-runtime-boundary.md)
- [ADR 0014: Crate and Module Split Policy](0014-crate-and-module-split-policy.md)
- [ADR 0015: Versioning and Compatibility Policy](0015-versioning-and-compatibility-policy.md)
- [Issue #158: Add dependency policy ADR](https://github.com/joshka/qwertty/issues/158)
