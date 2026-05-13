//! Builds a status line into a command buffer.

use qwertty::{CommandBuffer, ProtocolPosition, commands};

fn main() {
    let mut output = CommandBuffer::new();
    output
        .command(commands::cursor::save())
        .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))
        .command(commands::screen::erase_line())
        .text("build: running")
        .command(commands::cursor::restore());

    print!("{}", String::from_utf8_lossy(output.as_bytes()));
}
