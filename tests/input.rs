#![cfg(unix)]
//! Unix terminal input tests.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use qwertty::{
    ControlInput, CsiInput, CursorPositionReport, InputBytes, InputDecoder, InputEvent, KeyInput,
    ProtocolPosition, Terminal, TerminalSession, TerminalStatus, TerminalStatusReport,
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
fn input_bytes_classify_complete_csi_input() {
    let input = InputBytes::new(b"A\x1b[Z".to_vec());

    assert_eq!(
        input.events(),
        vec![
            InputEvent::Text('A'),
            InputEvent::Csi(CsiInput::from_bytes(b"\x1b[Z").expect("complete CSI input")),
        ]
    );
}

#[test]
fn input_bytes_preserve_unsupported_non_csi_escape_input_as_undecoded() {
    let input = InputBytes::new(b"A\x1bZ".to_vec());

    assert_eq!(
        input.events(),
        vec![
            InputEvent::Text('A'),
            InputEvent::Undecoded(InputBytes::new(b"\x1bZ".to_vec())),
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
fn input_decoder_buffers_split_csi_input() {
    let mut decoder = InputDecoder::new();

    assert_eq!(decoder.decode(b"\x1b[?"), Vec::<InputEvent>::new());
    assert_eq!(decoder.pending_bytes(), b"\x1b[?");
    assert_eq!(decoder.decode(b"25"), Vec::<InputEvent>::new());
    assert_eq!(decoder.pending_bytes(), b"\x1b[?25");
    assert_eq!(
        decoder.decode(b"n"),
        vec![InputEvent::Csi(
            CsiInput::from_bytes(b"\x1b[?25n").expect("complete CSI input")
        )]
    );
    assert!(decoder.pending_bytes().is_empty());
}

#[test]
fn input_decoder_keeps_arrow_keys_as_key_events() {
    let mut decoder = InputDecoder::new();

    assert_eq!(
        decoder.decode(b"\x1b[A\x1b[?25n"),
        vec![
            InputEvent::Key(KeyInput::Up),
            InputEvent::Csi(CsiInput::from_bytes(b"\x1b[?25n").expect("complete CSI input")),
        ]
    );
}

#[test]
fn input_decoder_preserves_split_unsupported_non_csi_escape_input_as_undecoded() {
    let mut decoder = InputDecoder::new();

    assert_eq!(decoder.decode(b"\x1b"), Vec::<InputEvent>::new());
    assert_eq!(
        decoder.decode(b"Z"),
        vec![InputEvent::Undecoded(InputBytes::new(b"\x1bZ".to_vec())),]
    );
    assert!(decoder.pending_bytes().is_empty());
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
fn input_decoder_preserves_invalid_csi_input_as_undecoded() {
    let mut decoder = InputDecoder::new();

    assert_eq!(decoder.decode(b"\x1b["), Vec::<InputEvent>::new());
    assert_eq!(
        decoder.decode([0xc3]),
        vec![InputEvent::Undecoded(InputBytes::new(vec![
            0x1b, b'[', 0xc3
        ])),]
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
fn input_decoder_finish_flushes_incomplete_csi_as_undecoded() {
    let mut decoder = InputDecoder::new();

    assert_eq!(decoder.decode(b"\x1b[?25"), Vec::<InputEvent>::new());
    assert_eq!(
        decoder.finish(),
        vec![InputEvent::Undecoded(InputBytes::new(b"\x1b[?25".to_vec()))]
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
fn csi_input_reports_documented_bytes() {
    let csi = CsiInput::from_bytes(b"\x1b[?25n").expect("complete CSI input");

    assert_eq!(csi.as_bytes(), b"\x1b[?25n");
    assert_eq!(csi.parameter_bytes(), b"?25");
    assert_eq!(csi.private_marker_bytes(), b"?");
    assert_eq!(csi.intermediate_bytes(), b"");
    assert_eq!(csi.final_byte(), b'n');
    assert_eq!(csi.clone().into_bytes(), b"\x1b[?25n");
}

#[test]
fn csi_input_reports_intermediate_bytes() {
    let csi = CsiInput::from_bytes(b"\x1b[1$u").expect("complete CSI input");

    assert_eq!(csi.parameter_bytes(), b"1");
    assert_eq!(csi.private_marker_bytes(), b"");
    assert_eq!(csi.intermediate_bytes(), b"$");
    assert_eq!(csi.final_byte(), b'u');
}

#[test]
fn csi_input_rejects_non_csi_and_incomplete_bytes() {
    assert_eq!(CsiInput::from_bytes(b"\x1bZ"), None);
    assert_eq!(CsiInput::from_bytes(b"\x1b["), None);
    assert_eq!(CsiInput::from_bytes(b"\x1b[?25"), None);
    assert_eq!(CsiInput::from_bytes(b"\x1b[?25nX"), None);
}

#[test]
fn cursor_position_report_parses_csi_position() {
    let csi = CsiInput::from_bytes(b"\x1b[12;34R").expect("complete CSI input");
    let report = CursorPositionReport::from_csi(&csi).expect("cursor position report");

    assert_eq!(report.position(), ProtocolPosition::new(12, 34));
    assert_eq!(report.row(), 12);
    assert_eq!(report.column(), 34);
}

#[test]
fn cursor_position_report_rejects_unrelated_csi() {
    let cursor_up = CsiInput::from_bytes(b"\x1b[12;34A").expect("complete CSI input");
    let private_status = CsiInput::from_bytes(b"\x1b[?25n").expect("complete CSI input");
    let with_intermediate = CsiInput::from_bytes(b"\x1b[12$R").expect("complete CSI input");

    assert_eq!(CursorPositionReport::from_csi(&cursor_up), None);
    assert_eq!(CursorPositionReport::from_csi(&private_status), None);
    assert_eq!(CursorPositionReport::from_csi(&with_intermediate), None);
}

#[test]
fn terminal_status_report_parses_ready_status() {
    let csi = CsiInput::from_bytes(b"\x1b[0n").expect("complete CSI input");
    let report = TerminalStatusReport::from_csi(&csi).expect("terminal status report");

    assert_eq!(report.status(), TerminalStatus::Ready);
    assert_eq!(report.status().parameter_bytes(), b"0");
}

#[test]
fn terminal_status_report_parses_malfunction_status() {
    let csi = CsiInput::from_bytes(b"\x1b[3n").expect("complete CSI input");
    let report = TerminalStatusReport::from_csi(&csi).expect("terminal status report");

    assert_eq!(report.status(), TerminalStatus::Malfunction);
    assert_eq!(report.status().parameter_bytes(), b"3");
}

#[test]
fn terminal_status_report_rejects_invalid_parameters() {
    for bytes in [
        b"\x1b[n".as_slice(),
        b"\x1b[1n",
        b"\x1b[03n",
        b"\x1b[0;3n",
        b"\x1b[?0n",
    ] {
        let csi = CsiInput::from_bytes(bytes).expect("complete CSI input");

        assert_eq!(TerminalStatusReport::from_csi(&csi), None);
    }
}

#[test]
fn terminal_status_report_rejects_unrelated_csi() {
    for bytes in [b"\x1b[0R".as_slice(), b"\x1b[0 n", b"\x1b[?25n"] {
        let csi = CsiInput::from_bytes(bytes).expect("complete CSI input");

        assert_eq!(TerminalStatusReport::from_csi(&csi), None);
    }
}

#[test]
fn terminal_status_report_matches_first_report_and_preserves_other_events() {
    let report_csi = CsiInput::from_bytes(b"\x1b[0n").expect("complete CSI input");
    let unrelated_csi = CsiInput::from_bytes(b"\x1b[12;34R").expect("complete CSI input");
    let events = vec![
        InputEvent::Text('a'),
        InputEvent::Csi(report_csi),
        InputEvent::Key(KeyInput::Up),
        InputEvent::Csi(unrelated_csi.clone()),
    ];

    let matched = TerminalStatusReport::match_events(events);

    assert_eq!(
        matched.report().map(TerminalStatusReport::status),
        Some(TerminalStatus::Ready)
    );
    assert_eq!(
        matched.remaining_events(),
        &[
            InputEvent::Text('a'),
            InputEvent::Key(KeyInput::Up),
            InputEvent::Csi(unrelated_csi),
        ]
    );
}

#[test]
fn terminal_status_report_match_preserves_all_events_without_match() {
    let unrelated_csi = CsiInput::from_bytes(b"\x1b[12;34R").expect("complete CSI input");
    let events = vec![
        InputEvent::Control(ControlInput::Tab),
        InputEvent::Csi(unrelated_csi),
        InputEvent::Undecoded(InputBytes::new(b"\x1bZ".to_vec())),
    ];

    let matched = TerminalStatusReport::match_events(events.clone());

    assert_eq!(matched.report(), None);
    assert_eq!(matched.remaining_events(), events.as_slice());
    assert_eq!(matched.into_parts(), (None, events));
}

#[test]
fn terminal_status_report_match_preserves_malformed_report_csi() {
    let malformed = CsiInput::from_bytes(b"\x1b[1n").expect("complete CSI input");
    let events = vec![InputEvent::Csi(malformed.clone())];

    let matched = TerminalStatusReport::match_events(events);

    assert_eq!(matched.report(), None);
    assert_eq!(matched.remaining_events(), &[InputEvent::Csi(malformed)]);
}

#[test]
fn terminal_status_report_match_preserves_duplicate_reports_after_first() {
    let first = CsiInput::from_bytes(b"\x1b[0n").expect("complete CSI input");
    let second = CsiInput::from_bytes(b"\x1b[3n").expect("complete CSI input");
    let events = vec![InputEvent::Csi(first), InputEvent::Csi(second.clone())];

    let matched = TerminalStatusReport::match_events(events);

    assert_eq!(
        matched.report().map(TerminalStatusReport::status),
        Some(TerminalStatus::Ready)
    );
    assert_eq!(matched.remaining_events(), &[InputEvent::Csi(second)]);
}

#[test]
fn cursor_position_report_rejects_invalid_parameters() {
    for bytes in [
        b"\x1b[R".as_slice(),
        b"\x1b[12R".as_slice(),
        b"\x1b[12;R".as_slice(),
        b"\x1b[;34R".as_slice(),
        b"\x1b[0;34R".as_slice(),
        b"\x1b[12;0R".as_slice(),
        b"\x1b[12;34;56R".as_slice(),
        b"\x1b[70000;34R".as_slice(),
        b"\x1b[?;34R".as_slice(),
    ] {
        let csi = CsiInput::from_bytes(bytes).expect("complete CSI input");

        assert_eq!(CursorPositionReport::from_csi(&csi), None);
    }
}

#[test]
fn cursor_position_report_matches_first_report_and_preserves_other_events() {
    let report_csi = CsiInput::from_bytes(b"\x1b[12;34R").expect("complete CSI input");
    let unrelated_csi = CsiInput::from_bytes(b"\x1b[?25n").expect("complete CSI input");
    let events = vec![
        InputEvent::Text('a'),
        InputEvent::Csi(report_csi),
        InputEvent::Key(KeyInput::Up),
        InputEvent::Csi(unrelated_csi.clone()),
    ];

    let matched = CursorPositionReport::match_events(events);

    assert_eq!(
        matched.report().map(CursorPositionReport::position),
        Some(ProtocolPosition::new(12, 34))
    );
    assert_eq!(
        matched.remaining_events(),
        &[
            InputEvent::Text('a'),
            InputEvent::Key(KeyInput::Up),
            InputEvent::Csi(unrelated_csi),
        ]
    );
}

#[test]
fn cursor_position_report_match_preserves_all_events_without_match() {
    let unrelated_csi = CsiInput::from_bytes(b"\x1b[?25n").expect("complete CSI input");
    let events = vec![
        InputEvent::Control(ControlInput::Tab),
        InputEvent::Csi(unrelated_csi),
        InputEvent::Undecoded(InputBytes::new(b"\x1bZ".to_vec())),
    ];

    let matched = CursorPositionReport::match_events(events.clone());

    assert_eq!(matched.report(), None);
    assert_eq!(matched.remaining_events(), events.as_slice());
    assert_eq!(matched.into_parts(), (None, events));
}

#[test]
fn cursor_position_report_match_preserves_malformed_report_csi() {
    let malformed = CsiInput::from_bytes(b"\x1b[12;0R").expect("complete CSI input");
    let events = vec![InputEvent::Csi(malformed.clone())];

    let matched = CursorPositionReport::match_events(events);

    assert_eq!(matched.report(), None);
    assert_eq!(matched.remaining_events(), &[InputEvent::Csi(malformed)]);
}

#[test]
fn cursor_position_report_match_preserves_duplicate_reports_after_first() {
    let first = CsiInput::from_bytes(b"\x1b[12;34R").expect("complete CSI input");
    let second = CsiInput::from_bytes(b"\x1b[56;78R").expect("complete CSI input");
    let events = vec![InputEvent::Csi(first), InputEvent::Csi(second.clone())];

    let matched = CursorPositionReport::match_events(events);

    assert_eq!(
        matched.report().map(CursorPositionReport::position),
        Some(ProtocolPosition::new(12, 34))
    );
    assert_eq!(matched.remaining_events(), &[InputEvent::Csi(second)]);
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
