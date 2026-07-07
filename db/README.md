# The sequence database

This directory is the qwertty control-sequence database: one hand-curated TOML file per protocol
family, plus a shared citation table and the fixtures that pin each sequence's bytes. It is the
public contract other tools generate from (documentation, conformance probes, the caniuse
dataset), and it is **not** a runtime lookup table.

This README is the ten-minute read. A new entry is writable from it alone.

## Layout

| Path                    | What it is                                                     |
| ----------------------- | -------------------------------------------------------------- |
| `db/<family>.toml`      | The `[[sequence]]` entries for one protocol family             |
| `db/sources.toml`       | Doc keys mapped to full citations (title, url, retrieved)      |
| `../fixtures/<family>/` | The `.seq` fixture files an entry's `fixtures` array points at |

Families partition the way sources and reviewers do: `dec`, `ecma48-csi`, `ecma48-syntax`,
`iterm2`, `kitty-color`, `kitty-graphics`, `kitty-keyboard`, `kitty-misc`, `kitty-multicursor`,
`kitty-pointer`, `osc`, `vendor-dcs`, `xterm-capabilities`, `xterm-input`, `xterm-modes`,
`xterm-session`.

## Entry schema — the whole thing

```toml
[[sequence]]
id          = "csi.cup"                # stable, namespaced, never reused or renamed
name        = "Cursor Position (CUP)"
description = "Moves the cursor to the given row and column."   # one plain-English sentence
direction   = "host-to-terminal"       # | "terminal-to-host" | "bidirectional"
syntax      = "CSI Pr ; Pc H"          # canonical ECMA-48 notation; omitted for quarantined replies
params      = [{ name = "Pr", kind = "number", default = 1 }]  # optional, where meaningful
refs        = [{ doc = "ecma48", section = "8.3.21" }]         # keys resolve in sources.toml
fixtures    = ["fixtures/ecma48/csi_cup.seq"]                  # paths relative to the repo root
replay      = "safe"                    # | "modal" | "destructive"
responds    = "csi.cpr"                 # optional: id of the reply, if this is a query
notes       = "1-based; clamps at margins."                    # optional free-form
superseded_by = "csi.newer"            # optional: set instead of deleting a wrong id
```

- **`id`** is `family.mnemonic`, lowercase, dot-separated, stable forever. Corrections change
  fields, never ids; a wrong id is deprecated with `superseded_by`, not deleted.
- **`description`** is one sentence a reader with no VT background can follow. The generated
  reference leads with it.
- **`refs`** resolve against `sources.toml`; every entry needs at least one. Report-direction
  entries cite the spec they were documented from.
- **`fixtures`** point at existing `../fixtures/<family>/<name>.seq` files by repo-root-relative
  path. The files are not duplicated here; `qdb validate` checks they exist.
- **Deliberately absent**: `confidence` (an entry's trustworthiness is its refs plus fixtures),
  support tiers (those live in conformance results), and Rust cross-links (generation flows
  db to code, never the other way).

## Terminal-to-host (reply) entries

Reply-direction sequences are documented as entries, but their `syntax` is quarantined: the
audited prototype fabricated several replies by echoing the query, so a reply's byte form re-enters
only through live capture. These entries carry `notes = "reply syntax pending live capture"`, omit
`syntax`, and have no fixtures (report fixtures import only with an `origin=capture:` header).

## The replay rubric

Every entry declares how safe it is to replay blind against a live terminal:

| Class         | Meaning                                            | Examples                                        |
| ------------- | -------------------------------------------------- | ----------------------------------------------- |
| `safe`        | Pure output or a query; no lasting state change    | SGR, titles, hyperlinks, cursor move, reports   |
| `modal`       | Changes a terminal mode; reversible by its inverse | alt-screen, mouse/paste toggles, mode set/reset |
| `destructive` | Irreversible or resizes the real terminal          | RIS full reset, DECSLPP set-lines-per-page      |

Judgment calls follow the audit: alt-screen and any mode set/reset is `modal`; a report emits
nothing that changes state, so it is `safe`; the DECSLPP class (which resizes a real xterm) is
`destructive` and never a conformance probe.

## Running `qdb`

`qdb` is the unpublished workspace tool (`tools/qdb`) that operates on this directory.

```sh
just qdb-validate            # or: cargo run -p qdb -- validate
cargo run -p qdb -- generate docs          # write markdown reference to target/qdb-docs/
cargo run -p qdb -- generate --check docs  # fail if the generated docs would drift
```

`qdb validate` enforces, per entry: id format (`family.mnemonic`, lowercase), globally unique ids,
every `refs` key resolves in `sources.toml`, every `fixtures` file exists and its header
`direction=` agrees with the entry, `replay` is present and a valid class, any `responds` or
`superseded_by` target is an existing id, and `description` is non-empty.
