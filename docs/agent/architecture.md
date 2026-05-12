# Agent Architecture Guidance

Use architecture work to reduce uncertainty for the next reviewable slice.

## Expectations

- Name the user behavior or maintainer problem first.
- Keep package and module boundaries feature-oriented.
- Prefer named module files; avoid `mod.rs` unless the directory shape needs it.
- Put the central item first, then helpers.
- Keep future protocol depth out of onboarding docs.
- Record durable decisions in ADRs.

## Out Of Scope

Do not add broad crate splits, registry machinery, or vendor protocol surfaces before their issue
explains the concrete behavior and review boundary.
