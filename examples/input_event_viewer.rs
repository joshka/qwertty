//! A continuous decoded-input event viewer, runnable on Unix and Windows.
//!
//! Unlike the other input examples (which are Unix-only and read a single event), this one runs on
//! both platforms and loops, printing every decoded [`Event`] until you press Ctrl-C. It is the
//! interactive tool for eyeballing what the backend actually produces — keys (including via an
//! IME), mouse, focus, bracketed paste, and resize — which is exactly what the Windows validation
//! runbook (`docs/development/windows-validation.md`) drives across the terminal matrix.
//!
//! Run it with the `tokio` feature:
//!
//! ```sh
//! cargo run --example input_event_viewer --features tokio
//! ```
//!
//! Type anything and watch the decoded events scroll by. On Windows, switch to a CJK input method
//! and confirm composed characters arrive as `Key::Char` with the expected `text`. Press Ctrl-C to
//! leave; the session restores the terminal on the way out.

#[cfg(all(any(unix, windows), feature = "tokio"))]
use qwertty::{Event, Key, KittyKeyboardFlags, MouseMode, TokioTerminalSession};

#[cfg(all(any(unix, windows), feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    use std::time::Duration;

    let mut session = TokioTerminalSession::open()?;

    // Turn on every input-reporting mode the viewer wants to show, so a human can exercise the
    // whole decoded surface. Each is a no-op the terminal may ignore; the viewer just reports
    // what arrives.
    session.enable_mouse(MouseMode::ButtonEvent).await?;
    session.enable_focus_events().await?;
    session.enable_bracketed_paste().await?;

    // Probe kitty keyboard rather than assume it: the grant reports the subset the terminal
    // actually honored (Windows Terminal 1.25+ supports it; most others do not), so the header
    // can say which enhancement is live without guessing.
    let requested =
        KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.union(KittyKeyboardFlags::REPORT_EVENT_TYPES);
    let kitty = match session
        .request_kitty_keyboard(requested, Duration::from_millis(250))
        .await
    {
        Ok(grant) => match grant.granted() {
            Some(flags) if !flags.is_empty() => format!("kitty keyboard: {flags:?}"),
            _ => "kitty keyboard: not supported".to_owned(),
        },
        Err(_) => "kitty keyboard: unknown (no answer)".to_owned(),
    };

    session
        .text(format!(
            "input event viewer — type to see decoded events; press Ctrl-C to exit\r\n{kitty}\r\n\r\n"
        ))
        .await?;
    session.flush().await?;

    loop {
        let event = session.next_event().await?;
        // Ctrl-C (the C0 ETX byte, decoded as `Key::Control(3)`) is the quit key: it does not
        // collide with any printable text, so IME composition and ordinary typing stay
        // fully observable.
        if let Event::Key(key) = &event {
            if key.key() == Key::Control(3) {
                break;
            }
        }
        session.text(format!("{}\r\n", describe(&event))).await?;
        session.flush().await?;
    }

    session.leave().await
}

/// Renders one event as a single diagnostic line: the key's identity and any decoded text for a key
/// event (so composed/IME input is visible), and the `Debug` form for everything else.
#[cfg(all(any(unix, windows), feature = "tokio"))]
fn describe(event: &Event) -> String {
    match event {
        Event::Key(key) => match key.text() {
            Some(text) => format!("key {:?} text {:?}", key.key(), text.as_str()),
            None => format!("key {:?}", key.key()),
        },
        Event::Syntax(token) => format!("syntax {:?}", token.as_bytes()),
        other => format!("{other:?}"),
    }
}

#[cfg(not(all(any(unix, windows), feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix or Windows");
}
