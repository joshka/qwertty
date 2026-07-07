//! Enter the alternate screen, hide the cursor, write a frame, then leave and show the terminal
//! restored.
//!
//! `TerminalSession::enter_alternate_screen` writes `CSI ? 1049 h` followed by an explicit
//! `CSI 2 J` clear (R-OUT-3): some hosts (mosh) do not clear the alternate buffer on 1049 the way
//! most terminals do, and helix works around exactly this by clearing right after entry, so
//! qwertty follows that evidence instead of trusting the terminal's own 1049 behavior. Both the
//! alternate-screen entry and the cursor hide are recorded in the session's mode ledger, so
//! `leave` — or a panic — restores the primary screen and shows the cursor again, in reverse
//! enablement order, with no extra cleanup code at the call site.
//!
//! Run this in a real terminal to see the alternate screen appear and then disappear again,
//! leaving the terminal exactly as it was before.

use qwertty::{ProtocolPosition, TerminalSession, commands};

fn main() -> qwertty::Result<()> {
    let mut session = TerminalSession::open()?;

    // Entering the alternate screen records one ledger entry: apply is enter-and-clear, undo is
    // the plain leave sequence (CSI ? 1049 l) — the primary buffer is never touched while
    // alternate, so undo never needs to clear it.
    session.enter_alternate_screen()?;

    // Hiding the cursor is tracked separately (FM-L3): leave restores the shown state regardless
    // of whether this example calls `show_cursor` itself.
    session.hide_cursor()?;

    session
        .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))?
        .text("alternate screen active, cursor hidden\r\n")?
        .text("leaving now restores the primary screen and shows the cursor again\r\n")?
        .flush()?;

    // Leaving replays the ledger in reverse enablement order: show the cursor, then leave the
    // alternate screen, then restore cooked mode.
    session.leave()
}
