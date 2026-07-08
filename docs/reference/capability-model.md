# Capability Model: Evidence, Identity, And Environment Inference

`Capabilities` is the typed result of the DA1-fenced probe bundle, a struct of typed findings each
carrying evidence of how it was obtained. With the optional `tokio` feature on Unix, it is what
`TokioTerminalSession::probe_capabilities` returns — see "The Capability Probe Bundle (DA1 Fence)"
in `crate::docs::tokio_input_ownership` (included with the `tokio` feature) for how the bundle is
written and fenced. This page explains the `Capabilities` model itself, which is runtime-neutral:
`Finding<T>` and `Evidence`, `TerminalIdentity`, the environment-heuristic table for capabilities
with no query, and how the whole struct behaves as a per-attachment snapshot.

## Finding And Evidence

Every probe-backed `Capabilities` field is a `Finding<T>`:

```rust
use qwertty::{Evidence, Finding};

let synchronized_output: Finding<bool> = Finding::probed(Some(true), "DECRQM 2026");
assert_eq!(synchronized_output.value(), Some(&true));
assert_eq!(
    synchronized_output.evidence(),
    &Evidence::Probed { via: "DECRQM 2026" }
);
assert!(synchronized_output.is_known());
```

`Finding::value()` is `Option<&T>` (or use `.value_copied()` for `Copy` types): `None` means
*unknown*, never *unsupported*. `Finding::evidence()` is a separate axis, `Evidence`:

- **`Evidence::Probed { via }`** — a terminal reply answered. `via` names the query, for example
  `"DECRQM 2026"`, `"OSC 11"`, or `"CSI ?u"`. A DECRQM "mode not recognized" answer is still
  `Probed` even though its value is `None`: the terminal *did* answer, just in the
  negative-unknown way DECRQM allows. That is different from silence.
- **`Evidence::Inferred { via }`** — no query exists for this capability; the value was guessed
  from an environment variable or heuristic named by `via`. Always used for
  `Capabilities::hyperlinks` and `Capabilities::truecolor`, never for the DECRQM/query-backed
  fields.
- **`Evidence::Unknown`** — nothing probed and nothing inferred.

A consumer that only reads `.value()` sees a tri-state `Option<T>`; a consumer that needs to tell
"we asked and got told no" apart from "we never got an answer" reads `.evidence()` as well.
`Evidence` is `#[non_exhaustive]`, so a future evidence source (for example a conformance-matrix
lookup keyed by identity) can be added without an existing `match` becoming non-exhaustive at
compile time in a breaking way — match with a wildcard arm if you need to compile against future
versions.

## Terminal Identity

Identity is a finding too. `Capabilities::identity` is a `TerminalIdentity`:

```rust,ignore
pub struct TerminalIdentity {
    pub program: Option<TerminalProgram>,
    pub version: Option<String>,
    pub mux_stack: Vec<Multiplexer>,
}
```

`program` is a `#[non_exhaustive]` enum (`Kitty`, `Ghostty`, `Iterm2`, `WezTerm`, `Alacritty`,
`Foot`, `Rio`, `VsCode`, `WindowsTerminal`, `AppleTerminal`, `Tmux`, `Screen`, or
`Unknown(String)` preserving unrecognized text verbatim).

### Deriving identity

`probe_capabilities` derives identity from, strongest signal first:

1. **The XTVERSION reply**, when the probe bundle received one. `program_from_xtversion` does
   best-effort substring matching against each terminal's own documented self-report text —
   `"kitty"` inside `kitty(0.35.1)`, `"ghostty"` inside `ghostty 1.0.0`, `"WezTerm"`, `"iTerm2"`,
   `"Alacritty"`, `"foot"`, a case-insensitive `"rio"`, and `"tmux"`/`"screen"` for the
   self-answering case. The full reply text becomes `identity.version` verbatim.
2. **`TERM_PROGRAM`**, when XTVERSION was silent: `iTerm.app`, `Apple_Terminal`, `vscode`,
   `WezTerm`, `ghostty`, `tmux`, `rio`. `TERM_PROGRAM_VERSION` fills `version` when XTVERSION
   carried none.
3. **`TERM`**, consulted last because it is the least reliable signal — terminfo can be stale or
   out of date, `TERM` propagates over ssh to hosts whose database does not know it, and
   multiplexers set it to describe themselves: `xterm-kitty`, `alacritty`, `foot`/`foot-extra`,
   `tmux-256color`/`tmux`, `screen`/`screen-256color`.

Primary Device Attributes (DA1) is deliberately *not* an identity signal here: it is a weak signal
for features, and identity matching from DA1 param shape overlaps across terminals even more than
feature matching does, so `program` is left unresolved rather than guessed from DA1 alone.

