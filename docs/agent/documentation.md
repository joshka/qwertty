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

## Spec-Aware API Docs

Terminal APIs should not require readers to inspect the implementation to understand what a helper
does. When a public API encodes or interprets a terminal protocol item, document the relevant spec
context where the API appears:

- expand protocol abbreviations the first time they matter;
- link to stable protocol references when available;
- show the concrete bytes emitted or interpreted for at least one realistic input;
- explain coordinate systems, modes, side effects, and compatibility notes that affect callers;
- include a practical example using the public API path a caller should use, such as
  `CommandBuffer` for encode-only output.

Do not defer examples or protocol context to a final documentation pass. Each API slice should carry
enough user-facing documentation for reviewers and users to understand the impact of the items added
in that slice.
