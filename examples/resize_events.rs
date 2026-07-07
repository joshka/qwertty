//! React to terminal resize events from both sources: in-band resize (mode 2048) and the
//! `SIGWINCH` fallback.
//!
//! In-band resize is preferred where the terminal supports it: enabling mode 2048 makes size
//! changes arrive as `Event::Resize` in the ordinary input stream (with pixel geometry when the
//! terminal reports it), coalesced so a resize storm collapses to one event with the final
//! geometry. The `SIGWINCH` stream is the fallback for terminals without mode 2048; qwertty
//! installs no signal handler of its own, it just hands back an awaitable stream to `select!` on.
//!
//! Run this, then resize the terminal window a few times.

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Event, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    // Prefer in-band resize: size changes then arrive as coalesced `Event::Resize` through
    // `next_event`, with no signal handling at all.
    session.enable_in_band_resize().await?;

    // The `SIGWINCH` fallback stream, for terminals that do not support mode 2048. Independent of
    // the session, so it can sit in the same `select!`.
    let mut resizes = session.resize_stream()?;

    session
        .text("resize the window; press q to quit\r\n")
        .await?;
    session.flush().await?;

    loop {
        let line = tokio::select! {
            event = session.next_event() => match event? {
                Event::Resize(resize) => {
                    let cells = resize.cells();
                    let pixels = resize.pixels();
                    format!(
                        "in-band resize: {}x{} cells, pixels {pixels:?}\r\n",
                        cells.columns(),
                        cells.rows(),
                    )
                }
                Event::Key(key) if matches!(key.key(), qwertty::Key::Char('q')) => break,
                other => format!("event: {other:?}\r\n"),
            },
            resize = resizes.next_resize() => {
                let cells = resize?.cells();
                format!(
                    "SIGWINCH resize: {}x{} cells\r\n",
                    cells.columns(),
                    cells.rows(),
                )
            }
        };
        session.text(line).await?;
        session.flush().await?;
    }

    session.leave().await
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
