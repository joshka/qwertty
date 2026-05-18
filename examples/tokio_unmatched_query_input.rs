//! Open a Tokio-backed terminal session and treat unmatched query-shaped CSI as ordinary input.

#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Error, InputEvent, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    session.text("requesting cursor position\r\n").await?;

    match session
        .request_cursor_position(Duration::from_millis(100))
        .await
    {
        Ok(report) => {
            session
                .text(format!(
                    "cursor position arrived immediately: row {} column {}\r\n",
                    report.row(),
                    report.column()
                ))
                .await?;
        }
        Err(Error::QueryTimeout { timeout, .. }) => {
            session
                .text(format!(
                    "cursor position query timed out after {timeout:?}\r\n"
                ))
                .await?;

            match tokio::time::timeout(Duration::from_millis(250), session.next_event()).await {
                Ok(Ok(InputEvent::Csi(csi))) => {
                    session
                        .text(format!(
                            "unmatched query-shaped CSI arrived through next_event: {:?}\r\n",
                            csi.as_bytes()
                        ))
                        .await?;
                }
                Ok(Ok(event)) => {
                    session
                        .text(format!("saw other input after timeout: {event:?}\r\n"))
                        .await?;
                }
                Ok(Err(err)) => return Err(err),
                Err(_elapsed) => {
                    session
                        .text("no unmatched query-shaped CSI arrived before the follow-up wait ended\r\n")
                        .await?;
                }
            }
        }
        Err(err) => return Err(err),
    }

    session.flush().await?;
    session.leave().await
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
