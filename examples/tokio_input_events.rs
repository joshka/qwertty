//! Open a Tokio-backed terminal session and react to decoded input events.

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{InputEvent, KeyInput, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;
    session
        .text("press q, Enter, or Up arrow to exit\r\n")
        .await?;
    session.flush().await?;

    let message = match session.next_event().await? {
        InputEvent::Text('q') => "saw q\r\n".to_owned(),
        InputEvent::Control(control) => format!("saw control: {control:?}\r\n"),
        InputEvent::Key(KeyInput::Up) => "saw Up arrow\r\n".to_owned(),
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
