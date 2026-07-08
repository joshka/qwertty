//! Open a Tokio-backed terminal session and react to decoded input events.

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Event, Key, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;
    // A lone Esc is held pending by the decoder until more input arrives (it might begin a
    // sequence). The default 25ms flush window makes a standalone Esc responsive; widen it here to
    // show the setter. `None` would opt out entirely.
    session.set_esc_flush_timeout(Some(std::time::Duration::from_millis(50)));
    session
        .text("press q, Esc, Enter, or Up arrow to exit\r\n")
        .await?;
    session.flush().await?;

    let message = match session.next_event().await? {
        Event::Key(key) => match key.key() {
            Key::Char('q') => "saw q\r\n".to_owned(),
            Key::Escape => "saw Escape\r\n".to_owned(),
            Key::Up => "saw Up arrow\r\n".to_owned(),
            Key::Enter => "saw Enter\r\n".to_owned(),
            other => format!("saw key: {other:?}\r\n"),
        },
        Event::Syntax(token) => format!("saw syntax: {:?}\r\n", token.as_bytes()),
        event => format!("saw event: {event:?}\r\n"),
    };

    session.text(message).await?;
    session.flush().await?;
    session.leave().await
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
