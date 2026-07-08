//! Suspend the process to the shell on a key, then resume cleanly when it returns.
//!
//! This is the suspend/resume lifecycle (design 01 §4). qwertty installs no signal handler of its
//! own: the application drives `suspend`/`resume` from its own job-control integration. Here the
//! demo presses `z` to suspend (which restores the terminal to a clean cooked state, disarms the
//! panic-safe restore handle, and sends `SIGTSTP` to the process group so the shell regains the
//! terminal), then waits for `SIGCONT` — delivered when you bring the job back with `fg` — to
//! `resume`. Resume re-enters raw mode and every recorded mode, re-asserts the readiness fd's
//! non-blocking flag, optionally flushes stale input typed at the shell, and queues a synthetic
//! `Event::Resize` so the app repaints at whatever size the terminal is now (the window may have
//! been resized while suspended).
//!
//! Run this, press `z` to suspend to your shell, then run `fg` to resume it. Press `q` to quit.

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Event, Key, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut session = TokioTerminalSession::open()?;

    // `SIGCONT` is delivered when the shell brings the job back (for example with `fg`). qwertty
    // installs no handler; the app owns this listener and calls `resume` when it fires.
    let mut cont =
        signal(SignalKind::from_raw(sigcont_number())).expect("install SIGCONT listener");

    session
        .text("press z to suspend (then `fg` to resume), q to quit\r\n")
        .await?;
    session.flush().await?;

    loop {
        match session.next_event().await? {
            Event::Key(key) if matches!(key.key(), Key::Char('q')) => break,
            Event::Key(key) if matches!(key.key(), Key::Char('z')) => {
                // Suspend: restore the terminal, disarm the emergency hook, and stop the process
                // group. This call returns only once the process is continued again.
                session.suspend().await?;

                // The process was stopped; when it is continued a `SIGCONT` arrives. Wait for it,
                // then resume — flushing stale input the user may have typed at the shell.
                cont.recv().await;
                session.resume(true).await?;

                session
                    .text("resumed; press z to suspend again, q to quit\r\n")
                    .await?;
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

/// The `SIGCONT` signal number, resolved without a libc dependency.
///
/// qwertty depends on rustix, not libc, so this example reads the platform's `SIGCONT` number from
/// rustix's signal table rather than pulling in a second signal crate for one constant.
#[cfg(all(unix, feature = "tokio"))]
fn sigcont_number() -> i32 {
    rustix::process::Signal::CONT.as_raw()
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
