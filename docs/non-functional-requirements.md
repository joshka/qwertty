# Non-Functional Requirements

The project sets quality requirements before expanding terminal behavior.

## Baseline

- CI must pass on every pull request.
- Rustfmt and clippy warnings are enforced.
- Rustdoc warnings are denied.
- Markdown is linted.
- Public examples compile when examples exist, and public API slices should include examples unless
  the issue explains why an example would be misleading.
- Tests cover advertised behavior.
- Protocol-facing APIs document relevant terms, references, bytes, side effects, and examples in
  the same slice that introduces the API.
- Unsafe code is absent unless an accepted ADR allows a narrow exception.
- Platform-specific APIs are documented clearly.
- Side-effecting terminal commands are policy-gated.
- Terminal cleanup errors are observable where the platform allows it.

## Future Requirements

- Performance-sensitive paths should identify their expected cost model when introduced and gain
  benchmarks once behavior stabilizes.
- Parser, query routing, and policy code should add fuzz or property-style tests when those
  behaviors become public enough for regressions to matter.
- Protocol claims should be backed by fixtures or conformance evidence.
