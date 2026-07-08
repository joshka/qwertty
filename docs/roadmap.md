# Roadmap

The project grows in reviewable slices.

> **Current state and handoff:** see [`work/STATUS.md`](../work/STATUS.md). In short — 0.1.0 is staged
> and gate-green in local jj, but nothing is pushed yet, so the `## Status` list below reflects the
> published `main` branch, which is far behind the local work tracked in
> [Forward Look](#forward-look-unpublished-work-in-progress).

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

The local work has advanced well past this published list; the [Forward Look](#forward-look-unpublished-work-in-progress)
below is the current milestone view, and [`work/STATUS.md`](../work/STATUS.md) is the live entry point.

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
| M8        | 0.1.0 publication gate                     | Staged; publish pending (maintainer) |
| M9        | Conformance runner and generated reference | Not started                          |

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
- **M8** — 0.1.0 publication gate: semver review, docs pass, release-checklist rerun, version bump,
  and `publish = false` removal are all done, and the release-engineering standard is in place. The
  version is **staged** and gate-green in local jj; the remaining `git push`, first `cargo publish`,
  and trusted-publisher setup are the maintainer's own manual act. See
  [`work/STATUS.md`](../work/STATUS.md) for the publish sequence.
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
