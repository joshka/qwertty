//! Probe a terminal's capabilities with one DA1-fenced query bundle, then report what it answered.
//!
//! The probe writes every capability query plus a trailing DA1 fence in a single write and waits
//! one deadline (design 03/06). Every `Capabilities` field that has a query behind it is a
//! `Finding<T>`: `.value()` is `Option<T>`, where `None` means *unknown, not unsupported* (FM-C4) —
//! a silent terminal, or a multiplexer that swallowed the queries, yields an all-unknown result
//! rather than a false claim that the terminal lacks a feature — and `.evidence()` says *how* the
//! value (or its absence) was obtained: probed (a terminal reply named the query), inferred (an
//! environment heuristic guessed, only for fields with no query at all — FM-C12), or unknown
//! (nothing probed and nothing inferred). `Capabilities::identity` is a finding too (R-CAP-5): the
//! terminal program, version, and multiplexer stack, cross-checked between the XTVERSION reply and
//! environment variables — under a multiplexer, `mux_stack` records that the probe replies describe
//! the mux, not the outer terminal (FM-C3).

#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Capabilities, Evidence, TokioTerminalSession};

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

/// Prints each finding as "yes / no / unknown (evidence)", making both the unknown-vs-unsupported
/// distinction and the probed-vs-inferred-vs-unknown provenance visible.
#[cfg(all(unix, feature = "tokio"))]
async fn render(session: &mut TokioTerminalSession, caps: &Capabilities) -> qwertty::Result<()> {
    session
        .text("terminal capabilities (unknown = terminal did not answer):\r\n")
        .await?;
    session
        .text(format!(
            "  synchronized output (2026): {}\r\n",
            tri(
                caps.synchronized_output.value_copied(),
                caps.synchronized_output.evidence()
            )
        ))
        .await?;
    session
        .text(format!(
            "  grapheme clustering (2027): {}\r\n",
            tri(
                caps.grapheme_clustering.value_copied(),
                caps.grapheme_clustering.evidence()
            )
        ))
        .await?;
    session
        .text(format!(
            "  in-band resize (2048):      {}\r\n",
            tri(
                caps.in_band_resize.value_copied(),
                caps.in_band_resize.evidence()
            )
        ))
        .await?;
    session
        .text(format!(
            "  bracketed paste (2004):     {}\r\n",
            tri(
                caps.bracketed_paste.value_copied(),
                caps.bracketed_paste.evidence()
            )
        ))
        .await?;
    session
        .text(format!(
            "  OSC 8 hyperlinks:           {}\r\n",
            tri(caps.hyperlinks.value_copied(), caps.hyperlinks.evidence())
        ))
        .await?;
    session
        .text(format!(
            "  truecolor:                  {}\r\n",
            tri(caps.truecolor.value_copied(), caps.truecolor.evidence())
        ))
        .await?;
    session
        .text(format!(
            "  background colour:          {}\r\n",
            caps.background_color.value().map_or_else(
                || "unknown".to_owned(),
                |c| format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
            )
        ))
        .await?;
    session
        .text(format!(
            "  identity:                   {}\r\n",
            caps.identity
                .program
                .as_ref()
                .map_or_else(|| "unknown".to_owned(), std::string::ToString::to_string)
        ))
        .await?;
    session
        .text(format!(
            "  version:                    {}\r\n",
            caps.identity.version.as_deref().unwrap_or("unknown")
        ))
        .await?;
    session
        .text(format!(
            "  mux stack:                  {}\r\n",
            if caps.identity.mux_stack.is_empty() {
                "none".to_owned()
            } else {
                format!("{:?}", caps.identity.mux_stack)
            }
        ))
        .await?;
    Ok(())
}

/// Renders a tri-state capability finding as "yes/no/unknown", with its evidence in parentheses.
#[cfg(all(unix, feature = "tokio"))]
fn tri(value: Option<bool>, evidence: &Evidence) -> String {
    let state = match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    };
    let source = match evidence {
        Evidence::Probed { via } => format!("probed via {via}"),
        Evidence::Inferred { via } => format!("inferred from {via}"),
        Evidence::Unknown => "no evidence".to_owned(),
        _ => "unrecognized evidence".to_owned(),
    };
    format!("{state} ({source})")
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
