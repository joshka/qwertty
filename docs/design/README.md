# Phase 2 design set

This directory is qwertty's approved phase-2 architecture design set — the documents that
gated implementation — plus the phase-3 prototype-salvage plan. Each design doc covers one
area (ownership, decoding, queries, async, the sequence database, capabilities/policy,
platform, API boundaries, width, graphics) with the alternatives considered and the winning
rationale; two accounting documents map every ADR and failure-mode catalog entry to its
disposition.

**Provenance:** drafted 2026-07-06 as the phase-2 design gate (with the 09-width and
11-graphics checkpoints added 2026-07-12), reviewed and signed off by the maintainer before
Phase 3 (salvage audit + build) began. Two documents from that phase — `10-windows.md` (the
Windows implementation plan) and the phase-4 review memos — remain maintainer-local working
records and are not included here.

## Index

- [00-overview.md](00-overview.md) — the design set's map, cross-cutting invariants, and the
  Phase 2 gate record.
- [01-ownership.md](01-ownership.md) — terminal ownership, lifecycle, and restore-on-exit.
- [02-decoder.md](02-decoder.md) — the input decoder architecture (syntax + semantic layers).
- [03-queries.md](03-queries.md) — query/reply correlation and the race-freedom proof plan.
- [04-async.md](04-async.md) — the async strategy (sans-io core, Tokio session, blocking driver).
- [05-database.md](05-database.md) — the sequence database schema and generation pipeline.
- [conformance-target-interface.md](conformance-target-interface.md) — the conformance
  runner's target trait (one-pager).
- [06-capabilities-policy.md](06-capabilities-policy.md) — capability detection and the
  security policy layer.
- [07-platform.md](07-platform.md) — the Unix/Windows/BSD platform strategy.
- [08-api-boundaries.md](08-api-boundaries.md) — crate/module boundaries and the public API
  shape.
- [09-width.md](09-width.md) — width measurement design and spike (decided).
- [11-graphics.md](11-graphics.md) — inline image protocols (graphics) design.
- [adr-dispositions.md](adr-dispositions.md) — disposition of ADR 0001–0018 against this set.
- [fm-dispositions.md](fm-dispositions.md) — every failure-mode catalog entry mapped to its
  addressing mechanism or declared limit.
- [prototype-salvage.md](prototype-salvage.md) — the phase-3 audit of what ports as-is, what
  ports with rework, and what gets discarded from the prior codebases.
