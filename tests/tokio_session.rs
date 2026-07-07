#![cfg(all(unix, feature = "tokio"))]
//! Tokio-backed Unix terminal session tests.
//!
//! These port the twelve query/event contracts (×2 report types) that the old `InputEvent`-based
//! Tokio session proved, adapted to the new `Event`/`KeyEvent` vocabulary the M2-S2 rework
//! delivers. The PTY harness is unchanged. Two tests exercise the driver over a headless
//! `FakeDevice` with no pseudoterminal: a full query round-trip, and the cancel-sweep that
//! guarantees a dropped query's late reply is never misdelivered to the next query.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use qwertty::report::TerminalStatus;
use qwertty::{
    Error, Event, FakeDevice, FakeTerminal, Key, KeyEvent, ProtocolPosition, SyntaxParser,
    SyntaxToken, TokioTerminalSession, commands,
};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::pty::{grantpt, ptsname, unlockpt};
use rustix::termios::{LocalModes, Termios, tcgetattr};
use tokio::time::{Duration, sleep, timeout};

/// Builds the semantic key event a single printable character decodes to.
fn text_event(character: char) -> Event {
    Event::Key(KeyEvent::new(Key::Char(character)).with_text(character))
}

/// Builds the passthrough `Event::Syntax` a complete CSI sequence decodes to.
///
/// This is the shape a query-shaped reply that no live query claimed surfaces as through
/// `next_event`: lossless CSI syntax, byte-for-byte.
fn csi_event(bytes: &[u8]) -> Event {
    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(bytes);
    tokens.extend(parser.finish());
    assert_eq!(tokens.len(), 1, "expected exactly one token from {bytes:?}");
    match tokens.into_iter().next().expect("one token") {
        token @ SyntaxToken::Csi(_) => Event::Syntax(token),
        other => panic!("expected a CSI token, got {other:?}"),
    }
}

