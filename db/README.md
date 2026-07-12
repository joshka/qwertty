# The sequence database

This directory is the qwertty control-sequence database: one hand-curated TOML file per protocol
family, plus a shared citation table and the fixtures that pin each sequence's bytes. It is the
public contract other tools generate from (documentation, conformance probes, the caniuse
dataset), and it is **not** a runtime lookup table.

This README is the ten-minute read. A new entry is writable from it alone.

## Layout

| Path                       | What it is                                                        |
|----------------------------|-------------------------------------------------------------------|
| `db/<family>.toml`         | The `[[sequence]]` entries for one protocol family                |
| `db/sources.toml`          | Doc keys mapped to full citations (title, url, retrieved)         |
| `db/results/<target>.toml` | Live-capture conformance seed: answered/silent/timeout per id     |
| `db/caniuse.md`            | Generated support matrix rendering the results above (see below)  |
| `../fixtures/<family>/`    | The `.seq` fixture files an entry's `fixtures` array points at    |

Families partition the way sources and reviewers do: `conpty`, `dec`, `ecma48-csi`, `ecma48-syntax`,
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
just qdb-validate                            # or: cargo run -p qdb -- validate
cargo run -p qdb -- generate docs            # write markdown reference to target/qdb-docs/
cargo run -p qdb -- generate --check docs    # fail if the generated docs would drift
cargo run -p qdb -- generate matrix          # write the caniuse support matrix to db/caniuse.md
cargo run -p qdb -- generate --check matrix  # fail if db/caniuse.md would drift
cargo run -p qdb -- generate                 # both docs and matrix (also generate --check)
```

`qdb validate` enforces, per entry: id format (`family.mnemonic`, lowercase), globally unique ids,
every `refs` key resolves in `sources.toml`, every `fixtures` file exists and its header
`direction=` agrees with the entry, `replay` is present and a valid class, any `responds` or
`superseded_by` target is an existing id, and `description` is non-empty. It also checks the
capture artifacts below: an `origin=capture:` fixture is terminal-to-host and has a matching
`db/captures/<target>/` log, and every `db/results/<target>.toml` row names an existing entry with
a valid status.

## The conformance runner (`qdb run`, `qdb capture`)

Both commands are one loop — the conformance runner driving a `Target` adapter
(`tools/qdb/src/targets.rs`, the Phase 2 target-interface design made concrete). The runner owns
policy: per-query deadlines, replay-class gating, late-reply attribution, an echoed-reply
fabrication guard, and a DA1/XTVERSION identity cross-check against what the adapter claims to be
driving. Adapters own only the byte transport. The PTY-hosted adapters (`tmux`: a detached pane;
`betamax`: a headless ghostty-vt under an on-the-fly tape) share one mechanic: a thin relay
(`qdb target-relay`) launched inside the target terminal, pumping bytes between the controlling
tty (opened through the qwertty library) and a FIFO pair the runner drives from outside.

For each query entry that has a `responds` link and an admitted replay class, the runner writes
the query's exact bytes (unescaped from the entry's own query fixture — the single source of
truth) and drains the reply with a per-query deadline. By default only `replay = "safe"` entries
run; `qdb run --allow-modal` / `--allow-destructive` opt the other classes in explicitly — they
are never probed blind (the DECSLPP incident rule).

```sh
just capture                              # capture both installed targets, skip-if-missing
cargo run -p qdb -- capture --target tmux            # capture one target
cargo run -p qdb -- capture --target betamax --entry osc.11.background_query   # one entry
cargo run -p qdb -- run --target tmux                # conformance pass: results seed only
```

**Capture mode is the same loop with recording on.** Reply-direction (`terminal-to-host`) entries
ship with quarantined syntax — the audited prototype fabricated replies, so a reply's bytes
re-enter only through a live capture. `qdb capture` writes three kinds of artifact, all
reviewable ([`db/captures/FORMAT.md`](captures/FORMAT.md) documents the sidecar schema):

| Path                                                   | What it is                                                        |
| ------------------------------------------------------ | ----------------------------------------------------------------- |
| `db/captures/<target>/<id>.json`                       | Per-entry sidecar: escaped raw reply, status, identity, timestamp |
| `fixtures/<family>/<name>_report_capture_<target>.seq` | The minted reply fixture, `origin=capture:` header                |
| `db/results/<target>.toml`                             | The conformance/caniuse seed: answered/silent/timeout per id      |

`qdb run` writes only the results seed — the conformance pass mints no trust artifacts.

A minted fixture's path is added to the reply entry's `fixtures` array (a scripted edit that
preserves formatting). The entry's quarantined `syntax` is **not** touched — deriving syntax from
captured bytes is review work a later slice does, citing the capture. **Silence is data**: a query
the target does not answer is recorded as a `timeout` result, not dropped. Entries whose probe
would be side-effecting or ill-defined to send blind (iTerm2 `RequestUpload`, `Button`) are skipped
honestly as `unprobeable` with a reason in the runner code. A reply that merely echoes the query
is recorded `echo_suspect` and never becomes a fixture — the fabrication guard.

Live runs need the target tools installed and are deliberately **not** in the `check` chain
(`just capture` skips cleanly when a tool is missing, like `just verify-emulators`). The pure
logic — the runner loop against a scripted fake target, sidecar/fixture/results rendering, the
TOML fixture-array edit — is unit-tested, so it is covered by `just check` without a live
terminal.

## Support matrix (`qdb generate matrix`)

`db/caniuse.md` is the "caniuse for terminals" view: a checked-in Markdown table rendering
`db/results/<target>.toml` against the database entries. It is generated, not hand-maintained —
**machines write support claims; humans write entries and citations** (design 05). Regenerate it
whenever the results files change:

```sh
cargo run -p qdb -- generate matrix          # write db/caniuse.md
cargo run -p qdb -- generate --check matrix  # fail if db/caniuse.md would drift
cargo run -p qdb -- generate                 # both docs and the matrix
```

`just qdb-generate-check` (part of `just check` and CI) runs the `--check` form: it regenerates the
matrix in memory and diffs it against the committed file, the same drift-detection pattern
`generate --check docs` uses for the reference pages.

**Shape**: rows are the queryable sequences — entries with a `responds` link that a capture can
actually probe (`replay = "safe"`, minus the harness's honestly-unprobeable set) — grouped by
family in file order. Columns are capture targets, sorted by name, each headed with its captured
version and timestamp. A cell reports what that target did when probed:

| Cell           | Meaning                                                             |
|----------------|---------------------------------------------------------------------|
| `answered (N)` | The target replied; `N` is the raw reply byte length.               |
| `silent`       | The target ran the probe and produced no reply before the deadline. |
| `timeout`      | Same wire event as `silent` (no bytes before the deadline).         |
| `unprobeable`  | The entry is a query but the harness will not send it blind.        |
| `—`            | No result: this target has not captured this entry.                 |

**Honesty about what this is not**: every capture behind this matrix today is unattended and
scripted, not an attended, interactive terminal session — but not every target is headless. tmux
is driven by `send-keys` and betamax (libghostty) by an on-the-fly tape, both without a display;
alacritty has no headless mode, so its capture briefly opens a real GUI window that closes itself
the moment the scripted probe pass ends (recorded as its adapter kind in the results file). The
matrix does not claim otherwise: an unlisted or dash-marked
target/entry pair means *no evidence*, never an assumed pass or fail. This is evidence from real
captures, not a hand-curated support claim; when the hand-curated prototype conflated "what the
world defines" with "what a terminal actually does," gap-report machinery had to police the
conflation after the fact. Rendering the matrix straight from `db/results/` avoids reintroducing
that gap.
