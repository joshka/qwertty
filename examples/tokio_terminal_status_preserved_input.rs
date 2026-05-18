//! Open a Tokio-backed terminal session and handle unrelated input preserved during a
//! terminal-status query.

#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Error, InputEvent, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    session.text("requesting terminal status\r\n").await?;

    match session
        .request_terminal_status(Duration::from_millis(100))
        .await
    {
        Ok(report) => {
            session
                .text(format!(
                    "terminal status arrived immediately: {:?}\r\n",
                    report.status()
                ))
                .await?;

            match session.next_event().await? {
                InputEvent::Text(ch) => {
                    session
                        .text(format!(
                            "preserved unrelated input arrived through next_event: {ch:?}\r\n"
                        ))
                        .await?;
                }
                event => {
                    session
                        .text(format!("saw other input after query: {event:?}\r\n"))
                        .await?;
                }
            }
        }
        Err(Error::QueryTimeout { timeout, .. }) => {
            session
                .text(format!(
                    "terminal status query timed out after {timeout:?}\r\n"
                ))
                .await?;

            match tokio::time::timeout(Duration::from_millis(250), session.next_event()).await {
                Ok(Ok(InputEvent::Text(ch))) => {
                    session
                        .text(format!(
                            "preserved unrelated input arrived through next_event: {ch:?}\r\n"
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
                        .text("no preserved unrelated input arrived before the follow-up wait ended\r\n")
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
