//! Encoding behavior tests.

use qwertty::commands::osc::ClipboardSelection;
use qwertty::commands::style::{Color, UnderlineStyle};
use qwertty::{Command, CommandBuffer, ProtocolPosition, SyntaxParser, SyntaxToken, commands};

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

/// Asserts a command's bytes parse back through `SyntaxParser` as exactly one CSI token whose
/// final byte is `m` (a well-formed SGR sequence).
fn assert_sgr_round_trip(command: &Command) {
    let mut bytes = Vec::new();
    command.encode(&mut bytes);

    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(&bytes);
    tokens.extend(parser.finish());

    assert_eq!(tokens.len(), 1, "expected one token from {bytes:?}");
    let SyntaxToken::Csi(csi) = &tokens[0] else {
        panic!("expected a CSI token from {bytes:?}, got {:?}", tokens[0]);
    };
    assert_eq!(csi.params().final_byte(), b'm');
}

#[test]
fn style_buffer_composes_attributes_color_and_reset() {
    let mut output = CommandBuffer::new();

    output
        .command(commands::style::bold())
        .command(commands::style::foreground(Color::Red))
        .command(commands::style::underline_style(UnderlineStyle::Curly))
        .text("alert")
        .command(commands::style::reset_all());

    assert_eq!(output.as_bytes(), b"\x1b[1m\x1b[31m\x1b[4:3malert\x1b[0m");
}

#[test]
fn style_foreground_truecolor_encodes_semicolon_form() {
    let mut output = CommandBuffer::new();

    output.command(commands::style::foreground(Color::Rgb(10, 20, 30)));

    assert_eq!(output.as_bytes(), b"\x1b[38;2;10;20;30m");
}

#[test]
fn style_background_indexed_encodes_semicolon_form() {
    let mut output = CommandBuffer::new();

    output.command(commands::style::background(Color::Indexed(214)));

    assert_eq!(output.as_bytes(), b"\x1b[48;5;214m");
}

#[test]
fn style_underline_color_encodes_semicolon_form_not_colon() {
    let mut output = CommandBuffer::new();

    output.command(commands::style::underline_color(Color::Rgb(1, 2, 3)));

    // FM-W6: semicolon form is used for underline color too, not `58:2::r:g:b`.
    assert_eq!(output.as_bytes(), b"\x1b[58;2;1;2;3m");
}

#[test]
fn style_reset_underline_color_bytes() {
    let mut output = CommandBuffer::new();

    output.command(commands::style::reset_underline_color());

    assert_eq!(output.as_bytes(), b"\x1b[59m");
}

#[test]
fn style_all_named_colors_and_underline_styles_round_trip_as_single_sgr_csi() {
    let named = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::White,
        Color::BrightBlack,
        Color::BrightRed,
        Color::BrightGreen,
        Color::BrightYellow,
        Color::BrightBlue,
        Color::BrightMagenta,
        Color::BrightCyan,
        Color::BrightWhite,
    ];

    for color in named {
        assert_sgr_round_trip(&commands::style::foreground(color));
        assert_sgr_round_trip(&commands::style::background(color));
    }

    let underline_styles = [
        UnderlineStyle::None,
        UnderlineStyle::Straight,
        UnderlineStyle::Double,
        UnderlineStyle::Curly,
        UnderlineStyle::Dotted,
        UnderlineStyle::Dashed,
    ];

    for style in underline_styles {
        assert_sgr_round_trip(&commands::style::underline_style(style));
    }

    assert_sgr_round_trip(&commands::style::underline_color(Color::Rgb(1, 2, 3)));
    assert_sgr_round_trip(&commands::style::underline_color(Color::Indexed(9)));
}

/// Asserts a command's bytes parse back through `SyntaxParser` as exactly one OSC token.
fn assert_osc_round_trip(command: &Command) {
    let mut bytes = Vec::new();
    command.encode(&mut bytes);

    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(&bytes);
    tokens.extend(parser.finish());

    assert_eq!(tokens.len(), 1, "expected one token from {bytes:?}");
    let SyntaxToken::Osc(osc) = &tokens[0] else {
        panic!("expected an OSC token from {bytes:?}, got {:?}", tokens[0]);
    };
    assert_eq!(osc.as_bytes(), bytes.as_slice());
}

#[test]
fn osc_buffer_composes_title_hyperlink_and_clipboard() {
    let mut output = CommandBuffer::new();

    output
        .command(commands::osc::set_title("qwertty"))
        .command(commands::osc::hyperlink(
            "https://example.com",
            Some("docs"),
        ))
        .text("docs")
        .command(commands::osc::close_hyperlink())
        .command(commands::osc::set_clipboard(
            ClipboardSelection::Clipboard,
            b"Hello",
        ));

    assert_eq!(
        output.as_bytes(),
        b"\x1b]2;qwertty\x1b\\\x1b]8;id=docs;https://example.com\x1b\\docs\x1b]8;;\x1b\\\x1b]52;c;SGVsbG8=\x1b\\"
    );
}

#[test]
fn osc_title_sanitizes_control_and_bidi_injection() {
    let command = commands::osc::set_title("a\x1b[31mb\u{202E}c");

    assert_osc_round_trip(&command);

    let mut bytes = Vec::new();
    command.encode(&mut bytes);
    assert_eq!(bytes, b"\x1b]2;a[31mbc\x1b\\");
}

#[test]
fn osc_semantic_prompt_sequence_round_trips() {
    let mut output = CommandBuffer::new();

    output
        .command(commands::osc::prompt_start())
        .text("$ ")
        .command(commands::osc::prompt_end())
        .text("ls")
        .command(commands::osc::command_executed())
        .text("file.txt\n")
        .command(commands::osc::command_finished(Some(0)));

    assert_eq!(
        output.as_bytes(),
        b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\ls\x1b]133;C\x1b\\file.txt\n\x1b]133;D;0\x1b\\"
    );

    for command in [
        commands::osc::prompt_start(),
        commands::osc::prompt_end(),
        commands::osc::command_executed(),
        commands::osc::command_finished(Some(0)),
    ] {
        assert_osc_round_trip(&command);
    }
}

#[test]
fn osc_hyperlink_and_clipboard_each_round_trip_as_single_osc() {
    assert_osc_round_trip(&commands::osc::hyperlink("https://example.com", None));
    assert_osc_round_trip(&commands::osc::hyperlink("https://example.com", Some("a")));
    assert_osc_round_trip(&commands::osc::close_hyperlink());
    assert_osc_round_trip(&commands::osc::set_clipboard(
        ClipboardSelection::Primary,
        b"clip",
    ));
}