#[tokio::test]
async fn tokio_session_preserves_output_order() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");

    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");
    session
        .command(commands::screen::clear())
        .await
        .expect("write clear command");
    session
        .command(commands::cursor::move_to(ProtocolPosition::new(2, 3)))
        .await
        .expect("write cursor command");
    session.text("Ready").await.expect("write text");
    session
        .command(commands::cursor::hide())
        .await
        .expect("write hide command");
    session.flush().await.expect("flush session output");

    let bytes = read_available_after_quiet(&mut master)
        .await
        .expect("read pty master");
    assert_eq!(bytes, b"\x1b[2J\x1b[2;3HReady\x1b[?25l");

    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_reads_raw_input_bytes() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    master.write_all(b"abc").expect("write pty input");

    let mut buffer = [0; 8];
    let input = session
        .read_input(&mut buffer)
        .await
        .expect("read Tokio input bytes");
    assert_eq!(input.as_bytes(), b"abc");

    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_delivers_decoded_events() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    master
        .write_all(b"A\x1b[A\xc3\xa9")
        .expect("write pty input");

    assert_eq!(
        session.next_event().await.expect("read text event"),
        text_event('A')
    );
    assert_eq!(
        session.next_event().await.expect("read key event"),
        Event::Key(KeyEvent::new(Key::Up))
    );
    assert_eq!(
        session.next_event().await.expect("read utf8 event"),
        text_event('é')
    );

    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_requests_cursor_position() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let report = session
            .request_cursor_position(Duration::from_secs(1))
            .await
            .expect("request cursor position");
        (session, report)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read cursor position request");
        assert_eq!(request, b"\x1b[6n");
        master
            .write_all(b"\x1b[12;34R")
            .expect("write cursor position report");
    };

    let ((session, report), ()) = tokio::join!(query, peer);

    assert_eq!(report.position(), ProtocolPosition::new(12, 34));
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_cursor_query_preserves_unrelated_events() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let report = session
            .request_cursor_position(Duration::from_secs(1))
            .await
            .expect("request cursor position");
        (session, report)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read cursor position request");
        assert_eq!(request, b"\x1b[6n");
        master
            .write_all(b"x\x1b[12;34R")
            .expect("write unrelated input and report");
    };

    let ((mut session, report), ()) = tokio::join!(query, peer);

    assert_eq!(report.position(), ProtocolPosition::new(12, 34));
    assert_eq!(
        session
            .next_event()
            .await
            .expect("read preserved unrelated event"),
        text_event('x')
    );
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_cursor_query_timeout_preserves_unrelated_events() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let result = session
            .request_cursor_position(Duration::from_millis(100))
            .await;
        (session, result)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read cursor position request");
        assert_eq!(request, b"\x1b[6n");
        master.write_all(b"x").expect("write unrelated input");
    };

    let ((mut session, result), ()) = tokio::join!(query, peer);

    assert!(matches!(
        result,
        Err(Error::QueryTimeout {
            operation: "cursor position query",
            ..
        })
    ));
    assert_eq!(
        session
            .next_event()
            .await
            .expect("read preserved unrelated event"),
        text_event('x')
    );
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_cursor_query_wrong_report_becomes_next_csi_event() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let result = session
            .request_cursor_position(Duration::from_millis(100))
            .await;
        (session, result)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read cursor position request");
        assert_eq!(request, b"\x1b[6n");
        master
            .write_all(b"\x1b[0n")
            .expect("write terminal status report");
    };

    let ((mut session, result), ()) = tokio::join!(query, peer);

    assert!(matches!(
        result,
        Err(Error::QueryTimeout {
            operation: "cursor position query",
            ..
        })
    ));
    assert_eq!(
        session
            .next_event()
            .await
            .expect("read preserved wrong-report csi"),
        csi_event(b"\x1b[0n")
    );
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_cursor_query_unmatched_csi_becomes_next_event() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let result = session
            .request_cursor_position(Duration::from_millis(100))
            .await;
        (session, result)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read cursor position request");
        assert_eq!(request, b"\x1b[6n");
        master
            .write_all(b"\x1b[?25n")
            .expect("write unmatched query-shaped csi");
    };

    let ((mut session, result), ()) = tokio::join!(query, peer);

    assert!(matches!(
        result,
        Err(Error::QueryTimeout {
            operation: "cursor position query",
            ..
        })
    ));
    assert_eq!(
        session
            .next_event()
            .await
            .expect("read preserved unmatched csi"),
        csi_event(b"\x1b[?25n")
    );
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_cursor_query_closed_terminal_returns_unexpected_eof() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let result = session
            .request_cursor_position(Duration::from_secs(1))
            .await;
        (session, result)
    };
    let peer = async move {
        let request = read_until_available(&mut master)
            .await
            .expect("read cursor position request");
        assert_eq!(request, b"\x1b[6n");
        drop(master);
    };

    let ((_session, result), ()) = tokio::join!(query, peer);

    assert!(matches!(
        result,
        Err(Error::ReadTerminal { source }) if source.kind() == ErrorKind::UnexpectedEof
    ));
}

