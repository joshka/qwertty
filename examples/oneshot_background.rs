//! Ask the terminal one question with no async runtime: a std-only query round-trip.
//!
//! Run with `cargo run --example oneshot_background` (default features — no `tokio`). This is the
//! "second consumer" the sans-io decode core (design 04) was built for: the same
//! [`SyntaxParser`](qwertty::SyntaxParser) and [`report`](qwertty::report) parsers the async
//! session uses, driven here by a hand-rolled synchronous poll loop instead of a reactor.
//!
//! The recipe is deliberately small enough to read in one screen, because it doubles as the
//! documentation for the pattern. Each step explains *why*:
//!
//! 1. Open a session (enters raw mode) and take a `RestoreHandle` so every exit path — orderly
//!    return, `?` error, or panic — puts the terminal back in cooked mode. No leaked raw mode.
//! 2. Write one probe: `CSI 6 n`, the cursor-position request (DSR). One write, one question.
//! 3. Wait for the fd to become readable with `rustix::event::poll` and a bounded timeout, so the
//!    program never blocks forever. A terminal that does not answer is the FM-C4 *unknown* case,
//!    not an error: we report "no reply" and exit cleanly.
//! 4. Read the available bytes, feed them through the sans-io `SyntaxParser`, and parse the reply
//!    with `report::CursorPositionReport`. Print the parsed position (or the no-reply outcome).
//! 5. `leave()` restores the terminal; the `RestoreHandle`/drop guarantee covers the error paths.
//!
//! Cursor position is chosen over device attributes because it is the simplest self-contained
//! round-trip: a single unambiguous CSI request and a single CSI report parser, with no capability
//! model or DA1 fence to reason about.

// `main` returns a boxed error so the two error vocabularies this recipe touches — qwertty's own
// `Error` from the session calls and rustix's `Errno` from `poll` — both flow through `?` without
// this example needing to invent a conversion between them. Every early return still restores the
// terminal: the panic hook and the session's drop-time `leave` cover the `?` paths.
#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;

    use qwertty::report::CursorPositionReport;
    use qwertty::{SyntaxParser, SyntaxToken, TerminalSession, commands};
    use rustix::event::{PollFd, PollFlags, Timespec, poll};

    // The whole query must complete inside this budget. 150 ms comfortably clears a local
    // terminal's round-trip while staying short enough that a non-answering host (piped stdio, a
    // multiplexer that swallows DSR) does not stall the program.
    let budget = Duration::from_millis(150);

    // Opening the session enters raw mode. Taking the restore handle first means a panic *anywhere*
    // below — including inside the parser — still restores cooked mode before the backtrace prints,
    // and the handle does not borrow the session, so the poll loop can keep using it freely.
    let mut session = TerminalSession::open()?;
    let restore = session.restore_handle();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        _ = restore.restore();
        previous(info);
    }));

    // Step 2: write and flush the single probe. Flushing guarantees the request is on the wire
    // before we start waiting for its answer.
    session
        .command(commands::cursor::request_position())?
        .flush()?;

    // Step 3: wait for the terminal to become readable, bounded by the budget. We poll the
    // session's own device fd (the runtime-neutral readiness seam) instead of blocking in a read,
    // so an unanswered probe times out cleanly instead of hanging. A device with no pollable fd
    // (never a live terminal) reports "no reply" rather than pretending to wait.
    let reply = match session.as_fd() {
        Some(fd) => {
            // `Timespec` fields are signed; our budget is a small positive constant, so the whole
            // seconds fit `i64` and the sub-second nanoseconds widen from `u32` losslessly.
            let timeout = Timespec {
                tv_sec: i64::try_from(budget.as_secs()).unwrap_or(i64::MAX),
                tv_nsec: budget.subsec_nanos().into(),
            };
            let mut fds = [PollFd::new(&fd, PollFlags::IN)];
            // `poll` returns 0 on timeout; any positive count means the fd is readable. We only
            // ever probe one fd, so a nonzero return is our fd and nothing else.
            let readable = poll(&mut fds, Some(&timeout))? > 0;

            if readable {
                // Step 4: one OS read of whatever arrived, fed to the sans-io parser. A single read
                // suffices because a CPR is tiny and arrives as one burst; a real event loop would
                // loop, but a one-shot probe wants exactly one answer.
                let mut buffer = [0u8; 64];
                let input = session.read_input(&mut buffer)?;
                let mut parser = SyntaxParser::new();
                let mut tokens = parser.feed(input.as_bytes());
                tokens.extend(parser.finish());
                // Take the first CSI token that parses as a cursor-position report. Anything else
                // (stray typeahead, an unrelated sequence) is simply not our answer.
                tokens.iter().find_map(|token| match token {
                    SyntaxToken::Csi(csi) => CursorPositionReport::from_control_sequence(csi),
                    _ => None,
                })
            } else {
                None
            }
        }
        None => None,
    };

    // Step 5: restore the terminal *before* printing, so our human-readable line lands in cooked
    // mode. `leave` is idempotent with the panic hook and drop, so it runs at most once.
    session.leave()?;

    match reply {
        Some(report) => println!(
            "cursor position report: row {}, column {}",
            report.row(),
            report.column()
        ),
        None => println!("no reply within {budget:?} (the FM-C4 unknown case, not an error)"),
    }

    Ok(())
}

#[cfg(not(unix))]
fn main() {
    // Raw mode and poll(2) are Unix-only in qwertty today, so the recipe has no non-Unix form.
    eprintln!("this example requires Unix because live terminal sessions are Unix-only today");
}
