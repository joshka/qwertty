# Documentation Style

Documentation is layered so readers meet complexity when they need it.

## Layers

- README: short promise, status, and next links.
- Guides: the smallest useful programs and workflows.
- Concepts: concise explanations of ownership, raw mode, output ordering, input, queries,
  capabilities, and policy.
- Reference: deep behavior details, protocol specifics, compatibility, and edge cases.
- Maintainer docs: release, conformance, fuzzing, performance, and protocol coverage.
- ADRs: durable decisions and rejected alternatives.

Public APIs should have Rustdoc that explains what a caller needs to use the API correctly.
User docs should be light and task-first; maintainer depth should not be front-loaded.
