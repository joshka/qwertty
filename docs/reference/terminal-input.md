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
