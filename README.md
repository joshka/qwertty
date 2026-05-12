# qwertty

qwertty is a Rust library for building terminal applications that need explicit terminal ownership,
ordered output, input handling, and policy-aware terminal features.

The library is being developed in small public slices. The first slices establish project quality
standards, then add command encoding, terminal device access, session lifecycle management, input,
queries, and capability policy.

## Status

qwertty is not ready for application use yet. The current crate exists so CI, documentation, and
review standards are active before terminal behavior is added.

## Project Shape

- User-facing APIs should be practical before they are broad.
- Examples should show realistic terminal workflows.
- Public APIs should include Rustdoc that explains relevant errors, invariants, safety, policy, or
  protocol behavior.
- Maintainer details live under `docs/` instead of the first reading path.

## Contributing

Use `just check` to run the local gate. See [docs/workflow.md](docs/agent/workflow.md) for the
development workflow and [docs/roadmap.md](docs/roadmap.md) for the planned order of work.
