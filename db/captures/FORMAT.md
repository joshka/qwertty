# Capture sidecar format (`db/captures/<target>/<id>.json`)

The per-entry evidence record the conformance runner writes when capture mode (recording) is on.
One JSON file per probed query entry, per target — answered *and* silent, because silence is data
the results seed must be able to cite. This is the capture-format handshake promised to ghostty-rs
(collab ask 4): the schema below is explicit and stable; fields are only added, never renamed or
removed.

A note on shape: the Phase 2 one-pager sketched "raw bytes + a TOML sidecar". What shipped in
M7-S2 and is documented here is a JSON sidecar carrying the reply bytes in the fixture escape
encoding (`fixtures/FORMAT.md`: `\e`, `\xNN`, `\\`) — lossless, so the raw bytes are one ~20-line
unescape away, and identical to the encoding the minted `.seq` fixture uses. One encoding, two
artifacts, no second opinion. The raw-bytes artifact proper is the minted fixture itself
(`fixtures/<family>/<name>_report_capture_<target>.seq`); the sidecar is the evidence log behind
it.

## Fields

Every sidecar has exactly these fields (JSON object, keys sorted, two-space indent, trailing
newline). Two optional anomaly fields appear only when set, so clean captures stay minimal.

| Field                | Always | Meaning                                                               |
| -------------------- | ------ | --------------------------------------------------------------------- |
| `query_id`           | yes    | The db entry id of the query that was sent                            |
| `reply_id`           | yes    | The db entry id of the reply (`responds` target) the bytes pin        |
| `target`             | yes    | Target slug (`tmux`, `betamax`, …) — also the directory name          |
| `identity`           | yes    | Identity record for the whole run (object, below)                     |
| `timestamp`          | yes    | UTC run timestamp, `YYYY-MM-DDTHH:MM:SSZ`, one clock per run          |
| `status`             | yes    | `answered` (at least one reply byte before the deadline) or `timeout` |
| `reply_len`          | yes    | Raw reply byte count (`0` when silent)                                |
| `reply_escaped`      | yes    | Reply bytes, fixture-escaped (empty string when silent)               |
| `late_reply_escaped` | no     | Late-arriving reply bytes, escaped (see below)                        |
| `echo_suspect`       | no     | `true` when the "reply" merely echoes the query (see below)           |

The anomaly fields: `late_reply_escaped` records bytes that arrived only after the deadline had
already declared this query silent — kept on the query they belong to, never attributed to the
next one. `echo_suspect` is set when the "reply" is byte-identical to the query — almost
certainly echo, the exact fabrication failure mode the quarantine exists for; such a line never
mints a fixture.

The `identity` object:

| Field               | Meaning                                                                       |
| ------------------- | ----------------------------------------------------------------------------- |
| `target`            | Target slug, repeated for standalone readability                              |
| `da1_escaped`       | Raw DA1 reply (`CSI c` → primary device attributes), escaped; empty if silent |
| `xtversion_escaped` | Raw XTVERSION reply (`CSI > q`), escaped; empty if silent                     |
| `version`           | Best-effort human version string (resolution rule below)                      |

`version` resolution: the XTVERSION self-report wins when present — the emulator naming itself
is authoritative (betamax hosts ghostty, so its wire says `libghostty`) — else the adapter's
out-of-band hint (`tmux -V`, `betamax --version`).

## Example

```json
{
  "identity": {
    "da1_escaped": "\\e[?1;2;4c",
    "target": "tmux",
    "version": "tmux 3.7b",
    "xtversion_escaped": "\\eP>|tmux 3.7b\\e\\\\"
  },
  "query_id": "csi.da.primary",
  "reply_escaped": "\\e[?1;2;4c",
  "reply_id": "csi.da.primary_report",
  "reply_len": 9,
  "status": "answered",
  "target": "tmux",
  "timestamp": "2026-07-07T10:59:13Z"
}
```

## Relationship to the other artifacts

One capture run writes three artifact kinds in one pass (`qdb capture`):

1. These sidecars — the per-entry evidence log, answered and silent alike. This is the **wire
   record**: `status` (answered/timeout) and `echo_suspect` are what the terminal did.
2. `origin=capture:` reply fixtures — minted only for clean answered lines (never for
   `echo_suspect` ones), payload byte-identical to `reply_escaped`.
3. `db/results/<target>.toml` — the conformance results (schema v2, see `db/README.md`). This is
   the **interpretation**: the wire record becomes a support verdict (`answered` → `supported`,
   `echo_suspect` → `unsupported`, `timeout` → `no-reply`), joined by the plan's `unprobeable`
   and replay-class-`skipped` entries so every query is accounted for. The sidecar is the
   evidence; the results row is the claim derived from it.

`qdb run` (the conformance pass) is the same runner loop with recording off: it writes only the
results seed. `qdb validate` cross-checks that every `origin=capture:` fixture has a sidecar
directory and that results rows name real entries.
