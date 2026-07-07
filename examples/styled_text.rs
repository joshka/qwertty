//! Builds a styled line into a command buffer using the `commands::style` family.
//!
//! This mirrors `build_status_line.rs`'s shape: pure byte-building against `CommandBuffer`, no
//! terminal opened. It shows bold plus a named foreground color, a truecolor background, a curly
//! underline substyle with an explicit underline color, and the full-reset cleanup at the end.

use qwertty::CommandBuffer;
use qwertty::commands::style::{self, Color, UnderlineStyle};

fn main() {
    let mut output = CommandBuffer::new();
    output
        .command(style::bold())
        .command(style::foreground(Color::Red))
        .text("error: ")
        .command(style::reset_bold_dim())
        .command(style::reset_foreground())
        .command(style::background(Color::Rgb(40, 40, 40)))
        .command(style::underline_style(UnderlineStyle::Curly))
        .command(style::underline_color(Color::Yellow))
        .text("build failed")
        .command(style::reset_all());

    assert_eq!(
        output.as_bytes(),
        b"\x1b[1m\x1b[31merror: \x1b[22m\x1b[39m\x1b[48;2;40;40;40m\x1b[4:3m\x1b[58;5;3mbuild failed\x1b[0m"
    );

    print!("{}", String::from_utf8_lossy(output.as_bytes()));
    println!();
}