#[tokio::test]
async fn tokio_session_cursor_query_late_reply_becomes_next_csi_event() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let result = session
            .request_cursor_position(Duration::from_millis(100))
            .await;
        (session, result)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read cursor position request");
        assert_eq!(request, b"\x1b[6n");
        sleep(Duration::from_millis(150)).await;
        master
            .write_all(b"\x1b[12;34R")
            .expect("write late cursor position report");
    };

    let ((mut session, result), ()) = tokio::join!(query, peer);

    assert!(matches!(
        result,
        Err(Error::QueryTimeout {
            operation: "cursor position query",
            ..
        })
    ));
    assert_eq!(
        session.next_event().await.expect("read late cursor report"),
        csi_event(b"\x1b[12;34R")
    );
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_cursor_query_cancellation_preserves_unrelated_events() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let mut query = Box::pin(session.request_cursor_position(Duration::from_secs(1)));
    let cancellation = async {
        let result = timeout(Duration::from_millis(100), &mut query).await;
        assert!(
            result.is_err(),
            "query should stay pending until its response or timeout"
        );
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read cursor position request");
        assert_eq!(request, b"\x1b[6n");
        master.write_all(b"x").expect("write unrelated input");
    };

    let ((), ()) = tokio::join!(cancellation, peer);
    drop(query);

    assert_eq!(
        session
            .next_event()
            .await
            .expect("read preserved unrelated event"),
        text_event('x')
    );
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_leave_restores_cooked_mode() {
    let Some((_master, slave_path)) = open_test_pty() else {
        return;
    };
    let slave = open_read_write(&slave_path).expect("open pty slave");
    let original = tcgetattr(&slave).expect("read original termios");

    let session =
        TokioTerminalSession::open_path(&slave_path).expect("open Tokio pty-backed session");

    let raw = tcgetattr(&slave).expect("read raw termios");
    assert_ne!(
        format!("{original:?}"),
        format!("{raw:?}"),
        "session start should enter raw mode"
    );

    session.leave().await.expect("leave Tokio session");
    let restored = tcgetattr(&slave).expect("read restored termios");
    assert_eq!(
        termios_without_pending_input(original),
        termios_without_pending_input(restored),
        "leave should restore captured cooked mode"
    );
}

#[tokio::test]
async fn tokio_session_requests_terminal_status() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let report = session
            .request_terminal_status(Duration::from_secs(1))
            .await
            .expect("request terminal status");
        (session, report)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read terminal status request");
        assert_eq!(request, b"\x1b[5n");
        master
            .write_all(b"\x1b[0n")
            .expect("write terminal status report");
    };

    let ((session, report), ()) = tokio::join!(query, peer);

    assert_eq!(report.status(), TerminalStatus::Ready);
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_terminal_status_query_preserves_unrelated_events() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let report = session
            .request_terminal_status(Duration::from_secs(1))
            .await
            .expect("request terminal status");
        (session, report)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read terminal status request");
        assert_eq!(request, b"\x1b[5n");
        master
            .write_all(b"x\x1b[3n")
            .expect("write unrelated input and report");
    };

    let ((mut session, report), ()) = tokio::join!(query, peer);

    assert_eq!(report.status(), TerminalStatus::Malfunction);
    assert_eq!(
        session
            .next_event()
            .await
            .expect("read preserved unrelated event"),
        text_event('x')
    );
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_terminal_status_query_timeout_preserves_unrelated_events() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let result = session
            .request_terminal_status(Duration::from_millis(100))
            .await;
        (session, result)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read terminal status request");
        assert_eq!(request, b"\x1b[5n");
        master.write_all(b"x").expect("write unrelated input");
    };

    let ((mut session, result), ()) = tokio::join!(query, peer);

    assert!(matches!(
        result,
        Err(Error::QueryTimeout {
            operation: "terminal status query",
            ..
        })
    ));
    assert_eq!(
        session
            .next_event()
            .await
            .expect("read preserved unrelated event"),
        text_event('x')
    );
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_terminal_status_query_wrong_report_becomes_next_csi_event() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let result = session
            .request_terminal_status(Duration::from_millis(100))
            .await;
        (session, result)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read terminal status request");
        assert_eq!(request, b"\x1b[5n");
        master
            .write_all(b"\x1b[12;34R")
            .expect("write cursor position report");
    };

    let ((mut session, result), ()) = tokio::join!(query, peer);

    assert!(matches!(
        result,
        Err(Error::QueryTimeout {
            operation: "terminal status query",
            ..
        })
    ));
    assert_eq!(
        session
            .next_event()
            .await
            .expect("read preserved wrong-report csi"),
        csi_event(b"\x1b[12;34R")
    );
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_terminal_status_query_unmatched_csi_becomes_next_event() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let result = session
            .request_terminal_status(Duration::from_millis(100))
            .await;
        (session, result)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read terminal status request");
        assert_eq!(request, b"\x1b[5n");
        master
            .write_all(b"\x1b[?25n")
            .expect("write unmatched query-shaped csi");
    };

    let ((mut session, result), ()) = tokio::join!(query, peer);

    assert!(matches!(
        result,
        Err(Error::QueryTimeout {
            operation: "terminal status query",
            ..
        })
    ));
    assert_eq!(
        session
            .next_event()
            .await
            .expect("read preserved unmatched csi"),
        csi_event(b"\x1b[?25n")
    );
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_terminal_status_query_closed_terminal_returns_unexpected_eof() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let result = session
            .request_terminal_status(Duration::from_secs(1))
            .await;
        (session, result)
    };
    let peer = async move {
        let request = read_until_available(&mut master)
            .await
            .expect("read terminal status request");
        assert_eq!(request, b"\x1b[5n");
        drop(master);
    };

    let ((_session, result), ()) = tokio::join!(query, peer);

    assert!(matches!(
        result,
        Err(Error::ReadTerminal { source }) if source.kind() == ErrorKind::UnexpectedEof
    ));
}

