//! Open a Tokio-backed terminal session and handle live query outcomes explicitly.

#[cfg(all(unix, feature = "tokio"))]
use std::io::ErrorKind;
#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Error, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    session.text("checking terminal query paths\r\n").await?;
    handle_terminal_status(&mut session).await?;
    handle_cursor_position(&mut session).await?;
    session.flush().await?;
    session.leave().await
}

#[cfg(all(unix, feature = "tokio"))]
async fn handle_terminal_status(session: &mut TokioTerminalSession) -> qwertty::Result<()> {
    match session
        .request_terminal_status(Duration::from_secs(1))
        .await
    {
        Ok(report) => {
            session
                .text(format!("terminal status: {:?}\r\n", report.status()))
                .await?;
            Ok(())
        }
        Err(Error::QueryTimeout { timeout, .. }) => {
            session
                .text(format!(
                    "terminal status query timed out after {timeout:?}\r\n"
                ))
                .await?;
            Ok(())
        }
        Err(Error::ReadTerminal { source }) if source.kind() == ErrorKind::UnexpectedEof => {
            session
                .text("terminal closed before a terminal status reply arrived\r\n")
                .await?;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

#[cfg(all(unix, feature = "tokio"))]
async fn handle_cursor_position(session: &mut TokioTerminalSession) -> qwertty::Result<()> {
    match session
        .request_cursor_position(Duration::from_secs(1))
        .await
    {
        Ok(report) => {
            session
                .text(format!(
                    "cursor position: row {} column {}\r\n",
                    report.row(),
                    report.column()
                ))
                .await?;
            Ok(())
        }
        Err(Error::QueryTimeout { timeout, .. }) => {
            session
                .text(format!(
                    "cursor position query timed out after {timeout:?}\r\n"
                ))
                .await?;
            Ok(())
        }
        Err(Error::ReadTerminal { source }) if source.kind() == ErrorKind::UnexpectedEof => {
            session
                .text("terminal closed before a cursor position reply arrived\r\n")
                .await?;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
