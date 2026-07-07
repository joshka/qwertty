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

`TerminalSession::leave` replays the session's mode ledger: every reversible state change the
session made is undone in reverse enablement order, every step is attempted even after a failure,
and the first error is reported. The replay ends with a flush so restoration bytes never sit in a
buffer. Today the ledger holds raw-mode restoration; alternate screen, cursor visibility, mouse
mode, and paste mode join it in later slices.

The lifecycle is re-entrant: `leave` does not consume the session, and
`TerminalSession::enter` re-applies the recorded state afterwards. A line-editor-shaped caller
cycles the pair once per prompt over one long-lived session; each transition replays mode actions
only and never reopens the device, so cycling stays as cheap as the mode changes themselves.
Sessions also run headless over any `TerminalDevice` through `TerminalSession::from_device` — the
`session_cycles.rs` example drives the full lifecycle against a `FakeDevice`.

Restoration runs at most once per entered period. Whichever of `leave`, drop, or the panic-safe
restore handle runs first performs it; the others skip. Dropping an entered session still
restores the terminal, but drop-time failures cannot be reported.

Flush explicitly before `leave` when the visibility ordering of your own output matters.

## Panic-Safe Restore

On Unix, `TerminalSession::restore_handle` returns a `RestoreHandle`: a cheap, cloneable handle
that restores the terminal without borrowing the session, built for panic hooks.

```no_run
use qwertty::TerminalSession;

fn main() -> qwertty::Result<()> {
    let mut session = TerminalSession::open()?;

    let restore = session.restore_handle();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        _ = restore.restore();
        previous(info);
    }));

    session.leave()
}
```

The emergency path is precomposed: the session keeps the handle's teardown bytes current as its
state changes, so the hook only writes bytes and restores the captured terminal mode. Writes are
bounded, so a stalled terminal cannot hang the hook. A panic hook covers unwinding panics on any
thread; it does not run on `abort` or fatal signals. See the `panic_safe_restore.rs` example.

## Async Boundary

qwertty is an async-first terminal library, but `TerminalSession` stays runtime-neutral. It does
not add async methods that only wrap synchronous file reads or writes.

The first async public surface is `TokioTerminalSession`, a separate Tokio session owner behind an
optional `tokio` Cargo feature on Unix. It uses runtime-backed terminal reads and writes, preserves
output ordering, feeds input through `SemanticDecoder`, delivers decoded events without swallowing
unrelated input, and documents cancellation at the event-delivery boundary.

Keeping this boundary explicit avoids making every user compile Tokio and avoids adding a
runtime-agnostic async trait before one real runtime implementation proves the shape.

See [Tokio Input Ownership And Query Handoff](
crate::docs#tokio-input-ownership-and-query-handoff) for the single-owner model, query/event
interaction, timeout behavior, and orderly handoff pattern.

See [Checked-In Examples](crate::docs#checked-in-examples) for a durable index of the runnable
session, input, and query-routing examples shipped with the crate.

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
next decoded `Event`:

```rust,no_run
use qwertty::{Event, Key, TokioTerminalSession};

# async fn run() -> qwertty::Result<()> {
let mut session = TokioTerminalSession::open()?;

match session.next_event().await? {
    Event::Key(key) if key.key() == Key::Char('q') => {}
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

For a small checked-in Tokio example that waits for a live query helper and then reads preserved
unrelated input through `next_event`, see `examples/tokio_preserved_unrelated_input.rs` in the
repository.

For a small checked-in Tokio example that waits for `request_terminal_status` and then reads
preserved unrelated input through `next_event`, see
`examples/tokio_terminal_status_preserved_input.rs` in the repository.

For a small checked-in Tokio example that waits for `request_terminal_status` and then reads a
preserved cursor-position report through `next_event`, see
`examples/tokio_terminal_status_wrong_report.rs` in the repository.

For a small checked-in Tokio example that waits for `request_terminal_status` and then reads
preserved unmatched query-shaped CSI through `next_event`, see
`examples/tokio_terminal_status_unmatched_query_input.rs` in the repository.

For a small checked-in Tokio example that starts a live query, cancels it explicitly, and keeps
using the session afterward, see `examples/tokio_query_cancellation.rs` in the repository.

For a small checked-in Tokio example that starts `request_terminal_status`, cancels it explicitly,
and keeps using the session afterward, see
`examples/tokio_terminal_status_cancellation.rs` in the repository.

If the terminal path closes before any matching report arrives, the helper returns
`Error::ReadTerminal` with an `UnexpectedEof` source instead of waiting until the timeout.

If the matching report arrives after that timeout has already been returned, the timed-out helper
does not claim it later. Under the current API, a later `next_event` call sees that late reply
through the ordinary decoded input path, as a lossless `Event::Syntax(...)` CSI passthrough.

The same rule applies to typed reports that belong to some other helper. If a cursor-position
query sees `CSI 0 n` or `CSI 3 n` while it is still waiting for `CSI row ; column R`, the
cursor-position helper leaves that terminal-status report in the ordinary decoded input path.

The helper also leaves query-shaped CSI input alone when that input is not a valid
cursor-position report at all. Unsupported or malformed query-shaped CSI remains available through
the ordinary decoded input path.

`TokioTerminalSession::request_terminal_status` uses the same session-owned boundary for terminal
status reports:

```rust,no_run
use std::time::Duration;

use qwertty::TokioTerminalSession;
use qwertty::report::TerminalStatus;

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

Closed-terminal behavior is the same as the cursor-position helper as well: if the terminal path
reaches end-of-file before a status reply arrives, the helper returns `Error::ReadTerminal` with
an `UnexpectedEof` source instead of a timeout.

Late matching replies follow the same rule. After a timeout, a later `CSI 0 n` or `CSI 3 n`
reply is delivered through the ordinary decoded input path instead of being consumed by the
timed-out helper.

For a small checked-in Tokio example that times out a live query and then handles a late reply
through `next_event`, see `examples/tokio_late_query_reply.rs` in the repository.

Likewise, if a terminal-status query sees a cursor-position report while it is waiting for a
status reply, that cursor-position report remains available through the ordinary decoded input
path instead of being consumed as a status result.

For a small checked-in Tokio example that times out one live query helper and then handles another
helper's report through `next_event`, see `examples/tokio_wrong_report_query.rs` in the
repository.

Unsupported or malformed query-shaped CSI follows the same rule. If the input does not form a
valid terminal-status report, the helper leaves it in the ordinary decoded input path.

For a small checked-in Tokio example that times out a live query and then handles unmatched
query-shaped CSI through `next_event`, see `examples/tokio_unmatched_query_input.rs` in the
repository.

These are still not general query routers. qwertty does not yet support multiple simultaneous live
queries, capability probing, or query registration.

For a small checked-in Tokio example that matches live query success, timeout, and terminal read
failure explicitly, see `examples/tokio_query_error_handling.rs` in the repository.

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

The [Tokio Input Ownership And Query Handoff](
crate::docs#tokio-input-ownership-and-query-handoff) guide shows how this boundary fits into a
real event loop and child-process handoff.

## Platform Support

The live terminal implementation currently supports Unix. Unsupported platforms expose the same
public types where possible and return `Error::Unsupported` for live terminal operations.

See [Platform Support](crate::docs#platform-support) for the current Unix-first support boundary
and the documented unsupported behavior on other platforms.