#[tokio::test]
async fn tokio_session_terminal_status_query_late_reply_becomes_next_csi_event() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let query = async move {
        let result = session
            .request_terminal_status(Duration::from_millis(100))
            .await;
        (session, result)
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read terminal status request");
        assert_eq!(request, b"\x1b[5n");
        sleep(Duration::from_millis(150)).await;
        master
            .write_all(b"\x1b[0n")
            .expect("write late terminal status report");
    };

    let ((mut session, result), ()) = tokio::join!(query, peer);

    assert!(matches!(
        result,
        Err(Error::QueryTimeout {
            operation: "terminal status query",
            ..
        })
    ));
    assert_eq!(
        session
            .next_event()
            .await
            .expect("read late terminal status report"),
        csi_event(b"\x1b[0n")
    );
    session.leave().await.expect("leave Tokio session");
}

#[tokio::test]
async fn tokio_session_terminal_status_query_cancellation_preserves_unrelated_events() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let mut query = Box::pin(session.request_terminal_status(Duration::from_secs(1)));
    let cancellation = async {
        let result = timeout(Duration::from_millis(100), &mut query).await;
        assert!(
            result.is_err(),
            "query should stay pending until its response or timeout"
        );
    };
    let peer = async {
        let request = read_until_available(&mut master)
            .await
            .expect("read terminal status request");
        assert_eq!(request, b"\x1b[5n");
        master.write_all(b"x").expect("write unrelated input");
    };

    let ((), ()) = tokio::join!(cancellation, peer);
    drop(query);

    assert_eq!(
        session
            .next_event()
            .await
            .expect("read preserved unrelated event"),
        text_event('x')
    );
    session.leave().await.expect("leave Tokio session");
}

// --- FakeDevice-driven tests (no PTY): the runtime-neutral-core payoff ---------------------------

