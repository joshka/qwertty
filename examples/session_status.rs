//! Open a terminal session, write a small status line, and leave explicitly.

use qwertty::{ProtocolPosition, TerminalSession, commands};

fn main() -> qwertty::Result<()> {
    let mut session = TerminalSession::open()?;
    let size = session.size()?;

    session
        .command(commands::screen::clear())?
        .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))?
        .text(format!(
            "session active: {} columns x {} rows\r\n",
            size.columns(),
            size.rows()
        ))?
        .flush()?;

    session.leave()
}
