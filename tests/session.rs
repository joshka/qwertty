#![cfg(unix)]
//! Unix terminal session tests.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use qwertty::{ProtocolPosition, Terminal, TerminalSession, commands};
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
    let session = TerminalSession::from_terminal(terminal).expect("start terminal session");

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
