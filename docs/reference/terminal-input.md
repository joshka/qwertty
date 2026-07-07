# Terminal Input Reference

`InputBytes` is qwertty's raw input value: one operating-system read, kept exactly as the terminal
device reported it, with no meaning assigned. Decoding those bytes happens in two layers above it.
`SyntaxParser` is the total, lossless syntax layer that classifies every byte into a `SyntaxToken`
by its ECMA-48 family. `SemanticDecoder` is the semantic layer that maps those tokens to the typed
`Event` vocabulary applications consume. Typed terminal reports (cursor position and terminal
status) parse from the same syntax tokens through the `report` module.

## Runtime Boundary

Reading raw bytes adds no runtime dependency and no Cargo feature. `TerminalSession::read_input`
uses the same runtime-neutral terminal device owner as output and cleanup.

qwertty remains async-first. The first async public surface is `TokioTerminalSession`, a
Tokio-specific session owner behind an optional `tokio` Cargo feature. It reads through
runtime-backed terminal I/O, feeds bytes through a `SemanticDecoder`, preserves unrelated decoded
events, and documents cancellation behavior at the event-delivery boundary.

Adding an async method that only wraps a blocking file read would make the public API look async
without proving the event boundary. Runtime-agnostic async traits are also deferred until the Tokio
owner proves behavior that another runtime can share.

