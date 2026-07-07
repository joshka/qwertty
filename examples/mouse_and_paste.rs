//! Enable mouse, focus, and bracketed-paste reporting, then print the decoded events.
//!
//! This opens a Tokio-backed session and turns on three input modes: SGR mouse tracking (button
//! events, DEC 1002 + 1006), focus reporting (1004), and bracketed paste (2004). Each enable is
//! recorded in the session's mode ledger, so leaving — or a panic — resets them. It then prints
//! every decoded [`Event`] until Escape is pressed: mouse presses/releases/drags/scroll (never
//! coalesced), focus gain/loss, and paste segments (with line endings normalized and control bytes
//! flagged for hygiene).
//!
//! Run it in a terminal that reports mouse and focus, then click, scroll, switch window focus, and
//! paste some text (try pasting multiple lines, and text containing an escape sequence) to see the
//! typed events.

#[cfg(all(unix, feature = "tokio"))]
use qwertty::commands::terminal::MouseMode;
#[cfg(all(unix, feature = "tokio"))]
use qwertty::event::{FocusState, MouseEventKind};
#[cfg(all(unix, feature = "tokio"))]
use qwertty::{Event, Key, TokioTerminalSession};

#[cfg(all(unix, feature = "tokio"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> qwertty::Result<()> {
    let mut session = TokioTerminalSession::open()?;

    // Turn on the three reporting modes. Each records a ledger entry whose reset bytes flow into
    // the emergency blob, so a panic or leave turns the modes back off.
    session.enable_mouse(MouseMode::ButtonEvent).await?;
    session.enable_focus_events().await?;
    session.enable_bracketed_paste().await?;

    session
        .text("mouse, focus, and paste enabled; click, scroll, focus, paste — Escape to exit\r\n")
        .await?;
    session.flush().await?;

    loop {
        match session.next_event().await? {
            Event::Key(key) if key.key() == Key::Escape => break,
            Event::Key(key) => {
                session.text(format!("key: {:?}\r\n", key.key())).await?;
            }
            Event::Mouse(mouse) => {
                // Scroll events are delivered one per wheel tick with no coalescing (FM-V6): an
                // application that wants a normalized magnitude builds it from this raw stream.
                let what = match mouse.kind() {
                    MouseEventKind::Press => "press",
                    MouseEventKind::Release => "release",
                    MouseEventKind::Moved => "move",
                    MouseEventKind::Scroll(direction) => {
                        session.text(format!("scroll {direction:?} ")).await?;
                        "scroll"
                    }
                    other => {
                        session.text(format!("{other:?} ")).await?;
                        "mouse"
                    }
                };
                session
                    .text(format!(
                        "{what}: {:?} at ({}, {}) mods {:?}\r\n",
                        mouse.button(),
                        mouse.column(),
                        mouse.row(),
                        mouse.modifiers(),
                    ))
                    .await?;
            }
            Event::Focus(focus) => {
                let state = match focus.state() {
                    FocusState::Gained => "gained",
                    FocusState::Lost => "lost",
                    other => {
                        session.text(format!("focus {other:?}\r\n")).await?;
                        continue;
                    }
                };
                session.text(format!("focus {state}\r\n")).await?;
            }
            Event::Paste(paste) => {
                // The payload is data with line endings normalized to LF. `contains_control` flags
                // pasted escape sequences so a hygiene-conscious app can strip or reject them.
                let hazard = if paste.contains_control() {
                    " (contains control bytes)"
                } else {
                    ""
                };
                let text = paste.as_str().unwrap_or("<non-UTF-8 paste>");
                session
                    .text(format!(
                        "paste[final={}, terminated={}]{hazard}: {text:?}\r\n",
                        paste.is_final(),
                        paste.terminated(),
                    ))
                    .await?;
            }
            Event::Syntax(token) => {
                session
                    .text(format!("syntax: {:?}\r\n", token.as_bytes()))
                    .await?;
            }
            other => {
                session.text(format!("event: {other:?}\r\n")).await?;
            }
        }
        session.flush().await?;
    }

    // Leaving resets mouse, focus, and paste modes in reverse enablement order.
    session.leave().await
}

#[cfg(not(all(unix, feature = "tokio")))]
fn main() {
    eprintln!("this example requires the `tokio` feature on Unix");
}
