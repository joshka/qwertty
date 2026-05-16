#![cfg(unix)]
//! Unix terminal input tests.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use qwertty::{
    ControlInput, InputBytes, InputDecoder, InputEvent, KeyInput, Terminal, TerminalSession,
};
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
fn input_bytes_classify_basic_arrow_keys() {
    let input = InputBytes::new(b"A\x1b[A".to_vec());

    assert_eq!(
        input.events(),
        vec![InputEvent::Text('A'), InputEvent::Key(KeyInput::Up)]
    );
}

#[test]
fn input_bytes_classify_mixed_arrow_key_text_and_controls() {
    let input = InputBytes::new(b"\x1b[Aok\r".to_vec());

    assert_eq!(
        input.events(),
        vec![
            InputEvent::Key(KeyInput::Up),
            InputEvent::Text('o'),
            InputEvent::Text('k'),
            InputEvent::Control(ControlInput::CarriageReturn),
        ]
    );
}

#[test]
fn input_bytes_preserve_unknown_escape_prefixed_input_as_undecoded() {
    let input = InputBytes::new(b"A\x1b[Z".to_vec());

    assert_eq!(
        input.events(),
        vec![
            InputEvent::Text('A'),
            InputEvent::Undecoded(InputBytes::new(b"\x1b[Z".to_vec())),
        ]
    );
}

#[test]
fn input_bytes_preserve_incomplete_escape_prefixed_input_as_undecoded() {
    let input = InputBytes::new(b"A\x1b[".to_vec());

    assert_eq!(
        input.events(),
        vec![
            InputEvent::Text('A'),
            InputEvent::Undecoded(InputBytes::new(b"\x1b[".to_vec())),
        ]
    );
}

#[test]
fn input_bytes_classify_complete_utf8_text() {
    let input = InputBytes::new("é".as_bytes().to_vec());

    assert_eq!(input.events(), vec![InputEvent::Text('é')]);
}

#[test]
fn input_bytes_classify_utf8_without_swallowing_later_controls() {
    let input = InputBytes::new("é\r".as_bytes().to_vec());

    assert_eq!(
        input.events(),
        vec![
            InputEvent::Text('é'),
            InputEvent::Control(ControlInput::CarriageReturn),
        ]
    );
}

#[test]
fn input_bytes_preserve_incomplete_utf8_as_undecoded() {
    let input = InputBytes::new(vec![0xc3]);

    assert_eq!(
        input.events(),
        vec![InputEvent::Undecoded(InputBytes::new(vec![0xc3]))]
    );
}

#[test]
fn input_bytes_preserve_invalid_utf8_as_undecoded() {
    let input = InputBytes::new(vec![0xc3, b'A']);

    assert_eq!(
        input.events(),
        vec![InputEvent::Undecoded(InputBytes::new(vec![0xc3, b'A'])),]
    );
}

#[test]
fn input_decoder_buffers_split_utf8_text() {
    let mut decoder = InputDecoder::new();

    assert_eq!(decoder.decode([0xc3]), Vec::<InputEvent>::new());
    assert_eq!(decoder.pending_bytes(), &[0xc3]);
    assert_eq!(decoder.decode([0xa9]), vec![InputEvent::Text('é')]);
    assert!(decoder.pending_bytes().is_empty());
    assert_eq!(decoder.finish(), Vec::<InputEvent>::new());
}

#[test]
fn input_decoder_buffers_split_arrow_key_input() {
    let mut decoder = InputDecoder::new();

    assert_eq!(decoder.decode([0x1b]), Vec::<InputEvent>::new());
    assert_eq!(decoder.pending_bytes(), b"\x1b");
    assert_eq!(decoder.decode(b"["), Vec::<InputEvent>::new());
    assert_eq!(decoder.pending_bytes(), b"\x1b[");
    assert_eq!(decoder.decode(b"A"), vec![InputEvent::Key(KeyInput::Up)]);
    assert!(decoder.pending_bytes().is_empty());
}

#[test]
fn input_decoder_classifies_mixed_input_after_buffered_key() {
    let mut decoder = InputDecoder::new();

    assert_eq!(decoder.decode(b"A\x1b["), vec![InputEvent::Text('A')],);
    assert_eq!(
        decoder.decode(b"B\r"),
        vec![
            InputEvent::Key(KeyInput::Down),
            InputEvent::Control(ControlInput::CarriageReturn),
        ]
    );
}

#[test]
fn input_decoder_preserves_split_invalid_utf8_as_undecoded() {
    let mut decoder = InputDecoder::new();

    assert_eq!(decoder.decode([0xc3]), Vec::<InputEvent>::new());
    assert_eq!(
        decoder.decode([b'A']),
        vec![InputEvent::Undecoded(InputBytes::new(vec![0xc3, b'A'])),]
    );
    assert_eq!(decoder.finish(), Vec::<InputEvent>::new());
}

#[test]
fn input_decoder_preserves_split_unsupported_escape_input_as_undecoded() {
    let mut decoder = InputDecoder::new();

    assert_eq!(decoder.decode(b"\x1b["), Vec::<InputEvent>::new());
    assert_eq!(
        decoder.decode(b"Z"),
        vec![InputEvent::Undecoded(InputBytes::new(b"\x1b[Z".to_vec())),]
    );
    assert!(decoder.pending_bytes().is_empty());
}

#[test]
fn input_decoder_finish_flushes_incomplete_utf8_as_undecoded() {
    let mut decoder = InputDecoder::new();

    assert_eq!(decoder.decode([0xc3]), Vec::<InputEvent>::new());
    assert_eq!(
        decoder.finish(),
        vec![InputEvent::Undecoded(InputBytes::new(vec![0xc3]))]
    );
    assert!(decoder.pending_bytes().is_empty());
    assert_eq!(decoder.finish(), Vec::<InputEvent>::new());
}

#[test]
fn input_decoder_finish_flushes_incomplete_escape_as_undecoded() {
    let mut decoder = InputDecoder::new();

    assert_eq!(decoder.decode(b"\x1b["), Vec::<InputEvent>::new());
    assert_eq!(
        decoder.finish(),
        vec![InputEvent::Undecoded(InputBytes::new(b"\x1b[".to_vec()))]
    );
    assert!(decoder.pending_bytes().is_empty());
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
fn key_input_reports_documented_bytes() {
    assert_eq!(KeyInput::Up.as_bytes(), b"\x1b[A");
    assert_eq!(KeyInput::Down.as_bytes(), b"\x1b[B");
    assert_eq!(KeyInput::Right.as_bytes(), b"\x1b[C");
    assert_eq!(KeyInput::Left.as_bytes(), b"\x1b[D");
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
