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

qwertty is an async-first terminal library, but this first session surface stays runtime-neutral. It
does not add async methods that only wrap synchronous file reads or writes. Async terminal I/O
belongs in the slice that introduces runtime-owned reads, writes, input events, or query response
routing.

Keeping this boundary explicit avoids committing to a runtime dependency or trait shape before the
event model proves what callers need.

## Platform Support

The live terminal implementation currently supports Unix. Unsupported platforms expose the same
public types where possible and return `Error::Unsupported` for live terminal operations.
