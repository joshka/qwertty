//! Encoding behavior tests.

use qwertty::{Command, CommandBuffer, ProtocolPosition, commands};

#[test]
fn command_encodes_raw_bytes() {
    let command = Command::raw(b"\x1b[2J");
    let mut bytes = Vec::new();

    command.encode(&mut bytes);

    assert_eq!(bytes, b"\x1b[2J");
}

#[test]
fn command_buffer_preserves_command_order() {
    let mut output = CommandBuffer::new();

    output
        .command(commands::cursor::hide())
        .command(commands::screen::clear())
        .command(commands::cursor::move_to(ProtocolPosition::new(2, 3)))
        .text("Ready")
        .command(commands::cursor::show());

    assert_eq!(
        output.as_bytes(),
        b"\x1b[?25l\x1b[2J\x1b[2;3HReady\x1b[?25h"
    );
}

#[test]
fn text_is_queued_as_verbatim_utf8_bytes() {
    let mut output = CommandBuffer::new();

    output.text("\u{1b}[31mred");

    assert_eq!(output.into_bytes(), b"\x1b[31mred");
}
