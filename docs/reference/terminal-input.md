# Terminal Input Reference

`InputBytes` is qwertty's first public input value. It represents raw bytes read from a terminal
session before qwertty has parsed those bytes into keys, text, terminal events, query responses, or
protocol-specific messages.

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

The bytes are not decoded. For example, if the terminal reports the `A` key followed by an Up arrow,
the raw bytes may be:

```text
A ESC [ A
```

In byte form:

```text
\x41\x1b\x5b\x41
```

`InputBytes::as_bytes` returns that byte sequence exactly as read.

## What Remains Undecoded

The first input boundary does not classify or interpret:

- UTF-8 text;
- control bytes such as `Ctrl-C`;
- Escape-prefixed key sequences;
- Control Sequence Introducer messages;
- terminal query responses;
- paste boundaries;
- mouse, focus, graphics, clipboard, or vendor extension protocols.

Those behaviors belong to later parser, query-routing, and policy slices. Until those layers exist,
callers that need interpretation should treat `InputBytes` as raw terminal data and apply their own
temporary parser at the application boundary.

## Cleanup

Input ownership does not add cleanup beyond the session lifecycle. Starting a `TerminalSession`
enters raw mode. Call `TerminalSession::leave` during orderly shutdown so cooked-mode restoration
errors can be reported. Drop remains a best-effort fallback through the underlying terminal.

## Platform Support

The live terminal implementation currently supports Unix. Unsupported platforms expose the same
public types where possible and return `Error::Unsupported` for live terminal operations.
