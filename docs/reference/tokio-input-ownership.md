# Tokio Input Ownership And Query Handoff

`TokioTerminalSession` is qwertty's first async terminal owner. It does more than expose async
reads and writes: it owns the live terminal file descriptor, raw-mode lifecycle, `InputDecoder`
state, decoded event queue, and the first live query-routing boundary.

This guide explains how to use that ownership model without guessing from API details.

## One Input Owner

Treat one `TokioTerminalSession` as the single owner for terminal reads in a running application.

That one owner is responsible for:

- calling `next_event` to pull decoded input from the terminal;
- issuing live query helpers such as `request_cursor_position` and
  `request_terminal_status`;
- handling timeout and cancellation results;
- deciding how application code receives events after qwertty decodes them.

Do not set up competing terminal read loops that race the same terminal device or try to split live
queries and event reads across separate session owners. qwertty keeps query routing, decoded-event
buffering, and cancellation guarantees local by requiring `&mut self` access to the same session
owner.

If the rest of the application needs input in multiple places, keep one task as the terminal owner
and forward application-level events from there.

## Event Delivery And Live Queries

`TokioTerminalSession::next_event` and the live query helpers share one decoded event stream.

When a query helper reads terminal input before its matching response arrives:

- the matching response is consumed by the query helper;
- unrelated decoded input stays queued inside the session;
- later `next_event` calls still observe that unrelated input in order.

That behavior applies to the current live query helpers:

- `request_cursor_position`, which sends `CSI 6 n` and waits for `CSI row ; column R`;
- `request_terminal_status`, which sends `CSI 5 n` and waits for `CSI 0 n` or `CSI 3 n`.

This means an application loop can interleave ordinary event reads and narrow typed queries without
having to rebuild routing state around each request:

```rust,no_run
use std::time::Duration;

use qwertty::{InputEvent, TokioTerminalSession};

# async fn run() -> qwertty::Result<()> {
let mut session = TokioTerminalSession::open()?;

let position = session.request_cursor_position(Duration::from_secs(1)).await?;
assert!(position.row() > 0);

match session.next_event().await? {
    InputEvent::Text('q') => {}
    _ => {}
}

session.leave().await
# }
```

qwertty still does not expose a generic public query router, concurrent live queries, or capability
probing. Keep live query use narrow and session-owned.

For a small checked-in example that waits for a live query helper and then reads preserved
unrelated input through `next_event`, see `examples/tokio_preserved_unrelated_input.rs` in the
repository.

For a small checked-in example that waits for `request_terminal_status` and then reads preserved
unrelated input through `next_event`, see `examples/tokio_terminal_status_preserved_input.rs` in
the repository.

For a small checked-in example that waits for `request_terminal_status` and then reads a preserved
cursor-position report through `next_event`, see `examples/tokio_terminal_status_wrong_report.rs`
in the repository.

For a small checked-in example that waits for `request_terminal_status` and then reads preserved
unmatched query-shaped CSI through `next_event`, see
`examples/tokio_terminal_status_unmatched_query_input.rs` in the repository.

For a small checked-in example that opens a Tokio session, performs live queries, writes output,
and leaves explicitly, see `examples/tokio_terminal_queries.rs` in the repository.

For a small checked-in example centered on decoded event delivery through `next_event`, see
`examples/tokio_input_events.rs` in the repository.

For a small checked-in example that matches live query success, `Error::QueryTimeout`, and
`Error::ReadTerminal` explicitly, see `examples/tokio_query_error_handling.rs` in the repository.

## Cancellation And Timeouts

Cancellation is defined at the session boundary.

If a task waiting in `next_event` is canceled before another terminal read completes, the session
remains usable. Events that qwertty already decoded from earlier reads stay queued for later calls.

The live query helpers follow the same rule. If a task waiting in `request_cursor_position` or
`request_terminal_status` is canceled while the session is waiting for more terminal input:

- the session remains usable;
- bytes and events qwertty already owns remain available;
- unrelated decoded events already seen by the query stay queued for later `next_event` calls.

