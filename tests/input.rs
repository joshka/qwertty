#![cfg(unix)]
//! Unix terminal input tests.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use qwertty::{ControlInput, InputBytes, InputEvent, Terminal, TerminalSession};
use rustix::pty::{grantpt, ptsname, unlockpt};

#[test]
fn input_bytes_preserve_raw_terminal_bytes() {
    let input = InputBytes::new(b"A\x1b[A\x03".to_vec());

    assert_eq!(input.as_bytes(), b"A\x1b[A\x03");
    assert_eq!(input.len(), 5);
    assert!(!input.is_empty());
    assert_eq!(input.into_bytes(), b"A\x1b[A\x03");
}

#[test]
fn input_bytes_classify_single_byte_text_and_controls() {
    let input = InputBytes::new(b"A \t\r\x03\x7f".to_vec());

    assert_eq!(
        input.events(),
        vec![
            InputEvent::Text('A'),
            InputEvent::Text(' '),
            InputEvent::Control(ControlInput::Tab),
            InputEvent::Control(ControlInput::CarriageReturn),
            InputEvent::Control(ControlInput::Other(0x03)),
            InputEvent::Control(ControlInput::Delete),
        ]
    );
}

#[test]
fn input_bytes_preserve_escape_prefixed_input_as_undecoded() {
    let input = InputBytes::new(b"A\x1b[A".to_vec());

    assert_eq!(
        input.events(),
        vec![
            InputEvent::Text('A'),
            InputEvent::Undecoded(InputBytes::new(b"\x1b[A".to_vec())),
        ]
    );
}

#[test]
fn input_bytes_preserve_non_ascii_input_as_undecoded() {
    let input = InputBytes::new("é".as_bytes().to_vec());

    assert_eq!(
        input.events(),
        vec![InputEvent::Undecoded(InputBytes::new(
            "é".as_bytes().to_vec()
        ))]
    );
}

#[test]
fn control_input_round_trips_named_bytes() {
    assert_eq!(ControlInput::from_byte(0x00), Some(ControlInput::Null));
    assert_eq!(ControlInput::from_byte(0x08), Some(ControlInput::Backspace));
    assert_eq!(ControlInput::from_byte(0x09), Some(ControlInput::Tab));
    assert_eq!(ControlInput::from_byte(0x0a), Some(ControlInput::LineFeed));
    assert_eq!(
        ControlInput::from_byte(0x0d),
        Some(ControlInput::CarriageReturn)
    );
    assert_eq!(ControlInput::from_byte(0x1b), Some(ControlInput::Escape));
    assert_eq!(ControlInput::from_byte(0x7f), Some(ControlInput::Delete));
    assert_eq!(
        ControlInput::from_byte(0x03),
        Some(ControlInput::Other(0x03))
    );
    assert_eq!(ControlInput::from_byte(b'A'), None);
    assert_eq!(ControlInput::Other(0x03).as_byte(), 0x03);
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
