# Roadmap

The project grows in reviewable slices.

## Status

- Project standards and scaffold are on `main`.
- The minimal encode-only library is on `main`.
- The terminal device layer is on `main`.
- Session lifecycle is on `main`.
- Input byte events is on `main`.
- Basic input events is on `main`.
- UTF-8 input text decoding is on `main`.
- Basic Escape input parsing is on `main`.
- Stateful input decoding is on `main`.
- CSI input sequence values are on `main`.
- Cursor position query report parsing is on `main`.
- Cursor position query response matching is on `main`.
- Async runtime boundary decision is on `main`.
- Feature-gated Tokio terminal session owner is on `main`.
- Tokio cursor position query routing is on `main`.
- Tokio terminal status query routing is on `main`.
- Tokio terminal ownership example is on `main`.
- Tokio input event example is on `main`.
- Tokio late query reply tests are on `main`.
- Tokio wrong-report query tests are on `main`.
- Tokio unmatched query report tests are on `main`.
- Tokio redirected terminal query tests are on `main`.
- Tokio query error-handling example is on `main`.
- Tokio query cancellation example is on `main`.
- Tokio late query reply example is on `main`.
- Tokio wrong-report query example is on `main`.
- Tokio unmatched query-shaped input example is on `main`.
- Tokio preserved unrelated input example is on `main`.
- Tokio terminal-status preserved input example is on `main`.
- Tokio terminal-status wrong-report example is on `main`.
- Tokio terminal-status unmatched query-shaped input example is on `main`.
- Tokio terminal-status cancellation example is on `main`.
- Checked-in examples reference page is on `main`.
- Platform support reference page is on `main`.
- Platform support policy ADR is on `main`.
- Crate and module split policy ADR is on `main`.
- Versioning and compatibility policy ADR is on `main`.
- Dependency policy ADR is on `main`.
- Release-readiness reference page is on `main`.
- First release target ADR is on `main`.
- Release checklist reference page is on `main`.
- First published version ADR is on `main`.
- Terminal query routing boundary decision is on `main`.
- Internal Tokio query routing state is on `main`.
- The next concrete slice is:
  [Add release-blocking examples reference page](https://github.com/joshka/qwertty/issues/178).

## Slices

1. Project standards and scaffold.
1. Minimal encode-only library.
1. Terminal device layer.
1. Session lifecycle.
1. Input and queries.
1. Capabilities and policy.
1. Protocol families.
1. Vendor protocol support.
1. Registry and conformance tooling.
1. Integrations and release polish.

Each slice should be understandable on its own, with issue scope, acceptance criteria, tests, and
documentation in the right layer.

## Forward Look (Unpublished Work In Progress)

> The sections above reflect only published `main`-branch state. This section tracks the larger
> in-progress rebuild happening in reviewed jj workspaces ahead of `main` — milestones M0 through
> M9 — so the plan stays visible even where `main` has not caught up yet. Update this table as
> milestones seal; it is deliberately concise, not a slice-by-slice log.

| Milestone | Scope                                      | Status                              |
| --------- | ------------------------------------------ | ----------------------------------- |
| M0        | Device seam and lifecycle                  | Complete                            |
| M1        | Syntax layer and events                    | Complete                            |
| M2        | Query correlator                           | Complete                            |
| M3        | Capabilities and policy                    | Nearly complete                     |
| M4        | Full input vocabulary                      | Complete (frozen for 0.1, ADR 0019) |
| M5        | Protocol-family commands                   | Complete                            |
| M6        | Suspend, resume, handoff                   | In progress                         |
| —         | Docs pass                                  | In progress (this slice)            |
| M7        | Sequence database and capture              | Complete                            |
| M8        | 0.1.0 publication gate                     | Not started                         |
| M9        | Conformance runner and generated reference | Not started                         |

Milestone detail:

- **M0** — device seam, mode ledger, panic-safe restore handle, re-entrant session.
- **M1** — lossless syntax layer, fuzz targets, fixture corpus, semantic event layer.
- **M2** — sans-io query correlator, Tokio session integration, real-emulator verification.
- **M3** — capability probe bundle and the `Capabilities`/`Finding`/`Evidence` model are done;
  the policy skeleton is in flight.
- **M4** — kitty `CSI u` keys, mouse, focus, bracketed paste, resize; the vocabulary froze for 0.1
  (ADR 0019).
- **M5** — SGR/style, alt-screen/cursor, OSC, synchronized output, scroll regions.
- **M6** — suspend/resume (`SIGTSTP`/`SIGCONT`) is in a review workspace (M6-S1); handoff
  (`run_detached`, M6-S2) and the optional signals stream (M6-S3) have not started.
- **M8** — 0.1.0 publication gate: semver review, README/docs.rs polish, release-checklist rerun,
  version bump, `publish = false` removal. Blocked on M6 and the docs pass; push, PR, and publish
  are the maintainer's own act.
- **M9** — generalizes the M7 capture harness into a full conformance runner (target-trait shape),
  produces a results-driven support matrix and a generated docs.rs reference, and adds
  width-behavior probes.

### Design-owed items (gate-mandated, before their code lands)

- **Width measurement mechanism** — a design doc plus a spike measuring how far real terminals
  deviate from static Unicode-width tables, keyed by terminal identity and mode 2027
  (grapheme-clustering) state, feeding the M9 conformance runner.
- **Inline-insertion recipe** (R-OUT-6) — once the M9 runner produces per-terminal scroll-region
  semantics data, write the recipe doc and wire the derived `inline_insertion_safe` capability
  result.
- **Suspend/handoff test harvest** — M6 needs new tests written from scratch; there is no prior
  suspend/resume test suite to port from either evidence line.
