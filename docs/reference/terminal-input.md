# Terminal Input Reference

`InputBytes` is qwertty's first public input value. It represents raw bytes read from a terminal
session. `InputEvent` is the first basic classification layer above those bytes. It can distinguish
complete UTF-8 text, ASCII control bytes, a small set of Escape-prefixed keys, and bytes qwertty
intentionally leaves undecoded.

## Runtime Boundary

This slice adds no runtime dependency and no Cargo feature. `TerminalSession::read_input` uses the
same runtime-neutral terminal device owner as output and cleanup.

qwertty remains async-first. Async input should enter when the library owns runtime-backed reads,
event delivery, query response routing, cancellation behavior, and wakeup semantics. Adding an
async method that only wraps a blocking file read would make the public API look async without
proving the event boundary.

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

## Basic Events

`InputBytes::events` classifies the small subset qwertty can name honestly today:

```rust
use qwertty::{ControlInput, InputBytes, InputEvent, KeyInput};

let input = InputBytes::new(b"A\x1b[A\r\x03".to_vec());

assert_eq!(
    input.events(),
    vec![
        InputEvent::Text('A'),
        InputEvent::Key(KeyInput::Up),
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
two `TerminalSession::read_input` calls, each incomplete chunk remains undecoded until a later
stateful input decoder owns buffering policy.

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

Unknown or incomplete Escape-prefixed input remains undecoded:

```rust
use qwertty::{InputBytes, InputEvent};

let input = InputBytes::new(b"\x1b[Z".to_vec());

assert_eq!(
    input.events(),
    vec![InputEvent::Undecoded(InputBytes::new(b"\x1b[Z".to_vec()))]
);
```

## What Remains Undecoded

The basic event layer does not classify or interpret:

- incomplete or invalid UTF-8;
- unsupported or incomplete Escape-prefixed key sequences;
- broader Control Sequence Introducer messages;
- terminal query responses;
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
