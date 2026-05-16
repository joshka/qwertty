//! Open a terminal session and read one chunk of raw input bytes.

use qwertty::{TerminalSession, commands};

fn main() -> qwertty::Result<()> {
    let mut session = TerminalSession::open()?;

    session.text("press a key, then Enter\r\n")?.flush()?;

    let mut buffer = [0; 32];
    let input = session.read_input(&mut buffer)?;

    session
        .command(commands::screen::clear())?
        .text(format!("read {} byte(s)\r\n", input.len()))?
        .flush()?;

    session.leave()
}
