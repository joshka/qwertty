//! Builds a buffer combining several OSC command families: a hyperlink, a window title, and a
//! policy-gated clipboard write.
//!
//! This mirrors `styled_text.rs`'s shape: pure byte-building against `CommandBuffer`, no terminal
//! opened. It also demonstrates title sanitization (FM-X3): the title argument below carries a
//! bidi-override character that is stripped before encoding.
//!
//! OSC 52 clipboard writes are an exfiltration surface (FM-X4): `commands::osc::set_clipboard`
//! only builds bytes, so this example builds them for illustration, but never writes them to a
//! real terminal. Code that does write these bytes to a real terminal is responsible for its own
//! policy gate first.

use qwertty::CommandBuffer;
use qwertty::commands::osc::{self, ClipboardSelection};

fn main() {
    let mut output = CommandBuffer::new();
    output
        // FM-X3: the embedded bidi right-to-left override (U+202E) is stripped by `set_title`
        // before encoding, so the emitted title bytes hold only "qwertty: build ok".
        .command(osc::set_title("qwertty: build \u{202E}ok"))
        .command(osc::hyperlink("https://example.com/docs", Some("docs")))
        .text("docs")
        .command(osc::close_hyperlink())
        // FM-X4: builds the bytes only; a session or application must policy-gate a real write.
        .command(osc::set_clipboard(
            ClipboardSelection::Clipboard,
            b"copied text",
        ));

    assert_eq!(
        output.as_bytes(),
        b"\x1b]2;qwertty: build ok\x1b\\\
\x1b]8;id=docs;https://example.com/docs\x1b\\docs\x1b]8;;\x1b\\\
\x1b]52;c;Y29waWVkIHRleHQ=\x1b\\"
    );

    print!("{}", String::from_utf8_lossy(output.as_bytes()));
    println!();
}
