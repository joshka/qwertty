//! Open a Tokio-backed terminal session and cancel a live terminal-status query explicitly.

#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::TokioTerminalSession;

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    session.text("starting terminal-status query\r\n").await?;

    let outcome = {
        let query = session.request_terminal_status(Duration::from_secs(1));
        tokio::pin!(query);

        tokio::select! {
            report = &mut query => {
                let report = report?;
                format!("terminal-status query finished before cancellation: {:?}\r\n", report.status())
            }
            () = tokio::time::sleep(Duration::from_millis(50)) => {
                String::from("canceled pending terminal-status query before a reply arrived\r\n")
            }
        }
    };

    session.text(outcome).await?;
    session
        .text("session is still usable after the query future ends\r\n")
        .await?;
    session.flush().await?;
    session.leave().await
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
