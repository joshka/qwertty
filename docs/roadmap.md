# Roadmap

The project grows in reviewable slices.

> **Current state and handoff:** see `work/STATUS.md` (maintainer checkout). In short — 0.1.x is
> published on crates.io with releases automated via release-plz, `main` is current and CI-green,
> and Phase 5 (conformance, Windows, breadth) is executing as parallel lanes planned in
> `work/phase5/`. The `## Status` list below is the historical early-slice log; the
> [Forward Look](#forward-look-unpublished-work-in-progress) table is the milestone view.

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

This early slice log is historical; the [Forward Look](#forward-look-unpublished-work-in-progress)
below is the current milestone view.

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

> This table tracks the milestone-level rebuild, M0 through M9. Everything marked complete is on
> `main` and in the published 0.1.x releases. Update this table as milestones land; it is
> deliberately concise, not a slice-by-slice log.

| Milestone | Scope                                      | Status                               |
| --------- | ------------------------------------------ | ------------------------------------ |
| M0        | Device seam and lifecycle                  | Complete                             |
| M1        | Syntax layer and events                    | Complete                             |
| M2        | Query correlator                           | Complete                             |
| M3        | Capabilities and policy                    | Complete                             |
| M4        | Full input vocabulary                      | Complete (frozen for 0.1, ADR 0019)  |
| M5        | Protocol-family commands                   | Complete                             |
| M6        | Suspend, resume, handoff, signals          | Complete                             |
| M7        | Sequence database and capture              | Complete                             |
| —         | Release engineering and publish prep       | Complete                             |
| —         | Docs pass and reference restructure        | Complete                             |
| M8        | 0.1.0 publication gate                     | Complete (0.1.2 on crates.io)        |
| MW        | Windows console support                    | Complete (unreleased)                |
| M9        | Conformance runner and generated reference | In progress (Phase 5 lanes)          |

Milestone detail:

- **M0** — device seam, mode ledger, panic-safe restore handle, re-entrant session.
- **M1** — lossless syntax layer, fuzz targets, fixture corpus, semantic event layer.
- **M2** — sans-io query correlator, Tokio session integration, real-emulator verification.
- **M3** — capability probe bundle, the `Capabilities`/`Finding`/`Evidence` tri-state model, and the
  `Policy`/`PolicyGate` layer are complete.
- **M4** — kitty `CSI u` keys, mouse, focus, bracketed paste, resize; the vocabulary froze for 0.1
  (ADR 0019).
- **M5** — SGR/style, alt-screen/cursor, OSC, synchronized output, scroll regions.
- **M6** — complete: suspend/resume (`SIGTSTP`/`SIGCONT`), the `run_detached` `$EDITOR` handoff, and
  the optional `signals`/resize streams all landed.
- **M8** — complete: 0.1.0 published manually, the repository registered as a crates.io trusted
  publisher, and 0.1.1/0.1.2 released through release-plz via OIDC. Releases are the maintainer's
  act (merging the release-plz PR or dispatching the workflow); the process is documented in
  [`docs/development/release-engineering.md`](development/release-engineering.md).
- **MW** — complete (unreleased): a VT-based Windows console backend (Windows 10 1809+) behind the
  shared device/session/decoder surface — the sync and async sessions, an FM-A1-safe worker-thread
  readiness transport, panic-safe console-mode restore, console Ctrl signals, and `run_detached`, with
  win32-input-mode decode. `suspend`/`resize_stream` are typed `Unsupported` (no job control; resize
  is in-band). Decisions are in ADR 0021 (unsafe policy) and ADR 0022 (console support model). The
  remaining validation is interactive/IME coverage on the real Windows terminal matrix.
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
