# Fixture file format (`.seq`)

This directory holds decoder test fixtures: single, well-formed terminal control sequences stored
as escaped text. Each fixture is one `.seq` file. This document is the normative specification of
that format so third parties can load fixtures standalone.

## File layout

A `.seq` file is:

1. A **header line** (see below), ending in a single `LF` (`0x0a`).
2. The **escaped payload**: the sequence itself, in the escaped-text encoding below.
3. A single **trailing `LF`** (`0x0a`) that is a file-format artifact, **not** sequence payload.

To recover the sequence bytes: read the file, drop everything up to and including the first `LF`
(the header), strip exactly one trailing `LF`, then unescape the remainder.

The trailing `LF` is always present and always removed. It is never part of the sequence. A fixture
whose sequence genuinely ends in `LF` encodes that byte explicitly as `\x0a` in the payload; the
file still adds its own trailing `LF` on top.

## Header line

Every file begins with exactly this shape:

```text
#! direction=host-to-terminal origin=<origin>
```

- `direction` is `host-to-terminal` for a sequence a host sends **to** a terminal (commands and
  queries) or `terminal-to-host` for a sequence a terminal sends **to** a host (input reports and
  replies). Most fixtures are host-to-terminal; terminal-to-host fixtures are the live-capture
  replies (`origin=capture:`) and spec-derived input reports (`origin=spec:`).
- `origin` records provenance. Fixtures imported from the audited reference prototype use
  `origin=prototype:audited-2026-07-06`. Other durable origins are `spec:<key>` (derived from a
  specification) and `capture:<terminal-version>` (recorded from a live terminal).

## Escaped-text encoding

The payload is ASCII text. A backslash introduces an escape; every other byte is literal:

| Input     | Decodes to                                               |
|-----------|----------------------------------------------------------|
| `\e`      | `ESC` (`0x1b`)                                           |
| `\xNN`    | the single byte with hexadecimal value `NN` (two digits) |
| `\\`      | a single backslash (`0x5c`)                              |
| any other | copied literally as its ASCII byte                       |

Because `\\` decodes first, a literal backslash followed by other text (for example VS Code's
`\x20` command-line escaping, written `\\x20` in the payload) round-trips as the literal bytes
`\`, `x`, `2`, `0` rather than being read as a `\xNN` escape. Only a backslash that is itself
unescaped starts an `\e`, `\xNN`, or `\\` escape.

## Reference unescape routine

This Rust function unescapes a payload (the bytes after the header line, with the trailing `LF`
already stripped). It mirrors the table above exactly.

```rust
fn unescape(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'\\' && i + 1 < input.len() {
            match input[i + 1] {
                b'e' => { out.push(0x1b); i += 2; }
                b'\\' => { out.push(b'\\'); i += 2; }
                b'x' if i + 3 < input.len() => {
                    let hi = (input[i + 2] as char).to_digit(16);
                    let lo = (input[i + 3] as char).to_digit(16);
                    match (hi, lo) {
                        (Some(hi), Some(lo)) => { out.push((hi * 16 + lo) as u8); i += 4; }
                        _ => { out.push(input[i]); i += 1; }
                    }
                }
                _ => { out.push(input[i]); i += 1; }
            }
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    out
}
```
