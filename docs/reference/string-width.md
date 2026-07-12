# String width

`width_of(s, caps)` answers the question every terminal layout needs: how many columns will this
string occupy when drawn? Getting it wrong misaligns status lines, tables, and cursor math.

```rust
use qwertty::{width_of, Capabilities};

// With no probed terminal identity, the static unicode-width baseline is used.
let caps = Capabilities::default();
assert_eq!(width_of("hello", &caps), 5);
assert_eq!(width_of("中文", &caps), 4); // wide CJK: two columns each
```

## Why it takes `Capabilities`

A static width table (the `unicode-width` crate) is correct for the overwhelming majority of text —
ASCII, CJK, Hangul, single emoji, combining marks, zero-width. But real terminals **disagree with
that table, and with each other**, on a small, named set of grapheme clusters: ZWJ emoji sequences
(a family emoji), skin-tone modifiers, regional-indicator flags, and VS16 emoji presentation. No
single static number is right for all of them — one terminal renders the ZWJ family in 2 columns,
another in 8.

So width is a property of *which terminal is rendering*, and that is exactly what qwertty already
knows. `width_of` uses a hybrid model (design 09-width):

- a static `unicode-width` **baseline** for the stable core (the bulk of text), and
- a per-terminal **deviation table**, measured from live conformance and keyed on the terminal's
  identity, for the clusters that terminal renders off-baseline.

An unknown terminal, or a cluster no terminal was measured to render oddly, uses the baseline — there
is never an invented per-terminal claim.

## Mode 2027 is observed, not enabled

Mode 2027 (grapheme clustering) makes a terminal that honours it collapse each cluster to a single
grapheme width. `width_of` reads the observed 2027 state from `caps.grapheme_clustering` and picks
the matching measured advance. It **never enables** 2027 — it changes no terminal state, performs no
I/O, and reflects the mode the terminal is actually in.

```rust,ignore
// caps comes from a probe (see the capabilities reference); width_of never probes itself.
let caps = session.probe_capabilities(Duration::from_millis(150)).await?;
let cols = width_of("a 👨‍👩‍👧‍👦 family", &caps);
```

## Where the numbers come from

The deviation table is generated from the conformance width measurements in `db/width/*.toml`
(`qdb width-probe`), so the numbers are measured on real terminals, never asserted. See
`examples/measure_string_width.rs` for a runnable probe-then-measure example.
