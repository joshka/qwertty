# Non-Functional Requirements

The project sets quality requirements before expanding terminal behavior.

## Baseline

- CI must pass on every pull request.
- Rustfmt and clippy warnings are enforced.
- Rustdoc warnings are denied.
- Markdown is linted.
- Public examples compile when examples exist.
- Tests cover advertised behavior.
- Unsafe code is absent unless an accepted ADR allows a narrow exception.
- Platform-specific APIs are documented clearly.
- Side-effecting terminal commands are policy-gated.
- Terminal cleanup errors are observable where the platform allows it.

## Future Requirements

- Performance-sensitive paths should gain benchmarks once behavior stabilizes.
- Parser, query routing, and policy code should gain fuzz or property-style tests when mature.
- Protocol claims should be backed by fixtures or conformance evidence.
