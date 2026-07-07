#![cfg(unix)]
//! Unix terminal session tests.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use qwertty::{
    DeviceMode, Error, FakeDevice, ProtocolPosition, Terminal, TerminalSession, commands,
};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::pty::{grantpt, ptsname, unlockpt};
use rustix::termios::{LocalModes, Termios, tcgetattr};

#[test]
fn pty_session_preserves_output_order() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");

    let terminal = Terminal::open_path(slave_path).expect("open pty-backed terminal");
    let mut session = TerminalSession::from_terminal(terminal).expect("start terminal session");
    session
        .command(commands::screen::clear())
        .expect("write clear command")
        .command(commands::cursor::move_to(ProtocolPosition::new(2, 3)))
        .expect("write cursor command")
        .text("Ready")
        .expect("write text")
        .command(commands::cursor::hide())
        .expect("write hide command")
        .flush()
        .expect("flush session output");

    let bytes = read_available_after_quiet(&mut master).expect("read pty master");
    assert_eq!(bytes, b"\x1b[2J\x1b[2;3HReady\x1b[?25l");

    session.leave().expect("leave terminal session");
}

#[test]
fn pty_session_leave_restores_cooked_mode() {
    let Some((_master, slave_path)) = open_test_pty() else {
        return;
    };
    let slave = open_read_write(&slave_path).expect("open pty slave");
    let original = tcgetattr(&slave).expect("read original termios");

    let terminal = Terminal::open_path(&slave_path).expect("open pty-backed terminal");
    let mut session = TerminalSession::from_terminal(terminal).expect("start terminal session");

    let raw = tcgetattr(&slave).expect("read raw termios");
    assert_ne!(
        format!("{original:?}"),
        format!("{raw:?}"),
        "session start should enter raw mode"
    );

    session.leave().expect("leave terminal session");
    let restored = tcgetattr(&slave).expect("read restored termios");
    assert_eq!(
        termios_without_pending_input(original),
        termios_without_pending_input(restored),
        "leave should restore captured cooked mode"
    );
}

#[test]
fn pty_restore_handle_restores_from_another_thread_once() {
    let Some((_master, slave_path)) = open_test_pty() else {
        return;
    };
    let slave = open_read_write(&slave_path).expect("open pty slave");
    let original = tcgetattr(&slave).expect("read original termios");

    let terminal = Terminal::open_path(&slave_path).expect("open pty-backed terminal");
    let mut session = TerminalSession::from_terminal(terminal).expect("start terminal session");
    let restore = session.restore_handle();

    let raw = tcgetattr(&slave).expect("read raw termios");
    assert_ne!(
        format!("{original:?}"),
        format!("{raw:?}"),
        "session should have entered raw mode"
    );

    // Emergency restoration runs off-thread, the way a panic hook on a worker thread would.
    let restored = thread::spawn(move || restore.restore())
        .join()
        .expect("join restore thread");
    assert!(restored, "the emergency path should perform restoration");

    let after_restore = tcgetattr(&slave).expect("read restored termios");
    assert_eq!(
        termios_without_pending_input(original),
        termios_without_pending_input(after_restore),
        "the emergency path should restore the captured termios"
    );

    // Orderly leave after an emergency restoration is a clean no-op.
    session
        .leave()
        .expect("leave should succeed after emergency restoration");
}

#[test]
fn emergency_blob_resets_enabled_modes_on_a_panic_teardown() {
    use qwertty::commands::terminal::MouseMode;

    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");

    let terminal = Terminal::open_path(&slave_path).expect("open pty-backed terminal");
    let mut session = TerminalSession::from_terminal(terminal).expect("start terminal session");
    let restore = session.restore_handle();

    // Enable the three modes; each records a byte-based ledger entry whose undo joins the emergency
    // blob published to the restore handle.
    session
        .enable_mouse(MouseMode::ButtonEvent)
        .expect("enable mouse")
        .enable_focus_events()
        .expect("enable focus")
        .enable_bracketed_paste()
        .expect("enable paste")
        .flush()
        .expect("flush");
    // Drain the enable bytes so the next read sees only the emergency output.
    let _ = read_available_after_quiet(&mut master).expect("drain enable bytes");

    // The emergency path (as a panic hook would) writes the precomposed reset blob directly.
    let restored = thread::spawn(move || restore.restore())
        .join()
        .expect("join restore thread");
    assert!(restored, "the emergency path should perform restoration");

    let blob = read_available_after_quiet(&mut master).expect("read emergency blob");
    // The blob resets the enabled modes in reverse enablement order: paste, focus, mouse.
    assert_eq!(blob, b"\x1b[?2004l\x1b[?1004l\x1b[?1006l\x1b[?1002l");

    // Orderly leave after an emergency restoration is a clean no-op (idempotent).
    session.leave().expect("leave after emergency restoration");
}

