//! Probe for synchronized output, then draw a capability-gated frame that degrades when
//! unsupported.
//!
//! Mode 2026 (synchronized output) lets a supporting terminal buffer a whole frame and paint it
//! atomically, avoiding the mid-redraw tearing a full repaint can otherwise show. But the begin/end
//! bytes leak raw onto terminals that do not understand them (FM-V4), so qwertty gates them on the
//! probe: [`TokioTerminalSession::synchronized`] wraps the frame only when the mode-2026 DECRQM
//! probe answered "supported", and otherwise runs the same frame un-batched. This example probes,
//! reports what the terminal answered, then draws one frame through the gate either way.

#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{ProtocolPosition, TokioTerminalSession, commands};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    // Probe once. The snapshot lives on the session; `synchronized` reads it to decide whether to
    // emit the mode-2026 wrap. A silent terminal costs one timeout and leaves the finding unknown.
    let capabilities = session
        .probe_capabilities(Duration::from_millis(250))
        .await?;
    let supported = capabilities.synchronized_output.value_copied() == Some(true);

    // Draw one frame through the gate. On a terminal that probed mode-2026-supported this emits
    // `CSI ? 2026 h` ... frame ... `CSI ? 2026 l`; anywhere else it emits just the frame (graceful
    // degradation — never the 2026 wrap into a terminal that did not answer).
    session
        .synchronized(async |s| -> qwertty::Result<()> {
            s.command(commands::screen::clear()).await?;
            s.command(commands::cursor::move_to(ProtocolPosition::ORIGIN))
                .await?;
            s.text(if supported {
                "synchronized output supported: this frame paints atomically\r\n"
            } else {
                "synchronized output unknown/unsupported: this frame paints un-batched\r\n"
            })
            .await?;
            Ok(())
        })
        .await??;

    session.flush().await?;
    session.leave().await
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
