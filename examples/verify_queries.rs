//! Real-emulator verification smoke for the live query path.
//!
//! Run once per real terminal application:
//!
//! ```sh
//! cargo run --example verify_queries --features tokio
//! ```
//!
//! PTY-backed tests script the terminal's side of every query, so they prove routing logic but
//! not what a real emulator sends. This example lets a real terminal answer for itself and
//! self-checks what it can:
//!
//! 1. Terminal status query answered.
//! 2. Cursor position query answered, and the column matches text the example just wrote.
//! 3. Typeahead survives queries: keys typed while queries run are delivered afterwards, in order,
//!    not stolen or reordered (verified by you against what you typed).
//! 4. After exit, your shell prompt must come back clean: no stray `[12;34R`-style garbage.

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    use std::time::Duration;

    use qwertty::{InputEvent, TokioTerminalSession};

    let timeout = Duration::from_secs(2);
    let mut session = TokioTerminalSession::open()?;
    let mut passes = 0;
    let mut failures = 0;

    // Check 1: the terminal answers a status query.
    session
        .text("\r\nverify_queries: live query smoke\r\n")
        .await?;
    session.flush().await?;
    match session.request_terminal_status(timeout).await {
        Ok(report) => {
            passes += 1;
            session
                .text(format!(
                    "PASS  status query answered: {:?}\r\n",
                    report.status()
                ))
                .await?;
        }
        Err(error) => {
            failures += 1;
            session
                .text(format!("FAIL  status query: {error}\r\n"))
                .await?;
        }
    }

    // Check 2: the reported cursor column matches text this example wrote. Five characters from
    // the start of a fresh line put the cursor in column six.
    session.text("\r12345").await?;
    session.flush().await?;
    match session.request_cursor_position(timeout).await {
        Ok(report) if report.column() == 6 => {
            passes += 1;
            session
                .text(format!(
                    "\r\nPASS  cursor position answered and matches (row {}, column 6)\r\n",
                    report.row()
                ))
                .await?;
        }
        Ok(report) => {
            failures += 1;
            session
                .text(format!(
                    "\r\nFAIL  cursor position answered but column {} != 6 (row {})\r\n",
                    report.column(),
                    report.row()
                ))
                .await?;
        }
        Err(error) => {
            failures += 1;
            session
                .text(format!("\r\nFAIL  cursor position query: {error}\r\n"))
                .await?;
        }
    }

    // Check 3: typeahead survives queries. Keys typed while queries run must be delivered
    // afterwards, complete and in order.
    session
        .text("Type a few letters NOW (about three seconds of queries are running)...\r\n")
        .await?;
    session.flush().await?;
    let mut query_rounds = 0u32;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        // Every round interleaves a live query with whatever is being typed.
        let _ = session.request_cursor_position(timeout).await;
        query_rounds += 1;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let mut typed = String::new();
    while let Ok(Ok(event)) =
        tokio::time::timeout(Duration::from_millis(200), session.next_event()).await
    {
        if let InputEvent::Text(character) = event {
            typed.push(character);
        }
    }

    session
        .text(format!(
            "Captured {typed:?} across {query_rounds} interleaved queries.\r\n\
             If that matches what you typed (in order, nothing missing), typeahead PASSES.\r\n"
        ))
        .await?;

    // Summary and the final human check.
    session
        .text(format!(
            "\r\nSelf-checked: {passes} pass, {failures} fail. Leaving the session now.\r\n\
             Final check: your prompt must come back clean, with no stray query replies.\r\n"
        ))
        .await?;
    session.flush().await?;
    session.leave().await?;

    if failures > 0 {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
    eprintln!("run: cargo run --example verify_queries --features tokio");
}
