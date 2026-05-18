# ADR 0013: Platform Support Policy

## Status

Accepted

## Context

qwertty now has a meaningful split between platform-neutral byte and parser types, Unix-only live
terminal ownership, and Tokio-backed async terminal ownership on Unix behind an optional feature.
That split is already visible in the implementation and user-facing references, but the repo does
not yet record it as a durable decision.

The long-term project goal explicitly calls out platform support policy as a boundary that should
not live only in chat history or scattered API notes. Future maintainers need one place to answer
questions like these:

- which surfaces are safe to describe as cross-platform today;
- which live terminal behaviors are Unix-only for now;
- why unsupported platforms return `Error::Unsupported` instead of pretending broader support;
- what evidence is required before qwertty widens a support claim.

Without that policy, later work can drift into accidental promises in docs, examples, CI badges, or
public APIs that the library does not yet prove.

## Decision

Adopt a Unix-first live terminal support policy.

The current support boundary is:

- command encoding, protocol value types, and parser surfaces that operate only on bytes in memory
  are platform-neutral when their behavior does not depend on a live terminal device;
- live terminal ownership through `Terminal`, `TerminalSession`, and `TokioTerminalSession` is
  currently a Unix implementation;
- the optional `tokio` feature adds async terminal ownership on Unix only and does not widen the
  live platform set by itself.

When a live terminal capability is not implemented on a platform, qwertty should keep the public
type surface where that surface remains honest and return `Error::Unsupported` at the operation
boundary instead of simulating or implying support.

Widen a platform support claim only when all of these are true for the claimed surface:

- the behavior is implemented for that platform;
- user-facing docs explain the platform-specific contract clearly;
- automated tests or other durable validation cover the claim at the right layer;
- examples, feature flags, and error behavior do not contradict the claim.

Do not widen support claims based only on type availability, build success, or an implementation
sketch. Support is a documented and validated behavior boundary.

The next implementation issue after this policy is
[Add platform support policy ADR](https://github.com/joshka/qwertty/issues/146).

## Consequences

- qwertty can document platform-neutral byte and parser types honestly without implying that live
  terminal ownership is equally portable.
- Unsupported platforms fail explicitly with `Error::Unsupported`, which keeps application behavior
  understandable and reviewable.
- Future platform work must arrive with docs and validation, not just code paths.
- Release planning gains a clearer rule for deciding whether a platform belongs in public support
  notes or only in future work.
- CI, examples, and docs need to stay aligned with the actual claimed platform set.

## Alternatives Considered

### Describe Support Only In API References

The current API references already mention Unix-only live support, but they do not record the rule
for widening support claims. That leaves future decisions to folklore and local interpretation.

### Hide Unsupported Platforms Behind Missing Types

Removing type availability per platform would make unsupported paths less explicit and would make it
harder for callers to write conditional behavior around one public API shape. Returning
`Error::Unsupported` keeps the contract visible.

### Claim Broader Support Once Code Compiles

Build success alone is not enough for terminal behavior. Live terminal ownership depends on OS
semantics, cleanup behavior, query routing, and validation. The project needs a higher evidentiary
bar than compilation.

## Reference Material

- [Architecture](../architecture.md)
- [Non-Functional Requirements](../non-functional-requirements.md)
- [Platform Support](../reference/platform-support.md)
- [Terminal Device Reference](../reference/terminal-device.md)
- [Terminal Session Reference](../reference/terminal-session.md)
- [Terminal Input Reference](../reference/terminal-input.md)
- [ADR 0011: Tokio Async Runtime Boundary](0011-tokio-async-runtime-boundary.md)
- [ADR 0012: Terminal Query Routing Boundary](0012-terminal-query-routing-boundary.md)
- [Issue #142: Add platform support reference page](https://github.com/joshka/qwertty/issues/142)
