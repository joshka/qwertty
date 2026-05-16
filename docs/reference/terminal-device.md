# Terminal Device Reference

qwertty's terminal device layer is the low-level boundary between Rust code and the operating
system terminal. It opens a terminal device, captures its original mode, can enter raw mode, can
restore cooked mode, can query the current terminal size, and can write bytes.

It does not own the full application lifecycle. A later session layer will own alternate screen,
cursor cleanup, feature cleanup, input parsing, query routing, and async event-loop policy.

## Current Terminal

[`Terminal::open`](crate::Terminal::open) opens the process controlling terminal. On Unix this is
`/dev/tty`, which addresses the terminal device associated with the process instead of wrapping
process standard input or standard output.

Opening the terminal captures its current mode immediately. That captured mode is the target for
[`Terminal::set_cooked_mode`](crate::Terminal::set_cooked_mode) and for best-effort drop-time
restoration.

## Raw And Cooked Mode

Raw mode is operating-system terminal state. In cooked mode, the terminal driver can line-buffer
input, echo typed bytes, and interpret control characters. In raw mode, later input code can receive
terminal bytes directly.

Use [`Terminal::set_raw_mode`](crate::Terminal::set_raw_mode) to enter raw mode. Use
[`Terminal::set_cooked_mode`](crate::Terminal::set_cooked_mode) during orderly shutdown so
restoration errors are visible to the caller.

`Terminal` also attempts to restore cooked mode when it is dropped. This is only a fallback because
drop cannot report cleanup errors.

## Size

[`Terminal::size`](crate::Terminal::size) returns a [`TerminalSize`](crate::TerminalSize). The
values are measured in terminal cells:

- columns are the horizontal cell count;
- rows are the vertical cell count.

The returned size is a snapshot. Future resize events belong to the session and input layers.

## Byte Output

[`Terminal::write_all`](crate::Terminal::write_all) writes bytes exactly as provided. It does not
escape text, interpret commands, or enforce policy. Use [`CommandBuffer`](crate::CommandBuffer) when
combining command helpers and text before writing:

```no_run
use qwertty::{CommandBuffer, ProtocolPosition, Terminal, commands};

fn main() -> qwertty::Result<()> {
    let mut terminal = Terminal::open()?;
    terminal.set_raw_mode()?;

    let mut output = CommandBuffer::new();
    output
        .command(commands::screen::clear())
        .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))
        .text("Ready\r\n");

    terminal.write_all(output.as_bytes())?;
    terminal.flush()?;
    terminal.set_cooked_mode()?;

    Ok(())
}
```

## Platform Status

The first terminal device implementation is Unix-only. Other platforms return a documented
unsupported error until their device behavior is implemented.
