//! Select on the terminal-relevant process signals a TUI cares about, alongside input and resize.
//!
//! qwertty installs no signal handler of its own and never auto-acts on a signal (design 01): the
//! `signals()` stream is opt-in, it only *reports* the signal, and the application owns the
//! response. This demo wires the recommended responses — `Suspend` (`SIGTSTP`, `Ctrl-Z`) calls
//! `suspend`, `Continue` (`SIGCONT`, delivered when the shell brings the job back with `fg`) calls
//! `resume`, and `Terminate` (`SIGTERM`) / `Interrupt` (`SIGINT`, `Ctrl-C` when the terminal
//! delivers it as a signal) break the loop for a graceful exit.
//!
//! `SIGWINCH` is deliberately not part of this stream — that is `resize_stream`'s job. The three
//! sources (`next_event`, `resize_stream().next_resize()`, and `signals().next()`) sit together in
//! one `select!`.
//!
//! Run this, then press `Ctrl-Z` to suspend (and `fg` to resume), resize the window, or send the
//! process a `SIGTERM`/`SIGINT`. Press `q` to quit.

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Event, Key, TerminalSignal, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    // The two independent streams the app selects on alongside input. Both are opt-in and neither
    // borrows the session, so all three futures live in one `select!`.
    let mut resizes = session.resize_stream()?;
    let mut signals = session.signals()?;

    session
        .text("Ctrl-Z suspends, fg resumes, resize the window; press q to quit\r\n")
        .await?;
    session.flush().await?;

    loop {
        tokio::select! {
            event = session.next_event() => match event? {
                Event::Key(key) if matches!(key.key(), Key::Char('q')) => break,
                other => {
                    session.text(format!("event: {other:?}\r\n")).await?;
                    session.flush().await?;
                }
            },
            resize = resizes.next_resize() => {
                let cells = resize?.cells();
                let line = format!("SIGWINCH resize: {}x{}\r\n", cells.columns(), cells.rows());
                session.text(line).await?;
                session.flush().await?;
            }
            signal = signals.next() => match signal? {
                // Suspend: restore the terminal, disarm the emergency hook, and stop the process
                // group. Returns once the process is continued again.
                TerminalSignal::Suspend => session.suspend().await?,
                // Continue: re-enter raw mode and recorded modes, re-assert non-blocking, flush
                // stale shell typeahead, and queue a synthetic resize.
                TerminalSignal::Continue => session.resume(true).await?,
                // Terminate/Interrupt: exit the loop and leave the session cleanly below.
                TerminalSignal::Terminate | TerminalSignal::Interrupt => break,
                // `TerminalSignal` is `#[non_exhaustive]`: future terminal-relevant signals land
                // here. Ignore them until this app grows a response.
                _ => {}
            }
        }
    }

    session.leave().await
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
