//! Open a Tokio-backed terminal session, issue live queries, and leave explicitly.

#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{ProtocolPosition, TokioTerminalSession, commands};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;
    if let Some(acquisition) = session.acquisition() {
        eprintln!("acquired controlling terminal: {acquisition}");
    }
    session.command(commands::screen::clear()).await?;
    session
        .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))
        .await?;
    session.text("querying terminal state\r\n").await?;

    let status = session
        .request_terminal_status(Duration::from_secs(1))
        .await?;
    let position = session
        .request_cursor_position(Duration::from_secs(1))
        .await?;

    session
        .text(format!(
            "status: {:?}, cursor: row {} column {}\r\n",
            status.status(),
            position.row(),
            position.column()
        ))
        .await?;
    session.flush().await?;
    session.leave().await
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
