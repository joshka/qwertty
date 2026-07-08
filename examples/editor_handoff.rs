//! Hand the terminal to `$EDITOR` (a pager, a subshell — any synchronous child) and reclaim it.
//!
//! This is the detached-handoff lifecycle (design 01 §4), the sibling of suspend/resume. Where
//! suspend/resume drops to the *shell* on `Ctrl-Z` and comes back on `SIGCONT`, `run_detached`
//! hands the terminal to a child the caller spawns and waits for **inside the closure**, with no
//! job-control stop. Before the child runs it restores the terminal to a clean blocking state and
//! disarms the panic-safe handle; after the child returns it re-enters raw mode (never trusting the
//! termios a child like `vi` may have left), re-registers async readiness on the same fd, and
//! queues a synthetic `Event::Resize` so the app repaints at whatever size the terminal is now.
//!
//! The closure is a synchronous `FnOnce`: the async session is quiescent while it runs, and
//! whatever it returns is returned from `run_detached`, so the caller inspects the child's
//! `ExitStatus` directly. Press `e` to launch `$EDITOR` (falling back to a portable no-op so this
//! runs without a real editor installed), `q` to quit.

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Event, Key, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    session
        .text("press e to hand the terminal to $EDITOR, q to quit\r\n")
        .await?;
    session.flush().await?;

    loop {
        match session.next_event().await? {
            Event::Key(key) if matches!(key.key(), Key::Char('q')) => break,
            Event::Key(key) if matches!(key.key(), Key::Char('e')) => {
                // Hand the terminal to a synchronous child. Everything inside the closure blocks:
                // the child owns a clean blocking terminal, and this session is quiescent until it
                // returns. The `ExitStatus` the closure returns is returned from `run_detached`.
                let status = session
                    .run_detached(|| std::process::Command::new(editor()).status())
                    .await?;

                // Back in raw mode with a fresh readiness registration and a synthetic resize
                // already queued. Report what the child did.
                let line = match status {
                    Ok(status) => format!("editor exited: {status}\r\n"),
                    Err(error) => format!("could not launch editor: {error}\r\n"),
                };
                session.text(line).await?;
                session.flush().await?;
            }
            Event::Resize(resize) => {
                let cells = resize.cells();
                let line = format!("resized to {}x{}\r\n", cells.columns(), cells.rows());
                session.text(line).await?;
                session.flush().await?;
            }
            other => {
                session.text(format!("event: {other:?}\r\n")).await?;
                session.flush().await?;
            }
        }
    }

    session.leave().await
}

/// Resolves the child to launch: `$EDITOR` when set, otherwise a portable no-op.
///
/// The example must run without a real editor installed (it is `no_run` in docs, but a `cargo run`
/// on a bare machine should still exit cleanly), so with no `$EDITOR` it launches `true(1)`, which
/// returns immediately with a success status. A real application would prompt for or require an
/// editor rather than silently no-op.
#[cfg(all(unix, feature = "tokio"))]
fn editor() -> std::ffi::OsString {
    std::env::var_os("EDITOR").unwrap_or_else(|| std::ffi::OsString::from("true"))
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
