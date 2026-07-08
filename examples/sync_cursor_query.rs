//! Ask the terminal for its cursor position with one typed call and no async runtime.
//!
//! Run with `cargo run --example sync_cursor_query` (default features — no `tokio`). Where
//! `oneshot_background.rs` spells out the whole poll/read/parse recipe by hand, this example uses
//! the typed convenience [`TerminalSession::request_cursor_position`] that wraps that same recipe:
//! it registers the query with the sans-io correlator, writes `CSI 6 n`, and drives a blocking
//! poll/read/decode loop until the reply completes — the *second consumer* the sans-io split
//! (design 04) was built for, driving the same correlator the async session uses without Tokio.
//!
//! The helper returns:
//!
//! - `Ok(Some(report))` when the terminal answered within the budget;
//! - `Ok(None)` when it did not — the FM-C4 *unknown* case, not an error and not a hang;
//! - `Err(..)` only on a genuine I/O failure.
//!
//! The raw building blocks stay available: this typed helper does not hide
//! [`TerminalSession::command`], [`TerminalSession::read_input`], or [`TerminalSession::as_fd`]. A
//! caller that needs its own loop still reaches for those — see `oneshot_background.rs` for the
//! hand-rolled form.

// `main` returns qwertty's own `Error`: unlike the hand-rolled recipe, the typed helper owns the
// poll loop, so this example never touches rustix's error vocabulary directly. Every early return
// still restores the terminal — the panic hook and the session's drop-time `leave` cover the `?`
// paths.
#[cfg(unix)]
fn main() -> qwertty::Result<()> {
    use std::time::Duration;

    use qwertty::TerminalSession;

    // The whole query must complete inside this budget. 150 ms comfortably clears a local
    // terminal's round-trip while staying short enough that a non-answering host does not stall.
    let budget = Duration::from_millis(150);

    // Opening the session enters raw mode. Take the restore handle first so a panic anywhere below
    // still restores cooked mode before the backtrace prints; the handle does not borrow the
    // session, so the query call can keep using `&mut session` freely.
    let mut session = TerminalSession::open()?;
    let restore = session.restore_handle();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        _ = restore.restore();
        previous(info);
    }));

    // One typed call: register, write `CSI 6 n`, poll/read/decode until the report completes, or
    // time out to `None`. Typeahead the user sent before the terminal answered is not swallowed —
    // it stays deliverable through the next `read_input`.
    let reply = session.request_cursor_position(budget)?;

    // Restore the terminal *before* printing so the human-readable line lands in cooked mode.
    // `leave` is idempotent with the panic hook and drop, so it runs at most once.
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
    // Raw mode, poll(2), and the blocking query driver are Unix-only in qwertty today.
    eprintln!("this example requires Unix because live terminal sessions are Unix-only today");
}