#[test]
fn emergency_blob_contains_alternate_screen_leave_and_cursor_show() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");

    let terminal = Terminal::open_path(&slave_path).expect("open pty-backed terminal");
    let mut session = TerminalSession::from_terminal(terminal).expect("start terminal session");
    let restore = session.restore_handle();

    // Enter the alternate screen, then hide the cursor; each records a byte-based ledger entry
    // whose undo joins the emergency blob published to the restore handle.
    session
        .enter_alternate_screen()
        .expect("enter alternate screen")
        .hide_cursor()
        .expect("hide cursor")
        .flush()
        .expect("flush");
    // Drain the enter/clear/hide bytes so the next read sees only the emergency output.
    let _ = read_available_after_quiet(&mut master).expect("drain enable bytes");

    // The emergency path (as a panic hook would) writes the precomposed reset blob directly.
    let restored = thread::spawn(move || restore.restore())
        .join()
        .expect("join restore thread");
    assert!(restored, "the emergency path should perform restoration");

    let blob = read_available_after_quiet(&mut master).expect("read emergency blob");
    // The blob resets in reverse enablement order: show the cursor, then leave the alternate
    // screen — the leave bytes alone, since the emergency path never needs the entry's clear.
    assert_eq!(blob, b"\x1b[?25h\x1b[?1049l");

    // Orderly leave after an emergency restoration is a clean no-op (idempotent).
    session.leave().expect("leave after emergency restoration");
}

#[test]
fn fake_device_session_round_trips_headless() {
    let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    session
        .command(commands::screen::clear())
        .expect("write clear command")
        .text("Ready")
        .expect("write text")
        .flush()
        .expect("flush session output");

    fake_terminal.feed_input(b"q").expect("feed input");
    let mut buffer = [0; 4];
    let input = session.read_input(&mut buffer).expect("read input");

    assert_eq!(input.as_bytes(), b"q");
    assert_eq!(fake_terminal.output().expect("output"), b"\x1b[2JReady");
    assert_eq!(fake_terminal.modes(), [DeviceMode::Raw]);

    session.leave().expect("leave fake session");
    assert_eq!(fake_terminal.modes(), [DeviceMode::Raw, DeviceMode::Cooked]);
}

#[test]
fn enable_modes_write_in_order_and_leave_undoes_in_reverse() {
    use qwertty::commands::terminal::MouseMode;

    let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    // Enable mouse (button-event tracking + SGR), focus, then bracketed paste, in that order.
    session
        .enable_mouse(MouseMode::ButtonEvent)
        .expect("enable mouse")
        .enable_focus_events()
        .expect("enable focus")
        .enable_bracketed_paste()
        .expect("enable paste")
        .flush()
        .expect("flush");

    // The enable bytes were written immediately, in enablement order. `output` drains, so this is
    // exactly what was written since construction (raw mode is a device mode, not bytes).
    assert_eq!(
        fake_terminal.output().expect("output"),
        b"\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[?2004h",
    );

    session.leave().expect("leave fake session");

    // Leave writes the undo bytes in reverse enablement order: paste, focus, then mouse (SGR reset
    // before the tracking-mode reset).
    assert_eq!(
        fake_terminal.output().expect("output"),
        b"\x1b[?2004l\x1b[?1004l\x1b[?1006l\x1b[?1002l",
    );
    assert_eq!(
        fake_terminal.modes(),
        [DeviceMode::Raw, DeviceMode::Cooked],
        "raw mode is restored through the device, not written bytes",
    );
}

#[test]
fn enable_in_band_resize_writes_2048_and_leave_undoes_it() {
    let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    session
        .enable_in_band_resize()
        .expect("enable in-band resize")
        .flush()
        .expect("flush");

    // The enable wrote CSI ? 2048 h immediately.
    assert_eq!(fake_terminal.output().expect("output"), b"\x1b[?2048h",);

    session.leave().expect("leave fake session");

    // Leave undid the mode with CSI ? 2048 l.
    assert_eq!(fake_terminal.output().expect("output"), b"\x1b[?2048l",);
}

#[test]
fn re_entering_replays_enabled_modes() {
    use qwertty::commands::terminal::MouseMode;

    let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    session
        .enable_mouse(MouseMode::AnyEvent)
        .expect("enable mouse")
        .enable_bracketed_paste()
        .expect("enable paste")
        .flush()
        .expect("flush");
    session.leave().expect("leave");
    let _ = fake_terminal.output().expect("drain enables and undos");

    // Re-entering replays the enable bytes in enablement order.
    session.enter().expect("re-enter");
    session.flush().expect("flush");
    assert_eq!(
        fake_terminal.output().expect("output"),
        b"\x1b[?1003h\x1b[?1006h\x1b[?2004h",
        "re-enter replays the enabled modes in enablement order",
    );
    session.leave().expect("final leave");
}

