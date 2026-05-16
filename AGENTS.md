# qwertty Agent Guide

This repository values small, reviewable changes that make the library easier to understand and
maintain.

## Working Agreements

- Use `jj` for source-control workflows.
- Keep changes focused on one coherent purpose.
- Prefer boring, explicit, idiomatic Rust APIs.
- Optimize for reader locality and low cognitive burden.
- Keep public documentation task-first and concise.
- Put durable design decisions in ADRs.
- Format Rust with `cargo +nightly fmt --all`.
- Run `just check` before handing off a change.

## Project Standards

Read the relevant guide before editing:

- [Standards](docs/standards.md)
- [Rust style](docs/agent/rust-style.md)
- [Documentation style](docs/agent/documentation.md)
- [Testing](docs/agent/testing.md)
- [Workflow](docs/agent/workflow.md)
- [GitHub artifacts](docs/agent/github-artifacts.md)
- [Writing style](docs/agent/writing-style.md)

## Shared Development Preferences

This repo carries a local copy of shared development guidance in [docs/development](docs/development/).
Use qwertty's local rules first. When local guidance is silent, use the shared guidance as a
fallback.

Entry points:

- [Reviewed agent rules](docs/development/snippets/agents/rules.md): compact reviewed rule pack.
- [Rule domains](docs/development/rules/README.md): targeted rule files for deeper loading.
- [Bootstrap guidance](docs/development/bootstrap-downstream.md): how to refresh and merge shared
  guidance.
- [Software Practices](https://www.joshka.net/practice/): rendered reference with deeper guide,
  rule, pattern, principle, mechanism, and tag context.

If a shared rule causes friction or seems wrong for most Rust or agent work, capture that feedback
for the upstream development-preferences/practice repo instead of only patching around it locally.

Do not add terminal implementation code without an issue or plan that names the user behavior,
scope, validation, and documentation layer for the change.
