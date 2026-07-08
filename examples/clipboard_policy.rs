//! Gate an OSC 52 clipboard write behind the session security policy.
//!
//! Run with `cargo run --example clipboard_policy`. No terminal is opened: a `FakeDevice` pair
//! stands in for a live terminal (see `fake_device.rs`), so the policy gate is exercised headless
//! in CI without a pseudoterminal.
//!
//! A `trusted()` policy allows the clipboard write; a hand-built restricted policy with clipboard
//! write turned off denies it, and the denial is a typed `Error::PolicyDenied` naming the gate.

#[cfg(unix)]
fn main() -> qwertty::Result<()> {
    use qwertty::commands::osc::ClipboardSelection;
    use qwertty::{Error, FakeDevice, Policy, PolicyGate, TerminalSession};

    let (device, mut fake_terminal) = FakeDevice::open()?;

    // A trusted policy allows every gated feature, including clipboard write.
    let mut session = TerminalSession::from_device(device)?.with_policy(Policy::trusted());
    session
        .set_clipboard(ClipboardSelection::Clipboard, b"copied from a trusted app")?
        .flush()?;
    println!("trusted policy wrote {:?}", fake_terminal.output()?);

    // Now hand-build a restricted policy with clipboard write explicitly off and try again. The
    // write is denied before any bytes reach the terminal, and the error names the gate teachably.
    session.set_policy(Policy {
        clipboard_write: false,
        ..Policy::restricted()
    });
    match session.set_clipboard(ClipboardSelection::Clipboard, b"secret") {
        Ok(_) => println!("unexpected: the write was allowed"),
        Err(Error::PolicyDenied { gate }) => {
            println!("denied by policy: {gate}");
            assert_eq!(gate, PolicyGate::ClipboardWrite);
        }
        Err(other) => return Err(other),
    }
    // Nothing was written by the denied call.
    assert!(fake_terminal.output()?.is_empty());

    session.leave()
}

#[cfg(not(unix))]
fn main() {
    eprintln!("this example requires Unix because FakeDevice is Unix-only today");
}
