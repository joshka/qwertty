# qwertty

qwertty is a Rust library for building terminal applications that need explicit terminal ownership,
ordered output, input handling, and policy-aware terminal features.

The library is being developed in small public slices. The first slices establish project quality
standards, then add command encoding, terminal device access, session lifecycle management, input,
queries, and capability policy.

## Status

qwertty has an encode-only command foundation, a Unix terminal device layer, a small terminal
session lifecycle, raw terminal input bytes, basic input events, and a small stateful input decoder.
It can build terminal output bytes, open the current terminal, manage raw mode, query terminal
size, write ordered session output, read input bytes, classify simple UTF-8 text/control/key input
across chunks, preserve complete CSI input syntax, parse and match cursor position reports, parse
terminal status reports, flush explicitly, and leave with reported cleanup errors. With the optional
`tokio` feature on Unix, it also exposes a Tokio-backed session owner for runtime-backed reads,
writes, decoded input events, explicit cleanup, live cursor position queries, and live terminal
status queries. It does not include a general terminal query router yet.

## Small Example

```rust
use qwertty::{CommandBuffer, ProtocolPosition, commands};

let mut output = CommandBuffer::new();
output
    .command(commands::screen::clear())
    .command(commands::cursor::move_to(ProtocolPosition::new(3, 5)))
    .text("Ready");

assert_eq!(output.as_bytes(), b"\x1b[2J\x1b[3;5HReady");
```

## Session Example

```rust,no_run
use qwertty::{ProtocolPosition, TerminalSession, commands};

fn main() -> qwertty::Result<()> {
    let mut session = TerminalSession::open()?;
    session
        .command(commands::screen::clear())?
        .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))?
        .text("Ready\r\n")?
        .flush()?;
    session.leave()
}
```

## Tokio Session Example

Enable the optional `tokio` feature to use the Tokio-backed session owner on Unix:

```rust,no_run
use qwertty::{ProtocolPosition, TokioTerminalSession, commands};

async fn run() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;
    session.command(commands::screen::clear()).await?;
    session
        .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))
        .await?;
    session.text("Ready\r\n").await?;
    let position = session
        .request_cursor_position(std::time::Duration::from_secs(1))
        .await?;
    session.text(format!("cursor: {:?}\r\n", position.position()))
        .await?;
    session.flush().await?;
    session.leave().await
}
```

See `examples/tokio_terminal_queries.rs` for a small checked-in Tokio example that opens a session,
issues live queries, and leaves explicitly.

See `examples/tokio_input_events.rs` for a small checked-in Tokio example centered on
`TokioTerminalSession::next_event`.

See `examples/tokio_query_error_handling.rs` for a small checked-in Tokio example that matches
live query success, timeout, and terminal read failure explicitly.

See `examples/tokio_query_cancellation.rs` for a small checked-in Tokio example that starts a live
query, cancels it explicitly, and continues using the session.

See `examples/tokio_late_query_reply.rs` for a small checked-in Tokio example that times out a
live query and then treats a late reply as ordinary decoded input.

See `examples/tokio_wrong_report_query.rs` for a small checked-in Tokio example that times out one
live query helper and then treats another helper's report as ordinary decoded input.

## Project Shape

- User-facing APIs should be practical before they are broad.
- Examples should show realistic terminal workflows.
- Public APIs should include Rustdoc that explains relevant errors, invariants, safety, policy, or
  protocol behavior.
- Maintainer details live under `docs/` instead of the first reading path.

## Contributing

Use `just check` to run the local gate. See [docs/workflow.md](docs/agent/workflow.md) for the
development workflow and [docs/roadmap.md](docs/roadmap.md) for the planned order of work. The API
docs include the terminal protocol reference in `qwertty::docs`.
