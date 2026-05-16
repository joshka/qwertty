# Architecture

qwertty starts with a small crate and will split package boundaries only when the split improves
ownership, stability, dependency isolation, or audience clarity.

## Planned Layers

- `qwertty`: user-facing facade and practical entry points.
- Terminal device layer: opening the current terminal, raw mode, size, and IO boundaries.
- Protocol layer: runtime-neutral command, event, query, and syntax types.
- Session layer: terminal ownership, ordered output, explicit flushing, cleanup, and explicit leave
  behavior.
- Testkit layer: deterministic tests for terminal behavior and protocol fixtures.

## Boundary Rule

A module becomes a crate only when it has an independent audience, dependency set, stability policy,
or ownership model. Tiny protocol surfaces should begin as modules or planned work.

## Layer Boundary

The terminal device layer should stay below the session layer. It owns the live terminal handle,
raw/cooked mode transition, terminal size lookup, and byte-oriented write/flush boundary.

It should not own application lifecycle policy yet. Session setup, alternate screen, ordered frame
cleanup, feature cleanup, input parsing, query routing, and async event loops belong to later
slices unless the implementation issue records a narrower reason to move one of those boundaries.

The first session layer owns raw-mode entry, ordered output writes, explicit flushing, and explicit
leave cleanup. It does not yet own input parsing, query routing, alternate screen policy, feature
cleanup, or async runtime integration.

## Design Rule

Public APIs are conservative until examples prove the shape. Durable choices about crate
boundaries, terminal ownership, parser architecture, query routing, policy, and release scope
belong in ADRs.
