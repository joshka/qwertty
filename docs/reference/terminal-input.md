# Terminal Input Reference

`InputBytes` is qwertty's first public input value. It represents raw bytes read from a terminal
session. `InputEvent` is the first basic classification layer above those bytes. It can distinguish
complete UTF-8 text, ASCII control bytes, a small set of Escape-prefixed keys, complete Control
Sequence Introducer input, and bytes qwertty intentionally leaves undecoded. `InputDecoder` owns the
first stateful decoding boundary for input that arrives split across terminal reads.

## Runtime Boundary

This slice adds no runtime dependency and no Cargo feature. `TerminalSession::read_input` uses the
same runtime-neutral terminal device owner as output and cleanup.

qwertty remains async-first. The first async public surface is `TokioTerminalSession`, a
Tokio-specific session owner behind an optional `tokio` Cargo feature. It reads through
runtime-backed terminal I/O, feeds bytes through `InputDecoder`, preserves unrelated decoded
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

## Chunk-Local Events

`InputBytes::events` classifies the small subset qwertty can name honestly today:

```rust
use qwertty::{ControlInput, CsiInput, InputBytes, InputEvent, KeyInput};

let input = InputBytes::new(b"A\x1b[A\x1b[?25n\r\x03".to_vec());

assert_eq!(
    input.events(),
    vec![
        InputEvent::Text('A'),
        InputEvent::Key(KeyInput::Up),
        InputEvent::Csi(CsiInput::from_bytes(b"\x1b[?25n").unwrap()),
        InputEvent::Control(ControlInput::CarriageReturn),
        InputEvent::Control(ControlInput::Other(0x03)),
    ]
);
```

The classified text set is complete UTF-8 text within the current `InputBytes` chunk. Incomplete or
invalid UTF-8 remains `InputEvent::Undecoded` with the original bytes preserved:

```rust
use qwertty::{InputBytes, InputEvent};

let input = InputBytes::new(vec![0xc3]);

assert_eq!(
    input.events(),
    vec![InputEvent::Undecoded(InputBytes::new(vec![0xc3]))]
);
```

This method does not buffer across terminal reads. If a multi-byte UTF-8 character is split across
two `TerminalSession::read_input` calls, each incomplete chunk remains undecoded. Use
`InputDecoder` when the caller wants qwertty to carry incomplete input across reads.

## Stateful Decoding

`InputDecoder` buffers the boundary cases qwertty can describe today: incomplete UTF-8 scalar
values and incomplete Control Sequence Introducer input. It accepts byte slices or `InputBytes`
values and returns the same `InputEvent` values as the chunk-local classifier:

```rust
use qwertty::{CsiInput, InputDecoder, InputEvent, KeyInput};

let mut decoder = InputDecoder::new();

assert!(decoder.decode([0xc3]).is_empty());
assert_eq!(decoder.decode([0xa9]), vec![InputEvent::Text('é')]);

assert!(decoder.decode(b"\x1b[").is_empty());
assert_eq!(decoder.decode(b"A"), vec![InputEvent::Key(KeyInput::Up)]);

assert!(decoder.decode(b"\x1b[?25").is_empty());
assert_eq!(
    decoder.decode(b"n"),
    vec![InputEvent::Csi(CsiInput::from_bytes(b"\x1b[?25n").unwrap())]
);
```

`InputDecoder::pending_bytes` exposes the exact buffered bytes for diagnostics and tests. The
decoder keeps those bytes until a later `decode` call resolves them or `finish` returns them as
undecoded input:

```rust
use qwertty::{InputBytes, InputDecoder, InputEvent};

let mut decoder = InputDecoder::new();

assert!(decoder.decode(b"\x1b[").is_empty());
assert_eq!(decoder.pending_bytes(), b"\x1b[");
assert_eq!(
    decoder.finish(),
    vec![InputEvent::Undecoded(InputBytes::new(b"\x1b[".to_vec()))]
);
```

`finish` does not guess whether a pending Escape byte was a standalone Escape key or the start of a
longer sequence. That timing and ambiguity policy belongs to a later input layer.

The classified control set is ASCII C0 controls and Delete. Common controls have named
`ControlInput` variants such as `Tab`, `LineFeed`, `CarriageReturn`, `Escape`, and `Delete`. Less
common controls remain available as `ControlInput::Other(byte)`.

## Escape-Prefixed Keys

Escape is classified as `ControlInput::Escape` only when it appears by itself. The first Escape
parser recognizes these common arrow-key encodings:

- Up arrow: `ESC [ A`, byte form `\x1b[A`.
- Down arrow: `ESC [ B`, byte form `\x1b[B`.
- Right arrow: `ESC [ C`, byte form `\x1b[C`.
- Left arrow: `ESC [ D`, byte form `\x1b[D`.

