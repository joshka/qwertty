# Workflow

Use `jj` for source control and keep each change focused.

## Local Gate

```sh
just check
```

The gate runs Cargo metadata, formatting, tests, clippy, docs, and Markdown linting.

## Change Shape

- Start separable work in a fresh `jj` change.
- Set the change description early.
- Keep work atomic and reviewable.
- Prefer small follow-up changes over overloaded commits.
- Use bookmarks with the `joshka/` prefix for personal lines.
- Use `--no-pager` with `jj` commands that produce output.

## Review Handoff

State files changed, validation run, acceptance status, and residual gaps.
