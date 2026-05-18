# Platform Support

qwertty's current public surface is intentionally uneven by platform.

The library is Unix-first today. It exposes the same public types where possible on other
platforms, but live terminal operations that are not implemented yet return
`Error::Unsupported`.

This page is the durable place to understand that boundary without reading implementation files.
The corresponding maintainer-facing decision is
[ADR 0013: Platform Support Policy](../adr/0013-platform-support-policy.md).

## What Works Today

### Runtime-Neutral Command And Protocol Types

These APIs are platform-neutral because they only build or interpret bytes in memory:

- `Command` and `CommandBuffer`
- terminal command helpers under `commands`
- `InputBytes`, `InputDecoder`, and `InputEvent`
- cursor-position and terminal-status report parsing

Those types do not open a live terminal device, enter raw mode, or depend on Tokio.

### Unix Terminal Ownership

The live terminal device and session owners are currently implemented on Unix:

- `Terminal`
- `TerminalSession`
- `TokioTerminalSession` behind the optional `tokio` feature

On Unix today, qwertty can:

- open the current controlling terminal or a test-provided terminal path;
- capture the original terminal mode;
- enter raw mode and restore cooked mode;
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

The `tokio` feature does not widen platform support by itself. It is still a Unix-only live
terminal surface today.

## Unsupported Platforms

On platforms without a live terminal implementation yet, qwertty keeps the type surface where it
can and fails explicitly at the operation boundary.

That means callers may still compile code that mentions the public terminal types, but operations
such as these return `Error::Unsupported`:

- `Terminal::open`
- `Terminal::open_path`
- `Terminal::size`
- `Terminal::set_raw_mode`
- `Terminal::set_cooked_mode`
- `Terminal::write_all`
- `Terminal::read`
- `Terminal::flush`

Higher-level live terminal APIs built on that device boundary inherit the same unsupported behavior
instead of pretending a broader platform contract.

## What This Means For Callers

- Use command and parser types freely across platforms when you only need byte building or byte
  interpretation.
- Treat live terminal ownership as Unix-only until qwertty documents another implemented platform.
- Match `Error::Unsupported` at the application boundary when you expose optional live terminal
  behavior on platforms that qwertty does not support yet.
- Do not infer broader support from the existence of a type alone; the support boundary is defined
  by documented behavior and validation, not by naming symmetry.

## Related References

- [Terminal Device Reference](crate::docs#terminal-device-reference)
- [Terminal Session Reference](crate::docs#terminal-session-reference)
- [Terminal Input Reference](crate::docs#terminal-input-reference)
- [Tokio Input Ownership And Query Handoff](crate::docs#tokio-input-ownership-and-query-handoff)
