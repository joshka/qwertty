#![cfg(windows)]
#![allow(
    unsafe_code,
    reason = "the windows-only test attaches a console via the AllocConsole FFI, which has no safe \
              wrapper — the same sanctioned #[cfg(windows)] opt-in as the device module (ADR 0021)"
)]
//! Windows console device integration tests.
//!
//! These drive the **public** [`Terminal`] surface (as a downstream crate sees it) against a real
//! console on the `windows-latest` CI host, complementing the in-crate live tests that reach the
//! device internals. A console is attached with `AllocConsole` before the tests run, and access is
//! serialized because the console modes and codepage are process-global state.
//!
//! Reading real input is not exercised: nothing types on CI. The read path's logic coverage lives
//! in the crate's platform-neutral translation tests, which run on every platform.

use std::sync::{Mutex, Once};

use qwertty::{DeviceMode, Terminal, TerminalDevice};

/// Serializes console access across tests: the mode and codepage are process-global.
static CONSOLE: Mutex<()> = Mutex::new(());
/// Attaches a console exactly once for the whole test binary.
static ALLOC: Once = Once::new();

/// Attaches a console if the process has none, then takes the serialization lock.
fn console_guard() -> std::sync::MutexGuard<'static, ()> {
    ALLOC.call_once(|| {
        // SAFETY: AllocConsole takes no arguments and fails harmlessly when a console already
        // exists; its result is intentionally ignored.
        let _ = unsafe { windows_sys::Win32::System::Console::AllocConsole() };
    });
    CONSOLE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn public_surface_opens_records_path_and_reports_size() {
    let _guard = console_guard();
    let terminal = Terminal::open_path("CONIN$").expect("open console");
    assert_eq!(terminal.path(), std::path::Path::new("CONIN$"));

    // size() is either a real positive measurement or the typed degenerate error — never a panic.
    match terminal.size() {
        Ok(size) => {
            assert!(size.columns() > 0);
            assert!(size.rows() > 0);
        }
        Err(error) => {
            let rendered = error.to_string();
            assert!(
                rendered.contains("degenerate") || rendered.contains("size"),
                "{rendered}"
            );
        }
    }
}

#[test]
fn public_surface_enters_and_leaves_raw_mode_through_the_trait() {
    let _guard = console_guard();
    let mut terminal = Terminal::open().expect("open console");

    // Drive the substitutable trait seam a session uses, not just the inherent methods.
    terminal.set_mode(DeviceMode::Raw).expect("enter raw mode");
    terminal.write_all(b"\x1b[0m").expect("write VT reset");
    terminal.flush().expect("flush is a success");
    terminal
        .set_mode(DeviceMode::Cooked)
        .expect("restore cooked mode");
}