#[test]
fn switching_mouse_mode_replaces_the_tracking_mode() {
    use qwertty::commands::terminal::MouseMode;

    let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    session
        .enable_mouse(MouseMode::Normal)
        .expect("enable normal mouse")
        .enable_mouse(MouseMode::AnyEvent)
        .expect("switch to any-event mouse")
        .flush()
        .expect("flush");
    let _ = fake_terminal.output().expect("drain both enables");
    session.leave().expect("leave");

    // Both enables were written, but the single-instance ledger entry means leave undoes only the
    // latest tracking mode (1003), never the superseded 1000.
    assert_eq!(
        fake_terminal.output().expect("output"),
        b"\x1b[?1006l\x1b[?1003l",
        "only the latest mouse mode is undone",
    );
}

#[test]
fn enter_alternate_screen_writes_enter_and_clear_and_leave_undoes_it() {
    let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    session
        .enter_alternate_screen()
        .expect("enter alternate screen")
        .flush()
        .expect("flush");

    // The apply action is the enter sequence followed by an explicit clear (R-OUT-3): some hosts
    // (mosh) do not clear the alternate buffer on 1049 the way most terminals do.
    assert_eq!(
        fake_terminal.output().expect("output"),
        b"\x1b[?1049h\x1b[2J",
    );

    session.leave().expect("leave fake session");

    // Leave undoes with the leave sequence alone; no matching clear is needed since the primary
    // buffer was never touched while alternate.
    assert_eq!(fake_terminal.output().expect("output"), b"\x1b[?1049l");
}

#[test]
fn hide_cursor_writes_hide_and_leave_shows_it_again() {
    let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    session
        .hide_cursor()
        .expect("hide cursor")
        .flush()
        .expect("flush");

    assert_eq!(fake_terminal.output().expect("output"), b"\x1b[?25l");

    session.leave().expect("leave fake session");

    // Hiding is the tracked state (FM-L3): leave restores the shown state regardless of whether
    // the application called `show_cursor` itself.
    assert_eq!(fake_terminal.output().expect("output"), b"\x1b[?25h");
}

#[test]
fn show_cursor_writes_immediately_and_is_not_undone_on_leave() {
    let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    session
        .show_cursor()
        .expect("show cursor")
        .flush()
        .expect("flush");

    assert_eq!(fake_terminal.output().expect("output"), b"\x1b[?25h");

    session.leave().expect("leave fake session");

    // Showing was never ledger-tracked (no prior hide entry), so leave writes nothing for cursor
    // visibility.
    assert_eq!(fake_terminal.output().expect("output"), b"");
}

#[test]
fn show_cursor_after_hide_cursor_is_visible_immediately_and_leave_writes_a_redundant_show() {
    let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    session
        .hide_cursor()
        .expect("hide cursor")
        .show_cursor()
        .expect("show cursor")
        .flush()
        .expect("flush");

    assert_eq!(
        fake_terminal.output().expect("output"),
        b"\x1b[?25l\x1b[?25h"
    );

    session.leave().expect("leave fake session");

    // The hide entry is still in the ledger (recording never removes an entry), so leave writes
    // one more redundant, harmless show.
    assert_eq!(fake_terminal.output().expect("output"), b"\x1b[?25h");
}

#[test]
fn ledger_undoes_raw_alt_screen_and_hidden_cursor_in_reverse_order() {
    let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    // Raw mode is already recorded at construction; enable alternate screen, then hide the
    // cursor, in that order.
    session
        .enter_alternate_screen()
        .expect("enter alternate screen")
        .hide_cursor()
        .expect("hide cursor")
        .flush()
        .expect("flush");
    let _ = fake_terminal.output().expect("drain enables");

    session.leave().expect("leave fake session");

    // Undo runs in reverse enablement order: hide-cursor's undo (show) first, then alternate
    // screen's undo (leave); raw mode is a device mode, restored through `modes()` not bytes.
    assert_eq!(
        fake_terminal.output().expect("output"),
        b"\x1b[?25h\x1b[?1049l",
        "undo order is hide-undo then altscreen-undo, then cooked mode",
    );
    assert_eq!(
        fake_terminal.modes(),
        [DeviceMode::Raw, DeviceMode::Cooked],
        "raw mode restore (cooked) is the device-mode step of the same reverse-order undo",
    );
}

