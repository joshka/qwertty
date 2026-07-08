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

The public surface also includes:

- **Capability detection** — `Capabilities`, built by `TokioTerminalSession::probe_capabilities`
  (with the `tokio` feature on Unix), reports synchronized output, grapheme clustering, in-band
  resize, bracketed paste, kitty keyboard flags, terminal identity, and env-inferred hyperlink and
  truecolor support, each with evidence of how it was learned. See the
  [capability model reference](docs/reference/capability-model.md).
- **Security policy** — `Policy` and `PolicyGate` gate side-effecting and exfiltrating features
  (clipboard write/read, notifications, file transfer, mux passthrough) so a program's output
  cannot silently reach the clipboard or the filesystem. See the
  [session lifecycle reference](docs/reference/terminal-session.md#security-policy).
- **Kitty keyboard** — `commands::terminal::push_kitty_keyboard_flags`/`pop_kitty_keyboard_flags`
  request progressive-enhancement key reporting, verified by readback rather than assumed granted.
  See [Input Modes](docs/reference/terminal-session.md#input-modes).
- **Mouse, focus, paste, and resize events** — `enable_mouse`, `enable_focus_events`,
  `enable_bracketed_paste`, and `enable_in_band_resize` turn on the terminal reporting modes the
  decoder turns into `Event::Mouse`, `Event::Focus`, `Event::Paste`, and `Event::Resize`. See
  [Input Modes](docs/reference/terminal-session.md#input-modes) and the
  [terminal control reference](docs/reference/terminal-control.md).

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

See `qwertty::docs` for the checked-in examples reference page on docs.rs. It groups the current
examples by purpose and points at the smallest runnable example for each session, input, and
query-routing contract.

See `qwertty::docs` for the release-blocking examples reference page as well. It identifies which
checked-in examples are part of the `0.1.0` release evidence instead of treating every example as
equally release-critical.

See `qwertty::docs` for the platform support reference page as well. It explains the current
Unix-first live terminal boundary and what unsupported platforms return today.

See `qwertty::docs` for the release-readiness reference page too. It summarizes the current docs,
examples, validation gates, and policy boundaries that should be true before publication stops
being future work.

See `qwertty::docs` for the release checklist reference page as well. It turns that release
readiness posture into the concrete maintainer checklist to use before changing `publish = false`.

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
