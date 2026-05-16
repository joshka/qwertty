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
        .command(commands::cursor::request_position())
        .command(commands::terminal::request_status())
        .text("Ready")
        .command(commands::cursor::show());

    assert_eq!(
        output.as_bytes(),
        b"\x1b[?25l\x1b[2J\x1b[2;3H\x1b[6n\x1b[5nReady\x1b[?25h"
    );
}

#[test]
fn cursor_position_query_encodes_device_status_report_request() {
    let mut output = CommandBuffer::new();

    output.command(commands::cursor::request_position());

    assert_eq!(output.as_bytes(), b"\x1b[6n");
}

#[test]
fn terminal_status_query_encodes_device_status_report_request() {
    let mut output = CommandBuffer::new();

    output.command(commands::terminal::request_status());

    assert_eq!(output.as_bytes(), b"\x1b[5n");
}

#[test]
fn text_is_queued_as_verbatim_utf8_bytes() {
    let mut output = CommandBuffer::new();

    output.text("\u{1b}[31mred");

    assert_eq!(output.into_bytes(), b"\x1b[31mred");
}