#[test]
fn session_enter_and_leave_cycle_and_are_idempotent() {
    let (device, fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    session.enter().expect("entering while entered is a no-op");
    session.leave().expect("first leave");
    session.leave().expect("leaving while left is a no-op");
    session.enter().expect("re-enter");
    session.leave().expect("second leave");

    assert_eq!(
        fake_terminal.modes(),
        [
            DeviceMode::Raw,
            DeviceMode::Cooked,
            DeviceMode::Raw,
            DeviceMode::Cooked,
        ]
    );
}

#[test]
fn session_cycles_ten_thousand_times_without_drift() {
    let (device, fake_terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TerminalSession::from_device(device).expect("start fake session");

    for _ in 0..10_000 {
        session.leave().expect("cycle leave");
        session.enter().expect("cycle enter");
    }
    session.leave().expect("final leave");

    // One initial enter plus 10,000 cycles plus the final leave: every mode change is a
    // deliberate ledger replay, and nothing accumulates or drifts across cycles.
    let modes = fake_terminal.modes();
    assert_eq!(modes.len(), 20_002);
    assert_eq!(modes.first(), Some(&DeviceMode::Raw));
    assert_eq!(modes.last(), Some(&DeviceMode::Cooked));
}

#[test]
fn degenerate_device_sizes_are_never_returned() {
    let (device, mut fake_terminal) = FakeDevice::open().expect("open fake device");
    fake_terminal.set_size(qwertty::TerminalSize::new(0, 0));
    let session = TerminalSession::from_device(device).expect("start fake session");

    // The environment may legitimately provide COLUMNS/LINES in some test environments, so the
    // contract under test is only: a degenerate device size never reaches the caller.
    match session.size() {
        Ok(size) => {
            assert_ne!(size.columns(), 0);
            assert_ne!(size.rows(), 0);
        }
        Err(Error::InvalidTerminalSize { columns, rows }) => {
            assert_eq!((columns, rows), (0, 0));
        }
        Err(other) => panic!("unexpected size error: {other}"),
    }
}

#[test]
fn pty_session_reads_raw_input_bytes() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };

    let terminal = Terminal::open_path(slave_path).expect("open pty-backed terminal");
    let mut session = TerminalSession::from_terminal(terminal).expect("start terminal session");

    master
        .write_all(b"A\x1b[A\x03")
        .expect("write input bytes to pty master");
    master.flush().expect("flush pty master input");

    let bytes = read_session_input(&mut session, 5).expect("read session input bytes");
    assert_eq!(bytes, b"A\x1b[A\x03");

    session.leave().expect("leave terminal session");
}

#[test]
fn pty_session_empty_input_buffer_does_not_read() {
    let Some((_master, slave_path)) = open_test_pty() else {
        return;
    };

    let terminal = Terminal::open_path(slave_path).expect("open pty-backed terminal");
    let mut session = TerminalSession::from_terminal(terminal).expect("start terminal session");

    let input = session
        .read_input(&mut [])
        .expect("read into empty input buffer");
    assert!(input.is_empty());

    session.leave().expect("leave terminal session");
}

fn read_session_input(
    session: &mut TerminalSession,
    expected_len: usize,
) -> qwertty::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while bytes.len() < expected_len {
        let mut buffer = [0; 16];
        let input = session.read_input(&mut buffer)?;
        bytes.extend_from_slice(input.as_bytes());
    }
    Ok(bytes)
}

fn open_test_pty() -> Option<(File, PathBuf)> {
    match open_test_pty_result() {
        Ok(pty) => Some(pty),
        Err(err) => {
            eprintln!("skipping PTY-backed test: {err}");
            None
        }
    }
}

fn open_test_pty_result() -> io::Result<(File, PathBuf)> {
    let master = open_read_write("/dev/ptmx")?;
    grantpt(&master)?;
    unlockpt(&master)?;
    let slave = ptsname(&master, Vec::new())?;
    let slave = PathBuf::from(OsString::from_vec(slave.into_bytes()));
    Ok((master, slave))
}

fn open_read_write(path: impl AsRef<std::path::Path>) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    let flags = fcntl_getfl(file.as_fd())?;
    fcntl_setfl(file.as_fd(), flags | OFlags::NONBLOCK)?;
    Ok(())
}

fn read_available(master: &mut File) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0; 4096];
    loop {
        match master.read(&mut buf) {
            Ok(0) => return Ok(out),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == ErrorKind::WouldBlock => return Ok(out),
            Err(err) => return Err(err),
        }
    }
}

fn read_available_after_quiet(master: &mut File) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    for _ in 0..20 {
        let before = out.len();
        out.extend(read_available(master)?);
        if out.len() == before {
            thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(out)
}

fn termios_without_pending_input(mut termios: Termios) -> String {
    termios.local_modes -= LocalModes::PENDIN;
    format!("{termios:?}")
}