For a small checked-in example that starts a live query, cancels it explicitly, and continues
using the session, see `examples/tokio_query_cancellation.rs` in the repository.

Timeouts are explicit query errors. qwertty returns `Error::QueryTimeout` with the operation name
and caller-provided duration:

```rust,no_run
use std::time::Duration;

use qwertty::{Error, TokioTerminalSession};

# async fn run() -> qwertty::Result<()> {
let mut session = TokioTerminalSession::open()?;

match session.request_terminal_status(Duration::from_millis(100)).await {
    Ok(report) => {
        let _ = report;
    }
    Err(Error::QueryTimeout { operation, .. }) => {
        assert_eq!(operation, "terminal status query");
    }
    Err(err) => return Err(err),
}

session.leave().await
# }
```

The timeout only says the expected response was not seen before the deadline. It does not discard
already queued unrelated input.

The timeout also does not reserve the matching reply forever. If the expected cursor-position or
terminal-status reply reaches the session after the timeout has already been returned, the
timed-out helper does not consume it later. Under the current API, a later `next_event` call
receives that late reply through the normal decoded event stream, typically as `InputEvent::Csi(...)`.

For a small checked-in example that times out a live query and then handles a late reply through
`next_event`, see `examples/tokio_late_query_reply.rs` in the repository.

If the terminal path closes before a matching reply arrives, the helper returns a terminal read
error instead of waiting for the timeout. Under the current implementation, that error is
`Error::ReadTerminal` with an `UnexpectedEof` source.

qwertty also keeps typed reports local to the helper that asked for that report shape. If a live
cursor-position query sees a terminal-status report, or a live terminal-status query sees a
cursor-position report, the waiting helper does not consume the wrong report. A later
`next_event` call receives it through the normal decoded event stream.

For a small checked-in example that times out one live query helper and then handles another
helper's report through `next_event`, see `examples/tokio_wrong_report_query.rs` in the
repository.

The same is true for query-shaped CSI that does not match any current helper. Unsupported or
malformed query-shaped CSI remains in the normal decoded event stream instead of being swallowed by
the waiting helper.

For a small checked-in example that times out a live query and then handles unmatched query-shaped
CSI through `next_event`, see `examples/tokio_unmatched_query_input.rs` in the repository.

## Handoff To Another Program

When a TUI or terminal application needs to hand control to another program, such as an editor or
shell command, use an orderly leave-run-reopen sequence.

The conservative pattern is:

1. Finish any output that must be visible.
1. Call `flush` when visibility matters.
1. Call `leave` to restore cooked mode before starting the child program.
1. Run the child program.
1. Open a fresh `TokioTerminalSession` after the child exits.
1. Re-enter the application's draw and input loop.

Do not rely on drop-time cooked-mode restoration for a planned handoff. Drop is a best-effort
fallback and cannot report cleanup failure to the caller.

```rust,no_run
use qwertty::TokioTerminalSession;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let mut session = TokioTerminalSession::open()?;
session.text("opening child process\r\n").await?;
session.flush().await?;
session.leave().await?;

let status = tokio::task::spawn_blocking(move || {
    std::process::Command::new("sh")
        .arg("-lc")
        .arg("printf child-ran >/dev/null")
        .status()
})
.await??;
assert!(status.success());

let mut session = TokioTerminalSession::open()?;
session.text("redraw after handoff\r\n").await?;
session.flush().await?;
session.leave().await?;
# Ok(())
# }
```

If the application owns higher-level state such as screen contents, cursor position intent, or UI
mode, rebuild that state explicitly after reopening the session. qwertty restores cooked mode on
leave, but it does not preserve application rendering state across handoff.

## What This Guide Does Not Claim

This guide describes the current public contract only.

qwertty does not yet provide:

- multiple simultaneous live queries;
- a public generic query registry;
- paste parsing or Escape-timeout policy;
- resize event delivery;
- subprocess helpers that reopen sessions automatically;
- non-Tokio async runtime support.

Those later slices should extend this guide only when the public API actually grows.
