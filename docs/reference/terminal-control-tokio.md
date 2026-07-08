# Live Query Helpers (Tokio)

The [Terminal Control Reference](crate::docs#terminal-control-reference) documents the encode-only
command helpers that build query request bytes. With the optional `tokio` feature on Unix,
`TokioTerminalSession` turns those requests into live query helpers that write the request, flush
output, wait for the matching report, and apply a caller-provided timeout. This section is included
only when the `tokio` feature is enabled, so its `TokioTerminalSession` examples compile only in that
configuration.

`TokioTerminalSession::request_cursor_position` pairs with `commands::cursor::request_position`:

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

`TokioTerminalSession::request_terminal_status` pairs with `commands::terminal::request_status`:

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

Unrelated decoded events that arrive before the matching report remain available through
`TokioTerminalSession::next_event`. This is still not a general query router: qwertty does not yet
support multiple simultaneous live queries, capability probing, or query registration. See
[Terminal Session: Async Boundary And Live
Queries](crate::docs#terminal-session-async-boundary-and-live-queries) and [Tokio Input Ownership
And Query Handoff](crate::docs#tokio-input-ownership-and-query-handoff) for the session-owned model.
