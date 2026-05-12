# Testing

Tests should prove advertised behavior without making future refactors unnecessarily expensive.

## Baseline

- Unit tests cover local logic.
- Integration tests cover public behavior across module boundaries.
- Examples compile when examples exist.
- Terminal behavior needs deterministic fixtures or testkit support before claims are broadened.
- Bug fixes should include a regression test unless the issue explains why that is impractical.

## Validation

Run `just check` before handoff. If a command is not applicable, state why in the handoff notes.
