#![cfg(unix)]
//! Unix terminal device tests.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use qwertty::{CommandBuffer, Terminal, TerminalSize, commands};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::pty::{grantpt, ptsname, unlockpt};
use rustix::termios::{LocalModes, Termios, tcgetattr};

#[test]
#[ignore = "requires an interactive controlling terminal"]
fn controlling_terminal_path_and_size_are_available() {
    let terminal = Terminal::open().expect("open controlling terminal");
    assert_eq!(terminal.path(), std::path::Path::new("/dev/tty"));

    let size = terminal.size().expect("terminal size");
    assert!(size.columns() > 0, "terminal columns should be nonzero");
    assert!(size.rows() > 0, "terminal rows should be nonzero");
}

#[test]
fn terminal_size_names_cell_dimensions() {
    let size = TerminalSize::new(100, 40);

    assert_eq!(size.columns(), 100);
    assert_eq!(size.rows(), 40);
}

#[test]
fn pty_raw_mode_restores_captured_mode() {
    let Some((_master, slave_path)) = open_test_pty() else {
        return;
    };
    let slave = open_read_write(&slave_path).expect("open pty slave");
    let original = tcgetattr(&slave).expect("read original termios");

    let terminal = Terminal::open_path(&slave_path).expect("open pty-backed terminal");
    terminal.set_raw_mode().expect("enter raw mode");

    let raw = tcgetattr(&slave).expect("read raw termios");
    assert_ne!(
        format!("{original:?}"),
        format!("{raw:?}"),
        "raw mode should change termios"
    );

    terminal.set_cooked_mode().expect("restore cooked mode");
    let restored = tcgetattr(&slave).expect("read restored termios");
    assert_eq!(
        termios_without_pending_input(original),
        termios_without_pending_input(restored),
        "cooked mode should restore captured termios"
    );
}

#[test]
fn pty_drop_restores_cooked_mode_as_fallback() {
    let Some((_master, slave_path)) = open_test_pty() else {
        return;
    };
    let slave = open_read_write(&slave_path).expect("open pty slave");
    let original = tcgetattr(&slave).expect("read original termios");

    let terminal = Terminal::open_path(&slave_path).expect("open pty-backed terminal");
    terminal.set_raw_mode().expect("enter raw mode");
    drop(terminal);

    let restored = tcgetattr(&slave).expect("read restored termios");
    assert_eq!(
        termios_without_pending_input(original),
        termios_without_pending_input(restored),
        "drop should restore captured termios"
    );
}

#[test]
fn pty_write_and_flush_preserve_command_buffer_bytes() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");

    let mut terminal = Terminal::open_path(slave_path).expect("open pty-backed terminal");
    let mut output = CommandBuffer::new();
    output
        .command(commands::screen::clear())
        .text("probe")
        .command(commands::cursor::hide());

    terminal
        .write_all(output.as_bytes())
        .expect("write output bytes");
    terminal.flush().expect("flush output bytes");

    let bytes = read_available_after_quiet(&mut master).expect("read pty master");
    assert_eq!(bytes, b"\x1b[2Jprobe\x1b[?25l");
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
