//! Measure how many terminal columns a string occupies with [`qwertty::width_of`].
//!
//! `width_of(s, caps)` uses a static `unicode-width` baseline, corrected for the grapheme clusters
//! where real terminals disagree with that table (ZWJ emoji, skin-tone modifiers, flags, VS16) by a
//! per-terminal deviation table measured from live conformance and keyed on the terminal's identity
//! and observed mode-2027 state (design 09-width). It performs no terminal I/O and never changes
//! terminal state — you pass in the `Capabilities` you already probed (see
//! `probe_capabilities.rs`).
//!
//! This probes the terminal running the example, then prints the measured width of a sample line —
//! the same width the same string would occupy if you drew it. On a terminal whose width behaviour
//! conformance has profiled, a ZWJ/skin-tone/flag/VS16 cluster is measured from that terminal's
//! table rather than the static number.

#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{TokioTerminalSession, width_of};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    // Identity + the mode-2027 state are what `width_of` reads; one probe, one deadline.
    let caps = session
        .probe_capabilities(Duration::from_millis(150))
        .await?;

    // A mix of the easy cases and the hard ones (ZWJ family, skin tone, flag, VS16 heart).
    let samples = [
        "hello",
        "中文",
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}", // ZWJ family
        "\u{1F44D}\u{1F3FD}",                                           // thumbs-up + skin tone
        "\u{1F1FA}\u{1F1F8}",                                           // US flag
        "\u{2764}\u{FE0F}",                                             // heart + VS16
    ];

    let program = caps
        .identity
        .program
        .as_ref()
        .map_or_else(|| "unknown terminal".to_string(), |p| format!("{p:?}"));
    session.text(format!("width_of on {program}:\r\n")).await?;
    for s in samples {
        session
            .text(format!("  {:>2} cols  {s}\r\n", width_of(s, &caps)))
            .await?;
    }
    session.flush().await?;
    session.leave().await
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
