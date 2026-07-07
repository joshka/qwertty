//! Install a panic hook that restores the terminal before the backtrace prints.
//!
//! Run with `cargo run --example panic_safe_restore`. The session enters raw mode; the restore
//! handle guarantees a panic anywhere in the program leaves the terminal usable and the panic
//! message readable, without borrowing the session.

#[cfg(unix)]
fn main() -> qwertty::Result<()> {
    use qwertty::TerminalSession;

    let mut session = TerminalSession::open()?;

    // Install the hook once, before application work. The handle stays valid without borrowing
    // the session, and restoration runs at most once across panic, leave, and drop.
    let restore = session.restore_handle();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        _ = restore.restore();
        previous(info);
    }));

    session
        .text("raw mode active; a panic would now restore the terminal first\r\n")?
        .flush()?;

    session.leave()
}

#[cfg(not(unix))]
fn main() {
    eprintln!("this example requires Unix because live terminal sessions are Unix-only today");
}
