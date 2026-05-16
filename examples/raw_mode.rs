//! Open the terminal device, enter raw mode, write bytes, and restore cooked mode.
//!
//! This example uses the low-level device API directly. Full terminal applications should prefer
//! the later session layer once it exists because sessions will own protocol cleanup, input
//! routing, and async event-loop policy.

use qwertty::{CommandBuffer, ProtocolPosition, Terminal, commands};

fn main() -> qwertty::Result<()> {
    let mut terminal = Terminal::open()?;
    let size = terminal.size()?;

    terminal.set_raw_mode()?;

    let mut output = CommandBuffer::new();
    output
        .command(commands::screen::clear())
        .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))
        .text(format!(
            "raw mode active: {} columns x {} rows\r\n",
            size.columns(),
            size.rows()
        ));

    terminal.write_all(output.as_bytes())?;
    terminal.flush()?;
    terminal.set_cooked_mode()?;

    Ok(())
}
