# qwertty

[![Crate Badge]][Crate]
[![Docs Badge]][Docs]
[![CI Badge]][CI]
[![License Badge]][License]
[![Deps Badge]][Dependency Status]

qwertty is a Rust library for building terminal applications that need explicit terminal ownership,
ordered output, input handling, and policy-aware terminal features.

qwertty is Unix-first and ships both a synchronous session owner and an optional Tokio-backed async
session owner behind the `tokio` feature, so an application can pick the runtime shape it needs
without qwertty imposing one.

## Status

qwertty has a runtime-neutral command encoding layer, a Unix terminal device layer, a synchronous
terminal session (`TerminalSession`) and an optional Tokio-backed async session
(`TokioTerminalSession`), a total lossless input syntax layer, and a semantic decoder that maps
input to typed `Event` values.

It can build terminal output bytes, open the current terminal, manage raw mode through a mode
ledger, query terminal size, write ordered session output, read raw input bytes or decoded events,
classify UTF-8 text/control/key input across chunks, preserve complete CSI/OSC/DCS syntax, and parse
cursor-position and terminal-status reports. Sessions are re-entrant (repeated enter/leave cycles,
the way a line editor hands the terminal back between prompts) and panic-safe: `RestoreHandle`
restores cooked mode from a panic hook even if the program never reaches its own cleanup code.

The public surface also includes:

- **Sync and async sessions** — `TerminalSession::request_cursor_position` and
  `request_terminal_status` answer live terminal queries with a blocking poll loop and no async
  runtime; `TokioTerminalSession` answers the same queries asynchronously and adds decoded
  `next_event` delivery. Both drive the same sans-io query correlator, so the query-routing
  contracts (timeout, cancellation, preserved input, wrong-report, unmatched input) are identical
  either way. See the [session lifecycle reference](docs/reference/terminal-session.md) and its
  [Tokio tail](docs/reference/terminal-session-tokio.md).
- **Suspend, resume, and handoff** — `TokioTerminalSession::suspend`/`resume(flush_input)` restore
  the terminal and re-enter raw mode around a `SIGTSTP`/`SIGCONT` job-control cycle, and
  `run_detached(f)` hands the terminal to a synchronous child (an `$EDITOR`, a pager, a subshell)
  and reclaims it afterward, resyncing termios and readiness either way. qwertty installs no signal
  handler itself; the app owns the `SIGTSTP`/`SIGCONT` wiring. See
  [Tokio Input Ownership And Query Handoff](docs/reference/tokio-input-ownership.md).
- **Terminal-relevant signals and resize** — `TokioTerminalSession::signals()` returns a
  `SignalStream` of typed `TerminalSignal::Suspend`/`Continue`/`Terminate`/`Interrupt` events for
  `SIGTSTP`/`SIGCONT`/`SIGTERM`/`SIGINT`, and `resize_stream()` returns a `ResizeStream` fallback
  for `SIGWINCH` alongside in-band coalesced `Event::Resize` delivery.
- **Security policy** — `Policy` and `PolicyGate` gate side-effecting and exfiltrating features
  (clipboard write/read, notifications, file transfer, mux passthrough) behind `restricted`,
  `interactive`, and `trusted` presets, so a program's output cannot silently reach the clipboard or
  the filesystem. Both sessions expose a gated `set_clipboard`, and a denied gate is a typed
  `Error::PolicyDenied` naming the gate. See the
  [session lifecycle reference](docs/reference/terminal-session.md#security-policy).
- **Kitty keyboard** — `push_kitty_keyboard` (on both the sync and Tokio sessions) sets
  progressive-enhancement key reporting flags directly, and
  `TokioTerminalSession::request_kitty_keyboard` verifies the granted subset by readback rather than
  assuming the request was honored. See [Input Modes](docs/reference/terminal-session.md#input-modes).
- **Capability probing** — `Capabilities`, built by `TokioTerminalSession::probe_capabilities` (with
  the `tokio` feature on Unix), reports synchronized output, grapheme clustering, in-band resize,
  bracketed paste, kitty keyboard flags, terminal identity, and env-inferred hyperlink and truecolor
  support, each with evidence of how it was learned — probed, inferred, or unknown, never assumed
  unsupported. See the [capability model reference](docs/reference/capability-model.md).
- **Terminal acquisition observability** — `TokioTerminalSession::acquisition()` reports how the
  controlling terminal was reached on macOS, for diagnostics and support requests.
- **Mouse, focus, paste, and resize events** — `enable_mouse`, `enable_focus_events`,
  `enable_bracketed_paste`, and `enable_in_band_resize` turn on the terminal reporting modes the
  decoder turns into `Event::Mouse`, `Event::Focus`, `Event::Paste`, and `Event::Resize`. See
  [Input Modes](docs/reference/terminal-session.md#input-modes) and the
  [terminal control reference](docs/reference/terminal-control.md).
- **Sequence database** — `db/` is a hand-curated, machine-validated database of terminal control
  sequences (375 entries across 16 families: ECMA-48, DEC, xterm, kitty, iTerm2, OSC, and vendor
  DCS), each with citations and byte-exact fixtures. The `qdb` tool validates the database and
  renders a live-capture conformance matrix (`db/caniuse.md`) from headless tmux and betamax
  (libghostty) captures. See [db/README.md](db/README.md).

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

See [CONTRIBUTING.md](CONTRIBUTING.md) for the contribution guidelines.

## License

Copyright (c) Josh McKinney

This project is licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the
work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.

[Crate]: https://crates.io/crates/qwertty
[Crate Badge]: https://img.shields.io/crates/v/qwertty?logo=rust&style=flat
[Docs]: https://docs.rs/qwertty
[Docs Badge]: https://img.shields.io/docsrs/qwertty?logo=rust&style=flat
[CI]: https://github.com/joshka/qwertty/actions/workflows/ci.yml
[CI Badge]: https://github.com/joshka/qwertty/actions/workflows/ci.yml/badge.svg
[License]: ./LICENSE-MIT
[License Badge]: https://img.shields.io/crates/l/qwertty?style=flat
[Dependency Status]: https://deps.rs/repo/github/joshka/qwertty
[Deps Badge]: https://deps.rs/repo/github/joshka/qwertty/status.svg?style=flat