See [Tokio Input Ownership And Query Handoff](
crate::docs#tokio-input-ownership-and-query-handoff) for the session-owned event loop and live
query model above this decoding layer.

## Reading Bytes

`TerminalSession::read_input` reads one chunk of bytes from the terminal device into a caller-owned
buffer and returns those bytes as `InputBytes`:

```rust,no_run
use qwertty::{TerminalSession, commands};

# fn main() -> qwertty::Result<()> {
let mut session = TerminalSession::open()?;
session
    .text("press a key, then Enter\r\n")?
    .flush()?;

let mut buffer = [0; 32];
let input = session.read_input(&mut buffer)?;

session
    .command(commands::screen::clear())?
    .text(format!("read {} byte(s)\r\n", input.len()))?
    .flush()?;

session.leave()?;
# Ok(())
# }
```

The raw bytes stay available exactly as read. For example, if the terminal reports the `A` key
followed by an Up arrow, the raw bytes may be:

```text
A ESC [ A
```

In byte form:

```text
\x41\x1b\x5b\x41
```

`InputBytes::as_bytes` returns that byte sequence exactly as read.

## Decoding Layers

The raw bytes above are decoded by two layers, documented in full below:

- [Syntax Tokens](#syntax-tokens) — `SyntaxParser` classifies *every* byte into a lossless
  `SyntaxToken` by its ECMA-48 family (text, C0 control, CSI, OSC, DCS, APC, PM, SOS, escape, or
  malformed), carrying partial sequences across `read_input` chunks so split input tokenizes
  identically to feeding it whole.
- [Key Events](#key-events) — `SemanticDecoder` maps those tokens to the typed `Event` vocabulary:
  `KeyEvent` values for keys and lossless `Event::Syntax` passthrough for complete-but-unmapped
  tokens.

Typed terminal reports (cursor position and terminal status) parse from the same CSI syntax tokens;
see [Typed Reports](#typed-reports).

## Query Routing

The input layer owns byte-to-event decoding and typed response matching. It does not own live query
requests, terminal writes, timeouts, cancellation, or runtime readiness.

Those live concerns belong to `TokioTerminalSession` for the first async query helpers. When a
query helper reads unrelated decoded input before its response, that input must remain queued for
later `TokioTerminalSession::next_event` calls.

If a query helper times out before its matching reply arrives, qwertty does not keep claiming that
future reply. A later `TokioTerminalSession::next_event` call receives it through the ordinary
decoded input path, typically as an `Event::Syntax` passthrough carrying the CSI token.

If the terminal path closes before any matching reply arrives, the live helper returns a terminal
read error instead of a timeout. Under the current implementation, the source error kind is
`UnexpectedEof`.

The same is true for typed reports that match some other helper's response shape. A live
cursor-position query does not consume terminal-status reports, and a live terminal-status query
does not consume cursor-position reports. Those reports remain visible through the ordinary
decoded input path.

Query-shaped CSI that does not form a valid cursor-position report or terminal-status report also
remains visible through the ordinary decoded input path. The live helper does not consume it just
because it looked like query-related CSI.

The first router boundary is internal to the Tokio session owner. qwertty does not yet expose a
generic query router, multiple simultaneous live queries, capability probing, or a runtime-agnostic
async query trait.

The [Tokio Input Ownership And Query Handoff](
crate::docs#tokio-input-ownership-and-query-handoff) guide explains how applications should treat
that session-owned routing boundary.

When no cursor position report is present, the match result contains no report and all events remain
available.

## Syntax Tokens

`SyntaxParser` is qwertty's total, lossless syntax layer. It classifies *every* input byte into a
`SyntaxToken` by its ECMA-48 family without assigning protocol meaning:

- `SyntaxToken::Text` for maximal runs of printable UTF-8, including multibyte characters.
- `SyntaxToken::Control` for a single C0 control byte that is not a sequence introducer.
- `SyntaxToken::Csi` for a complete CSI sequence, with structured `ControlParams` access.
- `SyntaxToken::Osc`, `Dcs`, `Apc`, `Pm`, `Sos` for complete control-string sequences.
- `SyntaxToken::Esc` for a complete non-CSI, non-string escape, or a bare trailing `ESC`.
- `SyntaxToken::Malformed` for any byte run that cannot be valid syntax.

Nothing collapses into an opaque "undecoded" bucket: OSC/DCS/APC/PM/SOS payloads, 8-bit C1
introducers and terminators (per ECMA-48), and aborted or invalid input are each preserved as their
own honest token, so no input byte is ever lost.

```rust
use qwertty::{SyntaxParser, SyntaxToken};

let mut parser = SyntaxParser::new();
let mut tokens = parser.feed(b"hi\x1b]52;c;SGVsbG8=\x07");
tokens.extend(parser.finish());

assert_eq!(tokens[0], SyntaxToken::Text(b"hi".to_vec()));
match &tokens[1] {
    SyntaxToken::Osc(osc) => assert_eq!(osc.payload(), b"52;c;SGVsbG8="),
    other => panic!("expected Osc, got {other:?}"),
}
```

The layer upholds four invariants. Concatenating each token's raw bytes reconstructs the input
byte-for-byte. Any chunking of the same input yields the identical token sequence, with continuation
state held in the parser and `SyntaxParser::finish` flushing pending bytes. String payloads are
bounded by a configurable limit (default 64 KiB via `SyntaxParser::with_payload_limit`); an
over-limit payload sets `StringSequence::truncated` and records the dropped-byte count instead of
buffering without bound. The bound is enforced while bytes accumulate, so parser memory stays
bounded even when a terminator never arrives. The same cap applies to CSI parameter runs, which
overflow to `SyntaxToken::Malformed` with nothing dropped, and to the size of individual text
tokens. CSI and DCS parameters keep both raw bytes and parsed numbers, preserve `:` versus `;`
separators, and flag param-count overflow rather than merging silently.

Because bytes `0x80..=0x9f` are both C1 controls and UTF-8 continuation bytes, a C1 byte is treated
as an introducer only at a position where a new character starts, never inside an in-progress UTF-8
sequence.

The semantic layer that turns these tokens into typed key, mouse, paste, and report events builds on
`SyntaxParser`. Its first slice, the key-event vocabulary, is described in [Key Events](#key-events)
below.

### Fuzzing

Three of these syntax-layer invariants are fuzzed continuously, not just checked against a fixed
corpus. A `cargo-fuzz` target backs each one: reconstruction (token bytes plus recorded dropped
counts always account for the input length, exactly), split-equivalence (an arbitrary chunking of any
input yields the same token sequence as feeding it whole, including under a small payload limit), and
bounded no-panic (parser memory stays within the payload limit plus a small constant after every
`feed`, and no input panics). A fourth target, `correlator_properties`, fuzzes the query correlator's
race-freedom invariants (see [Typed Reports](#typed-reports)): the passthrough sequence equals the
fed non-consumed events in order, every completion matches its expectation's reply shape, and no
reply completes an expectation registered after that reply was fed. The deterministic suites in
`tests/syntax.rs` and the correlator's seeded property test prove these over fixed and pseudo-random
cases; the fuzz targets generalize them over arbitrary input. CI runs each target briefly on every
push, and `just fuzz` runs them locally.

## Key Events

`SemanticDecoder` is qwertty's semantic input layer. It owns a `SyntaxParser` and maps its lossless
tokens to the typed `Event` vocabulary applications consume. Feed input chunks with
`SemanticDecoder::feed` and flush pending state with `SemanticDecoder::finish`, exactly like the
syntax parser; the owned parser carries partial sequences across chunks, so split input decodes
identically to feeding it whole.

The `Event` vocabulary is **pre-freeze until milestone M4 exit**. These `event::` types (`Event`,
`KeyEvent`, `Key`, `Modifiers`, `KeyEventKind`, `TextPayload`) change freely before the first
published version and calcify at that release. Every enum is non-exhaustive, so the M4 variants add
without breaking existing code.

```rust
use qwertty::{Event, Key, SemanticDecoder};

let mut decoder = SemanticDecoder::new();
let mut events = decoder.feed(b"hi\r\x1b[A\x1b[?25n");
events.extend(decoder.finish());

// Text -> one key event per character, with the character carried as associated text.
assert_eq!(events[0].key_event().map(|k| k.key()), Some(Key::Char('h')));
assert_eq!(
    events[0].key_event().and_then(|k| k.text()).map(|t| t.as_str()),
    Some("h")
);
// CR -> Enter, ESC [ A -> Up.
assert_eq!(events[2].key_event().map(|k| k.key()), Some(Key::Enter));
assert_eq!(events[3].key_event().map(|k| k.key()), Some(Key::Up));
// A CSI qwertty does not decode yet passes through losslessly as syntax.
assert_eq!(
    events[4].syntax_token().map(|t| t.as_bytes()),
    Some(&b"\x1b[?25n"[..])
);
```

### What Maps Today

This slice decodes the legacy input set — text, C0 controls, the four arrow keys, and standalone
Escape — over the richer syntax layer, and passes everything else through losslessly:

- **Text.** A printable UTF-8 run becomes one `KeyEvent` per character. The keycode is the trivial
  `Key::Char(c)` and the decoded character is also carried in the event's `TextPayload`, so text and
  key association is never lost. Legacy input always carries exactly one character per event.
- **C0 controls.** `CR` (`0x0d`) maps to `Key::Enter`, `HT` (`0x09`) to `Key::Tab`, and both `DEL`
  (`0x7f`) and `BS` (`0x08`) to `Key::Backspace`. Terminals disagree on which byte the Backspace key
  sends, so this layer folds both into the Backspace key to match the kitty model; a dedicated
  Delete key is a distinct `CSI u` code that arrives in M4. Every other C0 control is preserved as
  `Key::Control(byte)` so no input is lost.
- **Arrow keys.** `ESC [ A/B/C/D` (with no parameters or the single default parameter `1`) map to
  `Key::Up`, `Key::Down`, `Key::Right`, and `Key::Left`.
- **Standalone Escape.** A bare Escape, flushed by the layer above and surfaced by the parser as a
  lone `SyntaxToken::Esc`, maps to `Key::Escape`. The Escape-versus-sequence timing policy stays in
  the layer above this decoder; the decoder only ever sees a bare Escape once that layer has
  decided it stood alone.
- **Everything else** — a CSI qwertty does not decode yet, an OSC/DCS/APC/PM/SOS control string,
  another escape sequence, or a malformed run — passes through losslessly as `Event::Syntax`,
  carrying its token and bytes. New protocols degrade to visible, lossless syntax, never a fake
  keypress.

A `KeyEvent` from this slice is always a press (`KeyEventKind::Press`) with no modifiers
(`Modifiers::empty()`), because legacy input carries neither a press/release distinction nor
modifier information for these keys.

### What Arrives In Milestone M4

The vocabulary is deliberately shaped for the milestone M4 input work, which adds:

- **kitty `CSI u` key decoding**: functional keys (Home, End, F-keys, and the rest), real
  `Modifiers` values, `KeyEventKind::Repeat` and `KeyEventKind::Release`, and multi-codepoint
  associated `TextPayload` values.
- **mouse** events (SGR and legacy).
- **paste** as a dedicated aggregated event, not keyless text.
- **focus** in and out events.
- **resize** events.

Until then `Event` carries only `Key` and `Syntax`, and the modifier, key-kind, and multi-codepoint
text capacity exists so those M4 additions need no vocabulary change.

## Typed Reports

A report is a reply a terminal sends in answer to a query: a cursor position report, a device status
report, and later device attributes, mode reports, and colour reports. The `report` module holds the
typed parsers for these over the syntax layer. `report::CursorPositionReport` parses a
`CSI row ; column R` cursor report from a `ControlSequence`, and `report::TerminalStatusReport`
parses `CSI 0 n` (ready) and `CSI 3 n` (malfunction). Each parser is pure: it reads one CSI token and
returns a typed value or `None`, rejecting anything that is not exactly the report shape. These are
the report parsers the query correlator consumes and the ghostty-rs encode oracle checks against.
They are re-exported at the crate root (`CursorPositionReport`, `TerminalStatusReport`,
`TerminalStatus`) for convenience and are also reachable through `report::` for a stable module
path; both paths name the same types.

Matching a report to the query that provoked it is a separate concern from parsing it. The
correlator is a sans-io state machine (no clock, no I/O, no async) that holds a small ordered set of
typed expectations, is fed decoded events, and for each event either completes the first matching
expectation or passes the event through untouched in arrival order. Its rules are the risk core of
the query story: full-discriminator matching so no two pending expectations share a reply; duplicate
identical queries coalesce to one expectation with a waiter count and a shared result; a late reply
that arrives after its query timed out, was cancelled, or hit EOF is never matched and passes
through; and a `CSI c` Primary Device Attributes reply acts as a shape-tolerant fence. One report
shape is deliberately ambiguous: `CSI 1 ; modifier R` is both a row-1 cursor report and a modified-F3
key report, so the cursor matcher refuses that form and lets it pass through rather than guess. The
correlator is currently an internal (`pub(crate)`) building block; the sessions that drive it arrive
in a later slice. Applications that need to read an arbitrary reply the typed methods do not cover
still get it losslessly through the ordinary event stream as an `Event::Syntax` passthrough, so
nothing is stolen or reordered.

## What Is Not Yet Decoded Semantically

The syntax layer tokenizes every byte losslessly, but the semantic layer does not yet map every
token to a typed high-level event. It does not yet interpret:

- terminal query responses other than cursor position and terminal status reports;
- CSI meanings other than the four arrow keys and those two report shapes;
- kitty `CSI u` functional keys, modifiers, key-event kinds, and associated text;
- paste boundaries;
- mouse, focus, graphics, clipboard, or vendor extension protocols.

Those behaviors belong to the milestone M4 semantic slices. Until then such input still reaches the
application losslessly through `Event::Syntax`, carrying its `SyntaxToken`, so callers that need
richer interpretation can inspect the token at the application boundary without losing bytes.

## Cleanup

Input ownership does not add cleanup beyond the session lifecycle. Starting a `TerminalSession`
enters raw mode. Call `TerminalSession::leave` during orderly shutdown so cooked-mode restoration
errors can be reported. Drop remains a best-effort fallback through the underlying terminal.

## Platform Support

The live terminal implementation currently supports Unix. Unsupported platforms expose the same
public types where possible and return `Error::Unsupported` for live terminal operations.

See [Platform Support](crate::docs#platform-support) for the durable user-facing summary of that
boundary.
