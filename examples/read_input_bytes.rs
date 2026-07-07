//! Open a terminal session and read one chunk of raw input bytes.

use qwertty::{SemanticDecoder, TerminalSession, commands};

fn main() -> qwertty::Result<()> {
    let mut session = TerminalSession::open()?;

    session.text("press a key, then Enter\r\n")?.flush()?;

    let mut buffer = [0; 32];
    let input = session.read_input(&mut buffer)?;

    // `InputBytes` keeps the bytes exactly as read. Decode them into typed events by feeding the
    // raw bytes through the semantic decoder.
    let mut decoder = SemanticDecoder::new();
    let events = decoder.feed(input.as_bytes());

    session
        .command(commands::screen::clear())?
        .text(format!(
            "read {} byte(s), decoded {} event(s)\r\n",
            input.len(),
            events.len()
        ))?
        .flush()?;

    session.leave()
}
