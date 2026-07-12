//! Build the iTerm2 inline-image command bytes and print them, escaped.
//!
//! `commands::graphics::iterm2` is encode-only: it turns an already-encoded image and typed size
//! arguments into the `OSC 1337 ; File=…:<base64> ST` bytes iTerm2 (and `WezTerm`) render inline,
//! without opening a terminal or checking that the terminal supports images. This example shows
//! what the helpers produce; a real application would gate emission on a terminal-identity
//! capability finding and hand the bytes to a session (see `docs/reference/graphics.md`).
//!
//! Run it anywhere — it writes nothing to the terminal but its own printout:
//!
//! ```sh
//! cargo run --example iterm2_inline_image
//! ```

use std::fmt::Write as _;

use qwertty::CommandBuffer;
use qwertty::commands::graphics::iterm2::{self, Dimension};

fn main() {
    // A real caller passes an encoded image file (PNG/JPEG/GIF). Three zero bytes stand in here so
    // the output is short and stable.
    let image = b"\x00\x00\x00";

    let commands = [
        ("natural size", iterm2::inline_image(image)),
        (
            "10 cells x 50%",
            iterm2::inline_image_sized(image, Dimension::Cells(10), Dimension::Percent(50)),
        ),
    ];

    for (label, command) in commands {
        let bytes = CommandBuffer::new().command(command).as_bytes().to_vec();
        println!("{label:<16} {}", escape_for_display(&bytes));
    }
}

/// Renders control bytes readably: `ESC` as `\e`, other non-printables as `\xNN`, the rest literal.
fn escape_for_display(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &byte in bytes {
        match byte {
            0x1b => out.push_str("\\e"),
            0x20..=0x7e => out.push(byte as char),
            other => {
                let _ = write!(out, "\\x{other:02x}");
            }
        }
    }
    out
}
