//! Ask the terminal what it supports with one typed call and no async runtime.
//!
//! Run with `cargo run --example sync_capability_probe` (default features — no `tokio`). Where
//! `sync_cursor_query.rs` asks a single question, this example uses
//! [`TerminalSession::probe_capabilities`] — the DA1-fenced bundle: one write asking XTVERSION,
//! the kitty keyboard flags, OSC 10/11 (default foreground/background colour), and four DEC
//! private mode queries, with Primary Device Attributes (DA1) last as a fence. A terminal that
//! answers DA1 has finished answering everything it is going to answer, so the probe returns as
//! soon as the fence fires — not after the whole timeout — on a terminal that replies fast.
//!
//! This is the flagship one-shot CLI use case (OQ-1): detecting a dark or light background from
//! [`Capabilities::background_color`] to pick a matching colour scheme, the way starship, bat, and
//! delta segments do at startup. Every unanswered field is `None` — *unknown*, never unsupported
//! (FM-C4) — so a caller that gets no reply should fall back to an environment heuristic (see
//! [`Capabilities::hyperlinks`]/[`Capabilities::truecolor`] for the crate's own env-inferred
//! fields) rather than assume light or dark.

// `main` returns qwertty's own `Error`. Every early return still restores the terminal — the
// panic hook and the session's drop-time `leave` cover the `?` paths.
#[cfg(unix)]
fn main() -> qwertty::Result<()> {
    use std::time::Duration;

    use qwertty::TerminalSession;

    // The whole bundle must complete inside this budget. 150 ms comfortably clears a local
    // terminal's round-trip while staying short enough that a non-answering host (piped stdio, a
    // multiplexer that swallows some queries) does not stall the program.
    let budget = Duration::from_millis(150);

    // Opening the session enters raw mode. Take the restore handle first so a panic anywhere below
    // still restores cooked mode before the backtrace prints; the handle does not borrow the
    // session, so the probe call can keep using `&mut session` freely.
    let mut session = TerminalSession::open()?;
    let restore = session.restore_handle();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        _ = restore.restore();
        previous(info);
    }));

    // One typed call: register the whole bundle, write it in one buffer (DA1 last as the fence),
    // and drive a blocking poll/read/decode loop until the fence completes or the budget elapses.
    // Typeahead the user sent during the probe is not swallowed — it stays deliverable through the
    // next `read_input`.
    let capabilities = session.probe_capabilities(budget)?;

    // Restore the terminal *before* printing so the human-readable lines land in cooked mode.
    session.leave()?;

    match capabilities.background_color.value() {
        Some(rgb) => {
            // A simple luma threshold — good enough for a demo; a real one-shot CLI would use a
            // proper perceptual formula.
            let luma = u32::from(rgb.red()) * 299
                + u32::from(rgb.green()) * 587
                + u32::from(rgb.blue()) * 114;
            let scheme = if luma / 1000 < 128 { "dark" } else { "light" };
            println!("background {rgb:?} -> {scheme} color scheme");
        }
        None => println!(
            "background color unknown (OSC 11 unanswered) — fall back to an env heuristic, \
             never assume light or dark"
        ),
    }

    println!(
        "kitty keyboard flags: {:?}",
        capabilities.kitty_keyboard.value()
    );
    println!(
        "synchronized output (mode 2026): {:?}",
        capabilities.synchronized_output.value_copied()
    );

    Ok(())
}

#[cfg(not(unix))]
fn main() {
    // Raw mode, poll(2), and the blocking query driver are Unix-only in qwertty today.
    eprintln!("this example requires Unix because live terminal sessions are Unix-only today");
}
