//! Probe a terminal's capabilities with one DA1-fenced query bundle, then report what it answered.
//!
//! The probe writes every capability query plus a trailing DA1 fence in a single write and waits
//! one deadline (design 03/06). Every field of the returned `Capabilities` is `Option<T>`, where
//! `None` means *unknown, not unsupported* (FM-C4): a silent terminal, or a multiplexer that
//! swallowed the queries, yields an all-`None` result rather than a false claim that the terminal
//! lacks a feature.

#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Capabilities, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    // One write, one deadline. 150 ms is a reasonable local budget; a caller over ssh/tmux would
    // choose a larger one (the budget is the caller's, never a longer default — FM-C6/Q9).
    let caps = session
        .probe_capabilities(Duration::from_millis(150))
        .await?;

    render(&mut session, &caps).await?;
    session.flush().await?;
    session.leave().await
}

/// Prints each finding as "yes / no / unknown", making the unknown-vs-unsupported distinction
/// visible (a `None` is *unknown*, never *unsupported*).
#[cfg(all(unix, feature = "tokio"))]
async fn render(session: &mut TokioTerminalSession, caps: &Capabilities) -> qwertty::Result<()> {
    session
        .text("terminal capabilities (unknown = terminal did not answer):\r\n")
        .await?;
    session
        .text(format!(
            "  synchronized output (2026): {}\r\n",
            tri(caps.synchronized_output)
        ))
        .await?;
    session
        .text(format!(
            "  grapheme clustering (2027): {}\r\n",
            tri(caps.grapheme_clustering)
        ))
        .await?;
    session
        .text(format!(
            "  in-band resize (2048):      {}\r\n",
            tri(caps.in_band_resize)
        ))
        .await?;
    session
        .text(format!(
            "  bracketed paste (2004):     {}\r\n",
            tri(caps.bracketed_paste)
        ))
        .await?;
    session
        .text(format!(
            "  terminal version:           {}\r\n",
            caps.terminal_version.as_deref().unwrap_or("unknown")
        ))
        .await?;
    session
        .text(format!(
            "  background colour:          {}\r\n",
            caps.background_color.map_or_else(
                || "unknown".to_owned(),
                |c| format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
            )
        ))
        .await?;
    Ok(())
}

/// Renders a tri-state capability finding.
#[cfg(all(unix, feature = "tokio"))]
fn tri(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
