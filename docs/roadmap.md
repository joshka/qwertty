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
- Terminal query routing boundary decision is on `main`.
- Internal Tokio query routing state is on `main`.
- The next concrete slice is:
  [Add crate and module split policy ADR](https://github.com/joshka/qwertty/issues/150).

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