### Multiplexer stack

`mux_stack` is independent of `program` and checked unconditionally: `TMUX` pushes
`Multiplexer::Tmux`, `STY` pushes `Multiplexer::Screen`, `ZELLIJ` pushes `Multiplexer::Zellij`.
Nested multiplexers (for example zellij inside tmux) can push more than one entry. This exists
because under a multiplexer, probe replies describe the multiplexer, not the outer terminal — DA1,
XTVERSION, and DECRQM answers all come from whatever program is attached to the pty, so `mux_stack`
records that context explicitly instead of silently reporting the mux's own identity as if it were
the user's terminal. Passthrough gating (tmux `allow-passthrough`) reads `mux_stack` rather than
guessing from `program` alone.

### Testability: the injectable environment source

Both identity derivation and the environment-heuristic table below take an `EnvSource` —
`impl Fn(&str) -> Option<String>` — instead of calling `std::env::var` directly:

```rust
use qwertty::caps::identity_from_env;

let env = |key: &str| match key {
    "TERM_PROGRAM" => Some("iTerm.app".to_owned()),
    _ => None,
};
let identity = identity_from_env(None, env);
assert_eq!(identity.program, Some(qwertty::TerminalProgram::Iterm2));
```

Production code passes `qwertty::caps::std_env_source` (a thin wrapper over `std::env::var`); tests
pass a closure over a fixed map. This keeps the parsing/inference logic exercised without mutating
the real process environment, which is unsound to do from parallel tests.

## Environment Heuristics Have No Query

Two `Capabilities` fields have no backing query at all, because none exists in the protocol:

- **`hyperlinks`** — OSC 8 hyperlink support. Inferred from the documented `HYPERLINK_ENV_HEURISTICS`
  table (mirroring the `supports-hyperlinks` sniff set): `TERM_PROGRAM` ∈ `{Hyper, iTerm.app,
  WezTerm, vscode, ghostty}`, `TERM` ∈ `{xterm-kitty, alacritty}`, or the mere presence of
  `DOMTERM`, `WT_SESSION`, or `KONSOLE_VERSION`. `VTE_VERSION >= 5000` is checked separately as a
  numeric threshold the table's exact-value rows can't express.
- **`truecolor`** — 24-bit RGB SGR support. Inferred from `COLORTERM` ∈ `{truecolor, 24bit}` — a
  workaround for truecolor being otherwise inexpressible in terminfo.

Both tables are public, inspectable data (`qwertty::caps::HYPERLINK_ENV_HEURISTICS`,
`qwertty::caps::VTE_HYPERLINK_MIN_VERSION`), not hidden logic, so a caller can audit exactly which
signal produced a `hyperlinks`/`truecolor` finding. Both findings are always `Evidence::Inferred` or
`Evidence::Unknown` — never `Evidence::Probed`, because no query exists.

### `NO_COLOR` / `FORCE_COLOR` overrides

`truecolor` respects the color-override conventions ahead of the `COLORTERM` sniff:

- `NO_COLOR` set (any value, per no-color.org) forces `Some(false)`, evidence `Inferred { via:
  "NO_COLOR" }`, even when `COLORTERM=truecolor` is also set.
- `FORCE_COLOR` set (any non-empty value) forces `Some(true)`, evidence `Inferred { via:
  "FORCE_COLOR" }`, when `NO_COLOR` is absent.

A consumer that reads `evidence()` can see that a color decision was forced by an override rather
than sniffed from `COLORTERM`.

## Detection Posture

### Dumb terminals

A `TERM=dumb` terminal should not be sent the probe bundle at all — probing has side effects even
when unanswered — so a caller that detects `TERM=dumb` should skip `probe_capabilities` entirely
and treat every finding as unknown. This module has no opinion on *when* a caller probes; it only
guarantees that `identity_from_env` and the env-heuristic functions are safe to call over any
environment, `TERM=dumb` included, since they only read env vars and never write to the terminal.

### Snapshot, not live

`Capabilities` is a snapshot taken at probe time for one attachment. Resume/reattach can move a
session to a different outer terminal (zellij multi-client reattach is the canonical case), and
mode-2031-style theme-change events can change the answer mid-session. There is no cache, no TTL,
and no automatic re-probe: deciding when a stale `Capabilities` should be treated as invalid, and
re-probing, is left to the caller.

## See Also

- "The Capability Probe Bundle (DA1 Fence)" in `crate::docs::tokio_input_ownership` (included with
  the `tokio` feature) — how `probe_capabilities` writes the bundle and applies the DA1 fence.
- `examples/probe_capabilities.rs` — a runnable example printing every finding, its evidence, and
  the derived identity.
