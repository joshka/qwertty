# Design 06 — Capability detection and policy

Probing vs static evidence; degradation over ssh/tmux/IDE terminals; security posture.
Implements R-CAP-1–5, R-SEC-1–3; addresses FM-C*, FM-M*, FM-X*.

## Capability model

**A `Capabilities` struct of tri-state findings, each carrying its evidence.**

```rust
struct Finding<T> { value: Option<T>, evidence: Evidence }
enum Evidence { Probed { via: SequenceId }, Inferred { via: EnvKey }, Unknown }
```

- `Option<T>=None` + `Unknown` means *unknown*, never *unsupported* (FM-C4: DA1 silence, mux
  swallowing — R-CAP-2). Consumers and our own emit-gating read the evidence, so "we probed
  and it said no" and "nothing answered" degrade differently.
- Populated only by the explicit probe bundle (design 03) plus the env layer below. Nothing
  probes at construction (FM-C7, reedline#987).
- **Identity is a finding too** (R-CAP-5): `TerminalIdentity { program, version, mux_stack }`
  from XTVERSION/DA2 replies cross-checked with env (`TERM_PROGRAM`, `TMUX`, `ZELLIJ`,
  `KITTY_WINDOW_ID`, …). Mux presence is first-class: `mux_stack: Vec<Mux>` because answers
  under tmux describe tmux (FM-C3) and passthrough gating differs per layer (FM-M1/M2).
- **Conformance-derived quirk findings** (reviewer M4): behaviors that are real but
  unqueryable — headlined by `inline_insertion_safe` (may I use DECSTBM/RI history insertion
  here without destroying scrollback? FM-V1/V2) and, per the gate's P15 reversal,
  **width behavior** (how this terminal+version measures graphemes under its 2027 state,
  measured empirically by the runner via emit-grapheme + CPR; FM-P15 — the mechanism and
  Unicode-data policy are a named Phase 3 design item) — enter `Capabilities` as findings
  sourced `Conformance { result: … }`, looked up by identity from the checked-in results the
  runner produces. Scroll-region/insertion command helpers gate on this finding exactly as 2026
  emission gates on its probe — the codex-shaped consumer gets a yes/no/unknown *before*
  emitting, which is the question tui2 died unable to answer. (A portable insertion *recipe*
  built on these findings is an explicit Phase 3 deliverable, driven by matrix data.)
- **Env heuristics are a documented, inspectable table** (`COLORTERM`, the OSC 8 sniff set,
  `NO_COLOR`/`FORCE_*` overrides — FM-C12), applied only where no query exists, always
  labeled `Inferred`. The table ships in the database repo-side so the docs and the code
  generate from one list.
- **Caching policy** (FM-C8): findings are per-attachment. The session snapshots capabilities
  at probe time; `resume()`/reattach and mode-2031-style change events mark affected findings
  stale; re-probing is the app's explicit call (cheap: probe bundle is one round-trip). No
  global cache, no TTLs.
- **Verify-after-push** for kitty flags: push, query granted set, record granted-not-requested
  (FM-C11; helix handshake) — the session's mode ledger stores what was *granted* so teardown
  pops reality, not intent.

Emit-gating rule (R-CAP-4): typed commands for gated features take the capability finding (or
an explicit `Override::Force`) — you cannot accidentally emit 2026 wraps or kitty pushes into
a terminal that never answered (zellij's TERM-sniff corruption, FM-C10; codex's unconditional
2026 leaking onto consoles, FM-V4).

## Security policy layer

**A `Policy` value on the session gating side-effecting/exfiltrating features** (R-SEC-1).

- Gated: clipboard write, clipboard **read** (separately), notifications, window title,
  file transfer, mux passthrough wrapping, answer-influencing overrides. Default profile:
  `Policy::restricted()` — clipboard read **off**, file transfer **off**, notifications
  **off**, clipboard write **prompt-the-terminal** (i.e. allowed, since terminals gate it —
  FM-X4), title **sanitized-on** (control chars + bidi/invisible blocklist, the codex
  practice, FM-X3). `interactive()` and `trusted()` presets widen it; every preset is a plain
  struct the app can build by hand.
- Denied operations return a typed `PolicyDenied` error naming the gate — teachable (OQ-4:
  gates must explain themselves; each gate's rustdoc cites its FM-X attack class).
- qwertty **never auto-responds** to terminal-initiated requests (R-SEC-2) — not a policy
  knob, a non-feature.
- Paste hygiene (R-SEC-3): paste events expose `contains_control()`; a policy flag opts into
  stripping. Bracketed-paste terminator loss is bounded by decoder limits (FM-A8).

## Degradation playbook (documented, tested)

Per-transport reality, encoded as docs + conformance fixtures rather than code branches where
possible: tmux (answers for itself; passthrough off by default — emit passthrough only when
`mux_stack` says tmux *and* policy allows; FM-M1), screen (assume nothing modern), ssh
(latency budget is caller-owned, but identity helps callers choose: when `mux_stack`/ssh env
is present the probe API surfaces a *suggested* larger budget rather than leaving every
caller to rediscover termion#202 — reviewer m3), mosh (not byte-transparent, FM-M3 — treat as
its own terminal identity; its OSC 52 limits — `c` selector only, single-packet payload cap —
are identity-keyed emission constraints, FM-M2, a declared transport limit qwertty surfaces
rather than solves), IDE terminals (xterm.js quirks are conformance-matrix rows, not
hardcoded caps — R-OUT-6). Mid-session enhancement degradation across a mux (FM-M6) is
likewise *detected* (granted-set verification, stale-marking) rather than prevented — the
transport is lossy and we say so.

## Alternatives considered

- **terminfo as primary source**: rejected on the full FM-C1 record; usable someday as one
  more `Inferred` input, never a decision mechanism.
- **Boolean capability set** (crossterm-shaped `supports_x() -> bool`): rejected — erases the
  unknown/unsupported distinction that FM-C4 shows matters, and hides evidence provenance.
- **Automatic background re-probing** on focus/resize: rejected — probes have side effects
  and latency (FM-C6/C7); staleness is surfaced, re-probe is explicit.
- **Policy as compile-time features**: rejected — the same binary legitimately runs in
  trusted local and hostile-remote contexts; policy is runtime data.
- **Hardcoded per-terminal quirk tables in code** (codex's reflow caps, zellij's DA1): the
  honest version of this is the conformance results + identity model — quirk data lives in
  the database where it's cited and regenerable, not scattered in match arms.
