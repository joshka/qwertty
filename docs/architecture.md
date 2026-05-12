# Architecture

qwertty starts with a small crate and will split package boundaries only when the split improves
ownership, stability, dependency isolation, or audience clarity.

## Planned Layers

- `qwertty`: user-facing facade and practical entry points.
- Terminal device layer: opening the current terminal, raw mode, size, and IO boundaries.
- Protocol layer: runtime-neutral command, event, query, and syntax types.
- Session layer: terminal ownership, ordered output, cleanup, and explicit leave behavior.
- Testkit layer: deterministic tests for terminal behavior and protocol fixtures.

## Boundary Rule

A module becomes a crate only when it has an independent audience, dependency set, stability policy,
or ownership model. Tiny protocol surfaces should begin as modules or planned work.

## Design Rule

Public APIs are conservative until examples prove the shape. Durable choices about crate
boundaries, terminal ownership, parser architecture, query routing, policy, and release scope
belong in ADRs.