#[tokio::test]
async fn tokio_session_over_fake_device_round_trips_a_query() {
    // The whole point of `from_device`: a headless fake terminal drives the real Tokio session,
    // proving the query round-trip with no pseudoterminal (R-TST-1).
    let (device, mut terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TokioTerminalSession::from_device(device).expect("open Tokio fake session");

    let query = async move {
        let report = session
            .request_cursor_position(Duration::from_secs(1))
            .await
            .expect("request cursor position over fake device");
        (session, report)
    };
    let peer = async {
        // The fake terminal answers the request the session just wrote.
        let request = read_fake_until_available(&mut terminal)
            .await
            .expect("read cursor position request from fake terminal");
        assert_eq!(request, b"\x1b[6n");
        terminal
            .feed_input(b"\x1b[7;9R")
            .expect("feed cursor position report");
    };

    let ((session, report), ()) = tokio::join!(query, peer);

    assert_eq!(report.position(), ProtocolPosition::new(7, 9));
    session.leave().await.expect("leave Tokio fake session");
}

#[tokio::test]
async fn tokio_session_cancel_sweep_does_not_misdeliver_late_reply() {
    // Drop a cursor query mid-await (its expectation stays registered), then run a second cursor
    // query. The first query's late reply must NOT complete the second query: the cancel-sweep
    // resolves the abandoned expectation as Cancelled, so the late reply passes through as an
    // event, and the second query completes only with its own, distinct reply.
    let (device, mut terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TokioTerminalSession::from_device(device).expect("open Tokio fake session");

    // First query: start it, let it write the request, then cancel it by dropping the future while
    // it is still awaiting a reply.
    {
        let mut first = Box::pin(session.request_cursor_position(Duration::from_secs(1)));
        let cancellation = async {
            let result = timeout(Duration::from_millis(100), &mut first).await;
            assert!(result.is_err(), "first query stays pending until cancelled");
        };
        let peer = async {
            let request = read_fake_until_available(&mut terminal)
                .await
                .expect("read first cursor request");
            assert_eq!(request, b"\x1b[6n");
        };
        let ((), ()) = tokio::join!(cancellation, peer);
        // The first query's expectation is still registered here; dropping it does not resolve it.
    }

    // Now the abandoned reply for the first query and a fresh reply for the second both flow in.
    // The second query must complete with the second reply; the first reply must surface as an
    // event.
    let query = async move {
        let report = session
            .request_cursor_position(Duration::from_secs(1))
            .await
            .expect("second cursor query completes with its own reply");
        (session, report)
    };
    let peer = async {
        // The second query writes its own request; answer the first query late, then the second.
        let request = read_fake_until_available(&mut terminal)
            .await
            .expect("read second cursor request");
        assert_eq!(request, b"\x1b[6n");
        // The stale first reply arrives first, followed by the second query's real reply.
        terminal
            .feed_input(b"\x1b[1;1R\x1b[5;6R")
            .expect("feed late first reply then second reply");
    };

    let ((mut session, report), ()) = tokio::join!(query, peer);

    // The second query got its own reply, not the stale one.
    assert_eq!(report.position(), ProtocolPosition::new(5, 6));

    // The stale first reply was not misdelivered; it passes through as an ordinary event. A row-1
    // two-parameter CPR (`1;1`) is the modified-F3-ambiguous shape the correlator refuses to match,
    // so it is guaranteed to be a passthrough here regardless of query state — exactly the "late
    // reply is never misdelivered" contract.
    assert_eq!(
        session.next_event().await.expect("read stale first reply"),
        csi_event(b"\x1b[1;1R")
    );

    session.leave().await.expect("leave Tokio fake session");
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

async fn read_available_after_quiet(master: &mut File) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    for _ in 0..20 {
        let before = out.len();
        out.extend(read_available(master)?);
        if out.len() == before {
            sleep(Duration::from_millis(10)).await;
        }
    }
    Ok(out)
}

async fn read_until_available(master: &mut File) -> io::Result<Vec<u8>> {
    for _ in 0..20 {
        let bytes = read_available(master)?;
        if !bytes.is_empty() {
            return Ok(bytes);
        }

        sleep(Duration::from_millis(10)).await;
    }

    Ok(Vec::new())
}

/// Polls the fake terminal for output the session has written, giving the write time to arrive.
async fn read_fake_until_available(terminal: &mut FakeTerminal) -> qwertty::Result<Vec<u8>> {
    for _ in 0..20 {
        let bytes = terminal.output()?;
        if !bytes.is_empty() {
            return Ok(bytes);
        }
        sleep(Duration::from_millis(10)).await;
    }
    Ok(Vec::new())
}

fn termios_without_pending_input(mut termios: Termios) -> String {
    termios.local_modes -= LocalModes::PENDIN;
    format!("{termios:?}")
}
