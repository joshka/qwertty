# Bootstrap Downstream Guidance

Use this document when an agent is asked to bring shared development guidance into a downstream
repository.

The goal is a useful local agent map, not a verbatim replacement of the downstream repo's existing
instructions. Preserve local project rules, validation commands, architecture notes, and workflow
constraints. Add links to the copied shared guidance so future agents can load more context when a
task needs it.

## Source

- Canonical source repository: `https://github.com/joshka/practice`
- Canonical rendered reference: [Software Practices](https://www.joshka.net/practice/)
- Local copied guidance root: `docs/development/`

## Bootstrap Steps

1. Inspect the downstream repo's existing `AGENTS.md` and nearby project docs.
1. Refresh or install the copied shared guidance:

   ```bash
   python3 docs/development/update.py
   ```

   If this file is not present yet, copy `templates/downstream/` from the source repo or run the
   source generator:

   ```bash
   python3 scripts/generate_downstream_template.py --output /path/to/downstream-repo
   ```

1. Merge the shared guidance into the downstream `AGENTS.md` instead of replacing local content.
1. Keep `AGENTS.md` short. It should route agents to deeper files rather than becoming the full
   rule book.
1. Add or keep local validation commands, source-control rules, ownership boundaries, and project
   conventions.
1. Run the downstream repo's normal formatting, linting, and test checks.
1. Report what changed, what was preserved, and what validation ran.

## Recommended `AGENTS.md` Entry

Adapt this section to the downstream repo's voice:

```markdown
## Shared Development Preferences

This repo carries a local copy of shared development guidance in `docs/development/`.
Use this repo's local rules first. When local guidance is silent, use the shared guidance as a
fallback.

Entry points:

- `docs/development/snippets/agents/rules.md`: compact reviewed rule pack.
- `docs/development/rules/README.md`: rule domains for targeted loading.
- `docs/development/bootstrap-downstream.md`: how to refresh and merge the guidance.
- https://www.joshka.net/practice/: rendered reference with deeper guide, rule, pattern, principle,
  mechanism, and tag context.

If a shared rule causes friction or seems wrong for most Rust or agent work, capture that feedback
for the `development-preferences` repo instead of only patching around it locally.
```

## Merge Guidance

Prefer local specificity over shared defaults. For example, keep project-specific validation such as
`just check`, `cargo +nightly fmt --all`, fixture-update commands, or release gates.

Prefer shared guidance for general agent behavior, review handoffs, jj workflow, Rust
maintainability, documentation shape, and source-control hygiene when the downstream repo does not
already have a stronger local rule.

Do not copy every source guide into `AGENTS.md`. Link to the local compact rules and the public site
for deeper context.
