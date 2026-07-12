//! Build the kitty graphics protocol command bytes and print them, escaped.
//!
//! `commands::graphics::kitty` is encode-only: it turns an image and typed control parameters into
//! the `ESC _ G … ESC \` Application Program Command bytes the protocol defines, without opening a
//! terminal or checking that the terminal supports graphics. This example shows what each helper
//! produces; a real application would gate emission on a kitty-graphics capability finding and hand
//! the bytes to a session (see `docs/reference/graphics.md`).
//!
//! Run it anywhere — it writes nothing to the terminal but its own printout:
//!
//! ```sh
//! cargo run --example kitty_graphics
//! ```

use std::fmt::Write as _;

use qwertty::CommandBuffer;
use qwertty::commands::graphics::kitty::{self, Format};

fn main() {
    // A real caller passes already-encoded image bytes (a PNG file, or raw RGB/RGBA pixels). Three
    // zero bytes stand in here so the output is short and stable.
    let image = b"\x00\x00\x00";

    let commands = [
        (
            "transmit + display",
            kitty::transmit_and_display(Format::Png, image),
        ),
        ("place image id 7", kitty::place(7)),
        ("delete image id 7", kitty::delete_image(7)),
        ("delete all images", kitty::delete_all_images()),
    ];

    for (label, command) in commands {
        let bytes = CommandBuffer::new().command(command).as_bytes().to_vec();
        println!("{label:<20} {}", escape_for_display(&bytes));
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
