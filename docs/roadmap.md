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
- Cursor position query response matching is the current implementation slice:
  [Add cursor position query response matching](https://github.com/joshka/qwertty/issues/45).

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
