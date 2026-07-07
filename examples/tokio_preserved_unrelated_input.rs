//! Open a Tokio-backed terminal session and handle unrelated input preserved during a live query.

#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Error, Event, TokioTerminalSession};

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

            let event = session.next_event().await?;
            session.text(describe_preserved(&event)).await?;
        }
        Err(Error::QueryTimeout { timeout, .. }) => {
            session
                .text(format!(
                    "cursor position query timed out after {timeout:?}\r\n"
                ))
                .await?;

            match tokio::time::timeout(Duration::from_millis(250), session.next_event()).await {
                Ok(Ok(event)) => {
                    session.text(describe_preserved(&event)).await?;
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

/// Describes a preserved event, calling out the text a key event carries.
#[cfg(all(unix, feature = "tokio"))]
fn describe_preserved(event: &Event) -> String {
    match event {
        Event::Key(key) => match key.text() {
            Some(text) => {
                format!(
                    "preserved unrelated input arrived through next_event: {:?}\r\n",
                    text.as_str()
                )
            }
            None => format!("saw key event after query: {:?}\r\n", key.key()),
        },
        event => format!("saw other input after query: {event:?}\r\n"),
    }
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
