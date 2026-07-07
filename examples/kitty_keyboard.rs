//! Request kitty keyboard protocol flags with verify-after-push, then decode rich key events.
//!
//! This opens a Tokio-backed session, pushes the caller-chosen progressive-enhancement flags,
//! verifies which the terminal actually granted (design 06), and — when press/release reporting was
//! granted — prints decoded key events including releases and modifiers until Escape is pressed.
//! On a terminal that never answers the flags query the grant is *unknown* (not unsupported), and
//! the example says so rather than assuming an enhancement it did not get.
//!
//! Run over a real terminal that speaks the kitty keyboard protocol (kitty, ghostty, foot, recent
//! `WezTerm`) to see releases; on others it degrades to ordinary key events.

#[cfg(all(unix, feature = "tokio"))]
use std::time::Duration;

#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Event, Key, KeyEventKind, KittyKeyboardFlags, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    // Ask for the two flags an application most often wants: unambiguous escape codes (no bare-ESC
    // timing guess) and press/repeat/release reporting.
    let requested =
        KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.union(KittyKeyboardFlags::REPORT_EVENT_TYPES);
    let grant = session
        .request_kitty_keyboard(requested, Duration::from_millis(250))
        .await?;

    if grant.is_unknown() {
        session
            .text("terminal did not answer the kitty flags query; support is unknown\r\n")
            .await?;
        session.flush().await?;
        return session.leave().await;
    }

    let granted = grant.granted().unwrap_or_else(KittyKeyboardFlags::empty);
    session
        .text(format!(
            "requested flags {:#07b}, granted {:#07b}; press keys, Escape to exit\r\n",
            requested.bits(),
            granted.bits(),
        ))
        .await?;
    session.flush().await?;

    loop {
        match session.next_event().await? {
            Event::Key(key) => {
                if key.key() == Key::Escape {
                    break;
                }
                let kind = match key.kind() {
                    KeyEventKind::Press => "press",
                    KeyEventKind::Repeat => "repeat",
                    KeyEventKind::Release => "release",
                    other => {
                        // The vocabulary is non-exhaustive; name anything new generically.
                        session.text(format!("{other:?} ")).await?;
                        "kind"
                    }
                };
                let text = key.text().map_or("", |t| t.as_str());
                session
                    .text(format!(
                        "{kind}: {:?} mods {:?} text {text:?}\r\n",
                        key.key(),
                        key.modifiers(),
                    ))
                    .await?;
                session.flush().await?;
            }
            Event::Syntax(token) => {
                session
                    .text(format!("syntax: {:?}\r\n", token.as_bytes()))
                    .await?;
                session.flush().await?;
            }
            event => {
                session.text(format!("event: {event:?}\r\n")).await?;
                session.flush().await?;
            }
        }
    }

    // Leaving pops the granted kitty flags off the terminal's stack (verify-after-push teardown).
    session.leave().await
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
