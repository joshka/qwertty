//! Cycle a session's enter/leave lifecycle the way a line editor would.
//!
//! Run with `cargo run --example session_cycles`. A REPL enters raw mode for each prompt and
//! restores the terminal while the host program runs. The session lifecycle is re-entrant, so
//! one long-lived session cycles cheaply: each transition replays recorded mode actions and
//! never reopens the device. This example runs headless over a `FakeDevice` pair.

#[cfg(unix)]
fn main() -> qwertty::Result<()> {
    use qwertty::{FakeDevice, TerminalSession};

    let (device, fake_terminal) = FakeDevice::open()?;
    let mut session = TerminalSession::from_device(device)?;

    for prompt in 1..=3 {
        // Raw mode is active here: read keys, paint the prompt.
        session.text(format!("prompt {prompt}\r\n"))?.flush()?;

        // Hand the terminal back to the host program between prompts.
        session.leave()?;
        session.enter()?;
    }

    session.leave()?;

    println!(
        "modes requested across the cycles: {:?}",
        fake_terminal.modes()
    );
    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("this example requires Unix because FakeDevice is Unix-only today");
}
