//! Probe for kitty graphics support, then transmit, place, and delete an image — gated.
//!
//! This opens a Tokio-backed session, runs the DA1-fenced capability probe (which includes the
//! graphics protocol's own `a=q` support query and the pixel-geometry queries), and only when the
//! probe answered *supported* transmits a small generated RGBA gradient under a client-assigned
//! image id and places it at the cursor (`commands::graphics::kitty`). The terminal's
//! acknowledgement is read back and decoded as a `report::KittyGraphicsReport`, then the image is
//! explicitly deleted — placed images are app-owned content, so cleanup is the application's act,
//! never the session's.
//!
//! On a terminal that never answers the probe the finding is *unknown* (not unsupported), and the
//! example says so and draws nothing: graphics escapes never leak into a terminal that did not
//! affirm the protocol (FM-V4/R-CAP-4).
//!
//! Run under kitty, ghostty, or `WezTerm` to see the image; under tmux or alacritty it reports
//! unknown support and exits.

#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::commands::graphics::kitty::{self, Format, ImageSize, Placement};
#[cfg(all(unix, feature = "tokio"))]
use qwertty::report::KittyGraphicsReport;
#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Event, SyntaxToken, TokioTerminalSession};

/// The client-assigned image id this example owns (any nonzero u32; the app owns the id space).
#[cfg(all(unix, feature = "tokio"))]
const IMAGE_ID: u32 = 42;

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    // One DA1-fenced bundle answers everything, graphics support included.
    let caps = session
        .probe_capabilities(Duration::from_millis(250))
        .await?;

    match caps.kitty_graphics.value_copied() {
        Some(true) => {}
        Some(false) => {
            session
                .text("terminal answered the graphics query with an error; not drawing\r\n")
                .await?;
            session.flush().await?;
            return session.leave().await;
        }
        None => {
            session
                .text("terminal did not answer the graphics query; support is unknown, not drawing\r\n")
                .await?;
            session.flush().await?;
            return session.leave().await;
        }
    }

    // The probed cell geometry (CSI 16 t) is how an app would size placements; zeros stay unknown.
    let geometry = match caps.cell_size.value_copied() {
        Some(cell) => format!("{}x{} px per cell", cell.width(), cell.height()),
        None => "unknown cell geometry".to_string(),
    };
    session
        .text(format!(
            "kitty graphics supported ({geometry}); drawing...\r\n"
        ))
        .await?;

    // A generated 64x32 RGBA gradient: no image files, no image crates — the app owns pixel
    // encoding and hands qwertty the encoded bytes.
    let (width, height) = (64u32, 32u32);
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            // Channel values are bounded by construction: n * 255 / limit < 256 for n < limit.
            pixels.push(u8::try_from(x * 255 / width).expect("bounded")); // red, left to right
            pixels.push(u8::try_from(y * 255 / height).expect("bounded")); // green, top to bottom
            pixels.push(0x80); // constant blue
            pixels.push(0xff); // opaque
        }
    }

    // Transmit under this example's id (the terminal acknowledges it), then place scaled over 8
    // columns.
    session
        .command(kitty::transmit(
            IMAGE_ID,
            Format::Rgba,
            Some(ImageSize { width, height }),
            &pixels,
        ))
        .await?;
    session
        .command(kitty::place_with(IMAGE_ID, &Placement::new().columns(8)))
        .await?;
    session.flush().await?;

    // The acknowledgement arrives as an APC syntax event; decode it with the report parser.
    loop {
        match session.next_event().await? {
            Event::Syntax(SyntaxToken::Apc(apc)) => {
                if let Some(report) = KittyGraphicsReport::from_string_sequence(&apc) {
                    if report.image_id() == Some(IMAGE_ID) {
                        session
                            .text(format!(
                                "\r\nterminal acknowledged: {}\r\n",
                                report.message()
                            ))
                            .await?;
                        session.flush().await?;
                        break;
                    }
                }
            }
            Event::Key(_) => break, // don't hang if the acknowledgement was swallowed
            _ => {}
        }
    }

    session
        .text("press any key to delete the image and leave\r\n")
        .await?;
    session.flush().await?;
    let _ = session.next_event().await?;

    // Explicit cleanup: delete the placements and free the stored data. qwertty never does this
    // automatically — images are content, not session state.
    session
        .command(kitty::delete_image_and_data(IMAGE_ID))
        .await?;
    session.flush().await?;

    session.leave().await
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