Other complete CSI input is preserved as `CsiInput` instead of being interpreted:

```rust
use qwertty::{CsiInput, InputBytes, InputEvent};

let input = InputBytes::new(b"\x1b[?25n".to_vec());

assert_eq!(
    input.events(),
    vec![InputEvent::Csi(CsiInput::from_bytes(b"\x1b[?25n").unwrap())]
);
```

Unsupported non-CSI Escape-prefixed input remains undecoded:

```rust
use qwertty::{InputBytes, InputEvent};

let input = InputBytes::new(b"\x1bZ".to_vec());

assert_eq!(
    input.events(),
    vec![InputEvent::Undecoded(InputBytes::new(b"\x1bZ".to_vec()))]
);
```

## CSI Input

`CsiInput` preserves the original bytes and exposes the syntax qwertty can identify without
assigning meaning. The supported shape is `ESC [`, followed by parameter bytes `0x30..=0x3f`,
intermediate bytes `0x20..=0x2f`, and one final byte `0x40..=0x7e`.

```rust
use qwertty::CsiInput;

let csi = CsiInput::from_bytes(b"\x1b[?25n").unwrap();

assert_eq!(csi.as_bytes(), b"\x1b[?25n");
assert_eq!(csi.parameter_bytes(), b"?25");
assert_eq!(csi.private_marker_bytes(), b"?");
assert_eq!(csi.intermediate_bytes(), b"");
assert_eq!(csi.final_byte(), b'n');
```

Most CSI input is syntax, not policy. qwertty does not yet decide whether a generic CSI value is a
device status report, keyboard enhancement response, mouse event, or vendor extension.

## Cursor Position Reports

`CursorPositionReport` parses the first interpreted query-shaped CSI input. Terminals commonly
answer a cursor position query with `CSI row ; column R`, using one-based terminal protocol
coordinates:

```rust
use qwertty::{CsiInput, CursorPositionReport, ProtocolPosition};

let csi = CsiInput::from_bytes(b"\x1b[12;34R").unwrap();
let report = CursorPositionReport::from_csi(&csi).unwrap();

assert_eq!(report.position(), ProtocolPosition::new(12, 34));
assert_eq!(report.row(), 12);
assert_eq!(report.column(), 34);
```

Malformed reports, unrelated CSI input, private reports, and reports with intermediate bytes do not
produce a cursor position report:

```rust
use qwertty::{CsiInput, CursorPositionReport};

let csi = CsiInput::from_bytes(b"\x1b[?25n").unwrap();

assert_eq!(CursorPositionReport::from_csi(&csi), None);
```

This parser does not prove which query caused the report. With the optional `tokio` feature on
Unix, `TokioTerminalSession::request_cursor_position` writes the request, flushes output, waits for
this report shape, applies a timeout, and preserves unrelated decoded input. General query routing
starts as Tokio-session-owned state rather than a public router.

`CursorPositionReport::match_events` separates the first cursor position report from decoded input
events while returning all unrelated events to the caller:

```rust
use qwertty::{CsiInput, CursorPositionReport, InputEvent, ProtocolPosition};

let csi = CsiInput::from_bytes(b"\x1b[12;34R").unwrap();
let matched = CursorPositionReport::match_events(vec![
    InputEvent::Text('x'),
    InputEvent::Csi(csi),
]);

assert_eq!(
    matched.report().map(CursorPositionReport::position),
    Some(ProtocolPosition::new(12, 34))
);
assert_eq!(matched.remaining_events(), &[InputEvent::Text('x')]);
```

## Terminal Status Reports

`TerminalStatusReport` parses ECMA-48 Device Status Report terminal status replies. Terminals
commonly answer a terminal status query with `CSI 0 n` for ready or `CSI 3 n` for malfunction:

```rust
use qwertty::{CsiInput, TerminalStatus, TerminalStatusReport};

let ready = CsiInput::from_bytes(b"\x1b[0n").unwrap();
let report = TerminalStatusReport::from_csi(&ready).unwrap();

assert_eq!(report.status(), TerminalStatus::Ready);

let malfunction = CsiInput::from_bytes(b"\x1b[3n").unwrap();
let report = TerminalStatusReport::from_csi(&malfunction).unwrap();

assert_eq!(report.status(), TerminalStatus::Malfunction);
```

Malformed reports, private reports, reports with intermediate bytes, and unsupported status codes
do not produce a terminal status report:

```rust
use qwertty::{CsiInput, TerminalStatusReport};

let csi = CsiInput::from_bytes(b"\x1b[?0n").unwrap();

assert_eq!(TerminalStatusReport::from_csi(&csi), None);
```

