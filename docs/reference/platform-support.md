# Platform Support

qwertty owns a live terminal on Unix and on Windows. The two platforms share one public surface —
the same `Terminal`, `TerminalSession`, and `TokioTerminalSession` types, the same event vocabulary,
the same query correlator — over two different operating-system backends. A few lifecycle operations
exist only where the operating system has the concept behind them; those return `Error::Unsupported`
on the other platform rather than pretending. This page is the durable place to understand that
boundary without reading implementation files.

The maintainer-facing decisions are
[ADR 0013: Platform Support Policy](https://github.com/joshka/qwertty/blob/main/docs/adr/0013-platform-support-policy.md)
and
[ADR 0022: Windows Console Support Model](https://github.com/joshka/qwertty/blob/main/docs/adr/0022-windows-console-support-model.md).

## Windows At A Glance

The Windows backend is a console *client*: it opens `CONIN$`/`CONOUT$`, requires virtual-terminal
mode (`ENABLE_VIRTUAL_TERMINAL_PROCESSING` on output, `ENABLE_VIRTUAL_TERMINAL_INPUT` on input), and
speaks the same VT byte stream the Unix backend does, so the decoder, command encoders, and
capability model are shared unchanged. It is VT-only with no legacy-console rendering path.

- **Floor:** Windows 10 build 1809 (10.0.17763), 64-bit. This is the ConPTY-era baseline; there is
  no support below it and no legacy-console fallback.
- **Async transport:** a Windows console input handle is not pollable like a Unix file descriptor,
  so `TokioTerminalSession` reads through a worker thread that waits on the console input handle and
  a cancellation event, then feeds a channel — cancel-safe by the same "state lives on the struct"
  contract as the Unix `AsyncFd` path.
- **Resize:** delivered in-band as `Event::Resize` (synthesized from console
  `WINDOW_BUFFER_SIZE_EVENT` records); there is no separate resize stream on Windows.

## What Works Today

### Runtime-Neutral Command And Protocol Types

These APIs are platform-neutral because they only build or interpret bytes in memory:

- `Command` and `CommandBuffer`
- terminal command helpers under `commands`
- `InputBytes`, the raw read value
- `SyntaxParser`/`SyntaxToken` and `SemanticDecoder`/`Event`, the input decoding layers
- cursor-position and terminal-status report parsing under `report`

Those types do not open a live terminal device, enter raw mode, or depend on Tokio.

### Terminal Ownership (Unix And Windows)

The live terminal device and session owners are implemented on both Unix and Windows:

- `Terminal`
- `TerminalSession`
- `TokioTerminalSession` behind the optional `tokio` feature

On both platforms, qwertty can:

- open the current terminal (the controlling terminal on Unix; the process console via
  `CONIN$`/`CONOUT$` on Windows);
- capture the original terminal mode and restore it on teardown, drop, or a panic hook;
- enter raw mode and restore cooked mode (termios on Unix; console modes plus the output codepage on
  Windows);
- query terminal size;
- write ordered output and flush explicitly;
- read raw input bytes and decoded input events;
- issue live cursor-position and terminal-status queries through `TokioTerminalSession`.

## What The Tokio Feature Adds

Enable the optional `tokio` feature when a Unix application needs runtime-backed terminal reads and
writes:

```toml
qwertty = { version = "0.0.0", features = ["tokio"] }
```

That feature adds `TokioTerminalSession`, which owns:

- async ordered output;
- decoded `next_event` delivery;
- live cursor-position query routing;
- live terminal-status query routing;
- query timeout, cancellation, and preserved-input behavior documented in the session references.

On Windows the `tokio` feature adds the same `TokioTerminalSession`, driven by the console worker
thread described above instead of a reactor registration.

## Where The Platforms Differ

A few session operations map to an operating-system concept that exists on one platform and not the
other. Where the concept is absent, the method returns `Error::Unsupported` rather than approximating
it:

| Operation                          | Unix                                    | Windows                                                      |
| ---------------------------------- | --------------------------------------- | ------------------------------------------------------------ |
| `suspend` / `resume`               | `SIGTSTP`/`SIGCONT` job control         | `Unsupported` (Windows has no job control)                   |
| `signals()`                        | Suspend, Continue, Terminate, Interrupt | Terminate, Interrupt only (no suspend/continue)              |
| `resize_stream()`                  | `SIGWINCH` fallback stream              | `Unsupported` — resize is delivered in-band via `next_event` |
| `run_detached` (`$EDITOR` handoff) | supported                               | supported                                                    |
| `acquisition()`                    | reports the controlling-terminal branch | not applicable (the console is opened directly)              |

Everything else — raw/cooked mode, size, ordered output, decoded events, the query family
(`request_cursor_position`, `request_terminal_status`, `request_kitty_keyboard`,
`probe_capabilities`), `RestoreHandle`, and `run_detached` — behaves the same on both platforms.

## Input Enhancement On Windows

Windows keyboard input arrives as VT byte sequences under `ENABLE_VIRTUAL_TERMINAL_INPUT`, decoded by
the same parser as on Unix. Two enhancements sit on top:

- **Kitty keyboard protocol** — the preferred enhancement, supported by Windows Terminal 1.25+ and
  probed by readback like everywhere else.
- **win32-input-mode** (`CSI ? 9001 h`) — qwertty *decodes* these sequences, but enabling the mode is
  a policy-gated opt-in, not a default: it is all-or-nothing and absent under some hosts. See the
  [keybinding portability reference](crate::docs::keybinding_portability) for what each host can and
  cannot distinguish.

## Other Platforms

The platform-neutral command and parser types build everywhere, including `wasm32-unknown-unknown`.
On a target with no live terminal backend, the `Terminal` operations (`open`, `size`, `set_raw_mode`,
`read`, `write_all`, …) return `Error::Unsupported`, and higher-level session APIs inherit that
boundary. A clean cross-compile is evidence the type surface stays honest, not evidence of live
terminal support.

## How Each Platform Is Validated

Support means validated behavior, not a naming coincidence. CI cross-compiles and lints the library
(including its tests) for `x86_64-pc-windows-msvc` and `wasm32-unknown-unknown`, runs the full Unix
test suite on Linux and macOS runners, and runs the Windows test suite — including live console tests
that open a real console, exercise the async read path through injected input records, and verify
mode restoration — on a `windows-latest` runner. Interactive validation on the full matrix of Windows
terminals (Windows Terminal, classic conhost, wezterm) and IME/CJK composition under VT input is an
ongoing effort tracked for the Windows tier.

That interactive pass is driven by the `input_event_viewer` example (the first example that runs on
both Unix and Windows), which prints every decoded event so a human can see what the backend
produces; the step-by-step matrix and checklist live in the [Windows validation
runbook](https://github.com/joshka/qwertty/blob/main/docs/development/windows-validation.md).

## What This Means For Callers

- Use command and parser types freely across platforms when you only need byte building or byte
  interpretation.
- Treat live terminal ownership as available on Unix and Windows; match `Error::Unsupported` where
  the table above marks an operation platform-specific.
- Do not infer support from the existence of a type alone; the support boundary is defined by
  documented behavior and validation, not by naming symmetry.

## Related References

- [Terminal Device](crate::docs::terminal_device)
- [Terminal Session](crate::docs::terminal_session)
- [Terminal Input](crate::docs::terminal_input)
- Tokio Input Ownership And Query Handoff (`crate::docs::tokio_input_ownership`, with the `tokio`
  feature enabled)
