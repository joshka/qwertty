# Terminal Session Reference

`TerminalSession` is the first application-facing owner above the low-level terminal device. It
opens or accepts a `Terminal`, enters raw mode, writes output bytes in method-call order, reads raw
input bytes, flushes explicitly, and restores cooked mode through an explicit `leave` path.

## Lifecycle

Use `TerminalSession::open` for the current controlling terminal, or `TerminalSession::from_terminal`
when embedding code or tests have already opened the terminal device.

Starting a session enters raw mode. Raw mode disables canonical input processing and local echo at
the operating-system terminal boundary. The first session slice does not enter the alternate screen,
hide the cursor, enable mouse tracking, enable bracketed paste, write graphics, touch the clipboard,
or change vendor-specific protocol state.

## Output Ordering

The session writes command bytes, raw bytes, and text bytes immediately in the order its methods are
called:

```rust,no_run
use qwertty::{ProtocolPosition, TerminalSession, commands};

# fn main() -> qwertty::Result<()> {
let mut session = TerminalSession::open()?;
session
    .command(commands::screen::clear())?
    .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))?
    .text("Ready\r\n")?
    .flush()?;
session.leave()?;
# Ok(())
# }
```

The example writes these bytes before flushing:

```text
ESC [ 2 J ESC [ 1 ; 1 H R e a d y CR LF
```

In byte form:

```text
\x1b[2J\x1b[1;1HReady\r\n
```

`TerminalSession::text` writes UTF-8 bytes verbatim. It does not escape control characters, remove
escape sequences, or enforce a text policy. Renderers that accept user-controlled text should apply
their own escaping policy before writing to the session.

## Input Bytes

`TerminalSession::read_input` reads one chunk of raw terminal input bytes into a caller-provided
buffer and returns those bytes as `InputBytes`. It does not parse keys, UTF-8, Escape sequences,
query responses, paste, mouse input, or vendor protocols. See the
[terminal input reference](crate::docs) for the input byte contract.

## Flush And Leave

`TerminalSession::flush` reports output flushing errors. Call it when prior writes must be visible
before later application work continues.

`TerminalSession::leave` consumes the session and restores cooked mode. Use it during orderly
shutdown so restoration errors can be handled. Dropping a session without `leave` still relies on
the underlying `Terminal` drop fallback, but drop-time restoration errors cannot be reported.

`leave` does not flush pending output for you. Flush explicitly when output visibility is part of
the user-facing behavior.

## Async Boundary

qwertty is an async-first terminal library, but `TerminalSession` stays runtime-neutral. It does
not add async methods that only wrap synchronous file reads or writes.

The first async public surface is `TokioTerminalSession`, a separate Tokio session owner behind an
optional `tokio` Cargo feature on Unix. It uses runtime-backed terminal reads and writes, preserves
output ordering, feeds input through `InputDecoder`, delivers decoded events without swallowing
unrelated input, and documents cancellation at the event-delivery boundary.

Keeping this boundary explicit avoids making every user compile Tokio and avoids adding a
runtime-agnostic async trait before one real runtime implementation proves the shape.

Enable the feature in `Cargo.toml`:

```toml
qwertty = { version = "0.0.0", features = ["tokio"] }
```

Then use `TokioTerminalSession` inside a Tokio runtime:

```rust,no_run
use qwertty::{ProtocolPosition, TokioTerminalSession, commands};

# async fn run() -> qwertty::Result<()> {
let mut session = TokioTerminalSession::open()?;
session.command(commands::screen::clear()).await?;
session
    .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))
    .await?;
session.text("Ready\r\n").await?;
session.flush().await?;
session.leave().await
# }
```

`TokioTerminalSession::next_event` reads from the terminal through Tokio readiness and returns the
next decoded `InputEvent`:

```rust,no_run
use qwertty::{InputEvent, TokioTerminalSession};

# async fn run() -> qwertty::Result<()> {
let mut session = TokioTerminalSession::open()?;

match session.next_event().await? {
    InputEvent::Text('q') => {}
    _ => {}
}

session.leave().await
# }
```

If a task waiting in `next_event` is canceled before another terminal read completes, the session
remains usable. Events already decoded from earlier reads stay queued for later calls. This
boundary does not add alternate screen cleanup, mouse mode, paste mode, graphics, clipboard, or
vendor protocol policy.

## Live Cursor Position Query

`TokioTerminalSession::request_cursor_position` is the first live query helper. It writes the
cursor position Device Status Report request, flushes output, and reads decoded events until it
matches `CSI row ; column R`:

```rust,no_run
use std::time::Duration;

use qwertty::TokioTerminalSession;

# async fn run() -> qwertty::Result<()> {
let mut session = TokioTerminalSession::open()?;
let report = session.request_cursor_position(Duration::from_secs(1)).await?;

assert!(report.row() > 0);
assert!(report.column() > 0);

session.leave().await
# }
```

The emitted request bytes are:

```text
\x1b[6n
```

Terminals commonly answer with:

```text
\x1b[row;columnR
```

The timeout bounds the whole request/response operation. When the timeout elapses, the method
returns `Error::QueryTimeout`. Unrelated decoded events that arrive before the report remain queued
for later `next_event` calls.

`TokioTerminalSession::request_terminal_status` uses the same session-owned boundary for terminal
status reports:

```rust,no_run
use std::time::Duration;

use qwertty::{TerminalStatus, TokioTerminalSession};

# async fn run() -> qwertty::Result<()> {
let mut session = TokioTerminalSession::open()?;
let report = session.request_terminal_status(Duration::from_secs(1)).await?;

assert_eq!(report.status(), TerminalStatus::Ready);

session.leave().await
# }
```

It emits:

```text
\x1b[5n
```

and waits for either:

```text
\x1b[0n
\x1b[3n
```

The timeout and preserved-input behavior are the same as the cursor-position helper.

These are still not general query routers. qwertty does not yet support multiple simultaneous live
queries, capability probing, or query registration.

## Query Routing Boundary

Live query routing currently belongs to `TokioTerminalSession`. The session owns the terminal
write, flush, runtime-backed read, decoder state, timeout, and preserved event queue needed for
typed query helpers.

The public API remains method-based for now. New live queries should start as typed session methods
that document the bytes they emit, the response shape they wait for, timeout behavior, and which
unrelated input stays visible through `next_event`.

An internal session-owned router may share mechanics between those helpers, but qwertty does not
yet expose a generic query router, concurrent query registry, capability probing API, or
runtime-agnostic async query trait.

## Platform Support

The live terminal implementation currently supports Unix. Unsupported platforms expose the same
public types where possible and return `Error::Unsupported` for live terminal operations.