`TerminalStatusReport::match_events` separates the first terminal status report from decoded input
events while returning all unrelated events to the caller:

```rust
use qwertty::{CsiInput, InputEvent, TerminalStatus, TerminalStatusReport};

let csi = CsiInput::from_bytes(b"\x1b[0n").unwrap();
let matched = TerminalStatusReport::match_events(vec![
    InputEvent::Text('x'),
    InputEvent::Csi(csi),
]);

assert_eq!(
    matched.report().map(TerminalStatusReport::status),
    Some(TerminalStatus::Ready)
);
assert_eq!(matched.remaining_events(), &[InputEvent::Text('x')]);
```

This parser and matcher do not prove which query caused the report. Live status requests belong to
`TokioTerminalSession::request_terminal_status`, which owns terminal writes, flushing, timeout
policy, and preserved unrelated input.

## Query Routing

The input layer owns byte-to-event decoding and typed response matching. It does not own live query
requests, terminal writes, timeouts, cancellation, or runtime readiness.

Those live concerns belong to `TokioTerminalSession` for the first async query helpers. When a
query helper reads unrelated decoded input before its response, that input must remain queued for
later `TokioTerminalSession::next_event` calls.

If a query helper times out before its matching reply arrives, qwertty does not keep claiming that
future reply. A later `TokioTerminalSession::next_event` call receives it through the ordinary
decoded input path, typically as `InputEvent::Csi(...)`.

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

`SyntaxParser` is qwertty's total, lossless syntax layer. Where the `InputEvent` decoder above names
only the small set of shapes it can classify honestly and leaves everything else as
`InputEvent::Undecoded`, `SyntaxParser` classifies *every* input byte into a `SyntaxToken` by its
ECMA-48 family without assigning protocol meaning:

- `SyntaxToken::Text` for maximal runs of printable UTF-8, including multibyte characters.
- `SyntaxToken::Control` for a single C0 control byte that is not a sequence introducer.
- `SyntaxToken::Csi` for a complete CSI sequence, with structured `ControlParams` access.
- `SyntaxToken::Osc`, `Dcs`, `Apc`, `Pm`, `Sos` for complete control-string sequences.
- `SyntaxToken::Esc` for a complete non-CSI, non-string escape, or a bare trailing `ESC`.
- `SyntaxToken::Malformed` for any byte run that cannot be valid syntax.

The old `InputEvent` decoder preserves bytes but only recognizes complete UTF-8, C0 controls, the
four arrow keys, and 7-bit CSI input; OSC, DCS, APC, PM, SOS, 8-bit C1 forms, and malformed runs all
collapse into `InputEvent::Undecoded`. `SyntaxParser` closes that gap: OSC/DCS/APC/PM/SOS payloads,
8-bit C1 introducers and terminators (per ECMA-48), and aborted or invalid input are all preserved
as their own honest tokens.

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

Three of these invariants are fuzzed continuously, not just checked against a fixed corpus. A
`cargo-fuzz` target backs each one: reconstruction (token bytes plus recorded dropped counts always
account for the input length, exactly), split-equivalence (an arbitrary chunking of any input yields
the same token sequence as feeding it whole, including under a small payload limit), and bounded
no-panic (parser memory stays within the payload limit plus a small constant after every `feed`, and
no input panics). The deterministic suites in `tests/syntax.rs` prove these over the escape-layer
spike corpus and adversarial cases; the fuzz targets generalize the same three properties over
random input. CI runs each target briefly on every push, and `just fuzz` runs them locally.

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

This slice reaches parity with the old `InputDecoder` path, over the richer syntax layer:

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

## What Remains Undecoded

The basic event layer does not classify or interpret:

- incomplete or invalid UTF-8;
- unsupported or incomplete Escape-prefixed sequences;
- terminal query responses other than cursor position and terminal status reports;
- interpreted Control Sequence Introducer meanings other than cursor position and terminal status
  reports;
- paste boundaries;
- mouse, focus, graphics, clipboard, or vendor extension protocols.

Those behaviors belong to later parser, query-routing, and policy slices. Until those layers exist,
callers that need richer interpretation should handle `InputEvent::Undecoded` at the application
boundary.

## Cleanup

Input ownership does not add cleanup beyond the session lifecycle. Starting a `TerminalSession`
enters raw mode. Call `TerminalSession::leave` during orderly shutdown so cooked-mode restoration
errors can be reported. Drop remains a best-effort fallback through the underlying terminal.

## Platform Support

The live terminal implementation currently supports Unix. Unsupported platforms expose the same
public types where possible and return `Error::Unsupported` for live terminal operations.

See [Platform Support](crate::docs#platform-support) for the durable user-facing summary of that
boundary.
