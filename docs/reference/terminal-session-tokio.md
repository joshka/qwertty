# Terminal Session: Async Boundary And Live Queries

This is the Tokio-backed tail of the [Terminal Session Reference](crate::docs#terminal-session-reference).
It documents `TokioTerminalSession`, the async session owner behind the optional `tokio` Cargo
feature on Unix, so it is included in the crate docs only when that feature is enabled.

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
remains usable. Events already decoded from earlier reads stay queued for later calls. The Tokio
session enables and tears down mouse, focus, and bracketed-paste modes (see
[Input Modes](crate::docs#input-modes)) and alternate screen and cursor visibility (see [Screen And
Cursor Lifecycle](crate::docs#screen-and-cursor-lifecycle)); it does not yet add graphics,
clipboard, or vendor protocol policy.

## Terminal Acquisition Observability

On Unix, `TokioTerminalSession::open` reaches the controlling terminal through a three-branch
fallback. It prefers duplicating the inherited read-write standard input (the robust primary, and
the branch that works under tmux and around the macOS kqueue restriction on freshly opened
controlling-terminal descriptors); failing that it opens the resolved specific device path fresh;
and as a last resort it opens the `/dev/tty` alias. Every branch yields a working session, so which
one won is otherwise invisible.

`TokioTerminalSession::acquisition` makes that outcome observable. It returns a `TerminalAcquisition`
so a caller can log which branch produced the session or show it in a status view, without having to
reconstruct the decision. It is read-only observability, not a control to branch on. Sessions built
from an already-opened device through `from_device` return `None`, because no fallback runs and no
branch is taken.

```rust,no_run
use qwertty::TokioTerminalSession;

# async fn run() -> qwertty::Result<()> {
let session = TokioTerminalSession::open()?;
if let Some(acquisition) = session.acquisition() {
    eprintln!("acquired controlling terminal: {acquisition}");
}
session.leave().await
# }
```

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

## Suspend And Resume Lifecycle

A full-screen application should drop cleanly to the shell on `Ctrl-Z` and repaint correctly when
brought back with `fg`. `TokioTerminalSession` exposes the two halves of that lifecycle as explicit
operations the application drives from its own job-control integration. qwertty installs no signal
handler of its own, so the application owns the `SIGTSTP`/`SIGCONT` wiring and decides exactly when
to call each method (design 01 §4).

`suspend` restores the terminal to a clean cooked state (replaying the mode ledger's resets while
keeping the entries so resume can re-apply them), disarms the panic-safe restore handle so the
emergency hook cannot fire while the process is stopped, then sends `SIGTSTP` to the whole process
group so the controlling shell regains the terminal. Before signalling it checks the process group
defensively: a session leader with no job-control shell to resume it is a degenerate group, and
`suspend` returns `Error::DegenerateProcessGroup` rather than stopping into a state nothing will
continue (FM-G7).

`resume` re-establishes terminal state the shell may have scrambled, in a fixed order — termios
resync first, then flags resync. It re-enters raw mode and every recorded mode with a bounded retry
(the shell races the returning process for the terminal), re-asserts the readiness descriptor's
non-blocking flag (the shell may have cleared it, and the async reactor requires it), optionally
flushes stale input, and queues a synthetic `Event::Resize` so the application repaints at whatever
size the terminal is now — the window may have been resized while the process was stopped.

### The `flush_input` Choice

`resume` takes a `flush_input` flag, so dropping stale typeahead is the caller's decision rather
than a fixed policy. While the process is stopped a user may type at the shell; those bytes sit in
the terminal's input queue. Passing `flush_input: true` `tcflush`es them so they are not delivered
to the application as if typed into it — the usual choice for a full-screen editor. Passing
`false` keeps them, which a REPL replaying a partially typed command buffer may prefer. Only the
input queue is affected; queued output the session still owes is untouched.

```no_run
use qwertty::{Event, Key, TokioTerminalSession};

# async fn run() -> qwertty::Result<()> {
let mut session = TokioTerminalSession::open()?;
match session.next_event().await? {
    Event::Key(key) if matches!(key.key(), Key::Char('z')) => {
        // Restore the terminal, disarm the emergency hook, and stop the process group. This call
        // returns once the process is continued again (a SIGCONT the app waits for elsewhere).
        session.suspend().await?;
        // On return, re-enter modes, re-assert non-blocking, drop stale shell typeahead, and queue
        // a synthetic resize so the next `next_event` repaints at the current size.
        session.resume(true).await?;
    }
    _ => {}
}
# session.leave().await
# }
```

For a runnable Tokio example that suspends on a key and resumes on `SIGCONT`, see
`examples/suspend_resume.rs` in the repository.

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
