//! Drive terminal-facing code headless with `FakeDevice`.
//!
//! Run with `cargo run --example fake_device`. No terminal is opened: the fake device pair
//! stands in for a live terminal, so the same code path is testable in CI and downstream unit
//! tests without a pseudoterminal.

#[cfg(unix)]
fn main() -> qwertty::Result<()> {
    use qwertty::{CommandBuffer, DeviceMode, FakeDevice, TerminalDevice, commands};

    let (mut device, mut fake_terminal) = FakeDevice::open()?;

    // Application-side code writes through the `TerminalDevice` trait exactly as it would to a
    // live terminal.
    device.set_mode(DeviceMode::Raw)?;
    let mut output = CommandBuffer::new();
    output
        .command(commands::screen::clear())
        .text("hello from a fake terminal");
    device.write_all(output.as_bytes())?;
    device.flush()?;

    // The fake terminal side scripts input and observes output.
    fake_terminal.feed_input(b"q")?;
    let mut buffer = [0; 8];
    let read = device.read(&mut buffer)?;

    device.set_mode(DeviceMode::Cooked)?;

    println!("device read {:?}", &buffer[..read]);
    println!("terminal received {:?}", fake_terminal.output()?);
    println!("modes requested {:?}", fake_terminal.modes());

    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("this example requires Unix because FakeDevice is Unix-only today");
}
