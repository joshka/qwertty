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
    Error, Event, FakeDevice, FakeTerminal, Key, KeyEvent, KittyKeyboardFlags, PixelSize,
    ProtocolPosition, SyntaxParser, SyntaxToken, TerminalSize, TokioTerminalSession, commands,
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
async fn tokio_session_enables_modes_and_decodes_their_events() {
    use qwertty::commands::terminal::MouseMode;
    use qwertty::event::{FocusState, MouseButton, MouseEventKind};

    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");

    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    session
        .enable_mouse(MouseMode::ButtonEvent)
        .await
        .expect("enable mouse");
    session.enable_focus_events().await.expect("enable focus");
    session
        .enable_bracketed_paste()
        .await
        .expect("enable paste");

    // The enable bytes reached the terminal in enablement order.
    let enables = read_available_after_quiet(&mut master)
        .await
        .expect("read enable bytes");
    assert_eq!(enables, b"\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[?2004h");

    // A mouse report, a focus report, and a paste all decode to their typed events.
    master
        .write_all(b"\x1b[<0;10;20M\x1b[I\x1b[200~hi\x1b[201~")
        .expect("write input");

    let mouse = session.next_event().await.expect("mouse event");
    let mouse = mouse.mouse_event().expect("a mouse event");
    assert_eq!(mouse.kind(), MouseEventKind::Press);
    assert_eq!(mouse.button(), MouseButton::Left);
    assert_eq!((mouse.column(), mouse.row()), (10, 20));

    let focus = session.next_event().await.expect("focus event");
    assert_eq!(
        focus.focus_event().map(qwertty::FocusEvent::state),
        Some(FocusState::Gained)
    );

    let paste = session.next_event().await.expect("paste event");
    let paste = paste.paste_event().expect("a paste event");
    assert_eq!(paste.data(), b"hi");
    assert!(paste.is_final() && paste.terminated());

    // The reverse-order reset on leave and the emergency blob are covered by the sync session PTY
    // tests, which do not race the session-fd close the way a post-leave master read would here.
    session.leave().await.expect("leave Tokio session");
}

/// Builds an in-band resize report `CSI 48 ; rows ; cols ; hp ; wp t` (mode 2048).
fn resize_report(rows: u16, cols: u16, height_px: u16, width_px: u16) -> Vec<u8> {
    format!("\x1b[48;{rows};{cols};{height_px};{width_px}t").into_bytes()
}

/// A resize storm — several in-band resize reports back to back with no other input — collapses to
/// exactly one `Event::Resize` carrying the final geometry (design 01 §resize, R-IN-8, FM-G2).
#[tokio::test]
async fn tokio_session_coalesces_a_resize_storm_to_the_final_geometry() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    // A burst of four resize reports; only the last geometry (100x40) should survive, and its
    // report carries pixel geometry so the surviving event keeps it.
    let mut burst = Vec::new();
    burst.extend(resize_report(30, 80, 0, 0));
    burst.extend(resize_report(31, 82, 0, 0));
    burst.extend(resize_report(35, 90, 0, 0));
    burst.extend(resize_report(40, 100, 400, 800));
    master.write_all(&burst).expect("write resize burst");

    // Let the whole burst settle into one read so all reports buffer before delivery.
    sleep(Duration::from_millis(150)).await;

    // A terminating keystroke lets us assert the queue held nothing but the one coalesced resize.
    master.write_all(b"x").expect("write sentinel key");
    sleep(Duration::from_millis(50)).await;

    let first = timeout(Duration::from_secs(1), session.next_event())
        .await
        .expect("next_event did not hang")
        .expect("first event");
    let resize = first.resize_event().expect("first event is a resize");
    assert_eq!(resize.cells(), TerminalSize::new(100, 40));
    assert_eq!(resize.pixels(), Some(PixelSize::new(800, 400)));

    // The very next event is the sentinel key: every earlier resize was dropped, none duplicated.
    let second = timeout(Duration::from_secs(1), session.next_event())
        .await
        .expect("next_event did not hang")
        .expect("second event");
    assert_eq!(second, text_event('x'));

    session.leave().await.expect("leave Tokio session");
}

/// A resize storm interleaved with keystrokes delivers every keystroke in original order and
/// exactly one resize reflecting the final geometry, positioned where the last resize sat.
///
/// Input `R1 a R2 b R3` must deliver `a b R3` — the ordering invariant (design 01 §resize).
#[tokio::test]
async fn tokio_session_coalescing_preserves_interleaved_keys_in_order() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    let mut burst = Vec::new();
    burst.extend(resize_report(30, 80, 0, 0)); // R1
    burst.extend(b"a"); // key a
    burst.extend(resize_report(31, 85, 0, 0)); // R2
    burst.extend(b"b"); // key b
    burst.extend(resize_report(32, 90, 0, 0)); // R3 (final geometry)
    master.write_all(&burst).expect("write interleaved burst");

    sleep(Duration::from_millis(150)).await;
    // A trailing sentinel proves the resize sits before it, in R3's position.
    master.write_all(b"c").expect("write sentinel key");
    sleep(Duration::from_millis(50)).await;

    let mut delivered = Vec::new();
    for _ in 0..4 {
        let event = timeout(Duration::from_secs(1), session.next_event())
            .await
            .expect("next_event did not hang")
            .expect("event");
        delivered.push(event);
    }

    // Keys a and b keep their order; exactly one resize survives, carrying R3's geometry, in R3's
    // position (after b, before the sentinel c). R1 and R2 are dropped.
    assert_eq!(delivered[0], text_event('a'));
    assert_eq!(delivered[1], text_event('b'));
    let resize = delivered[2]
        .resize_event()
        .expect("third event is the resize");
    assert_eq!(resize.cells(), TerminalSize::new(90, 32));
    assert_eq!(delivered[3], text_event('c'));

    session.leave().await.expect("leave Tokio session");
}

/// A single resize with no other resize behind it passes through unchanged — coalescing never
/// drops the only resize.
#[tokio::test]
async fn tokio_session_delivers_a_lone_resize_unchanged() {
    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    master
        .write_all(&resize_report(24, 80, 480, 640))
        .expect("write single resize");

    let event = timeout(Duration::from_secs(1), session.next_event())
        .await
        .expect("next_event did not hang")
        .expect("resize event");
    let resize = event.resize_event().expect("a resize event");
    assert_eq!(resize.cells(), TerminalSize::new(80, 24));
    assert_eq!(resize.pixels(), Some(PixelSize::new(640, 480)));

    session.leave().await.expect("leave Tokio session");
}

/// A mouse-scroll burst is NEVER coalesced (FM-V6): every wheel tick is delivered, deliberately
/// opposite to the resize policy. This is the guard against accidentally coalescing the wrong kind.
#[tokio::test]
async fn tokio_session_never_coalesces_a_scroll_burst() {
    use qwertty::event::{MouseEventKind, ScrollDirection};

    let Some((mut master, slave_path)) = open_test_pty() else {
        return;
    };
    let mut session =
        TokioTerminalSession::open_path(slave_path).expect("open Tokio pty-backed session");

    // Five identical scroll-up ticks (`CSI < 64 ; 5 ; 5 M`). All five must arrive.
    let mut burst = Vec::new();
    for _ in 0..5 {
        burst.extend_from_slice(b"\x1b[<64;5;5M");
    }
    master.write_all(&burst).expect("write scroll burst");
    sleep(Duration::from_millis(150)).await;
    master.write_all(b"z").expect("write sentinel key");
    sleep(Duration::from_millis(50)).await;

    for _ in 0..5 {
        let event = timeout(Duration::from_secs(1), session.next_event())
            .await
            .expect("next_event did not hang")
            .expect("scroll event");
        let mouse = event.mouse_event().expect("a mouse event");
        assert_eq!(mouse.kind(), MouseEventKind::Scroll(ScrollDirection::Up));
    }
    // The sentinel follows the fifth tick: no scroll tick was dropped or merged.
    let sentinel = timeout(Duration::from_secs(1), session.next_event())
        .await
        .expect("next_event did not hang")
        .expect("sentinel event");
    assert_eq!(sentinel, text_event('z'));

    session.leave().await.expect("leave Tokio session");
}

/// Enabling in-band resize over a headless `FakeDevice` writes the enable bytes, records the ledger
/// entry, and leaving undoes it (`CSI ? 2048 h` on enable, `CSI ? 2048 l` on leave).
#[tokio::test]
async fn tokio_session_in_band_resize_lifecycle_over_fake_device() {
    let (device, mut peer) = FakeDevice::open().expect("open fake device");
    let mut session =
        TokioTerminalSession::from_device(device).expect("open Tokio session over fake device");

    session
        .enable_in_band_resize()
        .await
        .expect("enable in-band resize");
    session.flush().await.expect("flush");

    // The enable bytes reached the device.
    let enable = peer.output().expect("read enable output");
    assert!(
        enable.windows(8).any(|w| w == b"\x1b[?2048h"),
        "enable wrote CSI ? 2048 h, got {enable:?}",
    );

    session.leave().await.expect("leave Tokio session");

    // Leave undid the mode with CSI ? 2048 l.
    let undo = peer.output().expect("read undo output");
    assert!(
        undo.windows(8).any(|w| w == b"\x1b[?2048l"),
        "leave wrote CSI ? 2048 l, got {undo:?}",
    );
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

#[tokio::test]
async fn tokio_session_requests_kitty_keyboard_and_records_granted_flags() {
    // Verify-after-push over a headless fake terminal: the session pushes the requested flags,
    // queries, the fake answers a granted set, and the ledger pops the granted set on leave
    // (design 06). Here the terminal grants everything requested.
    let (device, mut terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TokioTerminalSession::from_device(device).expect("open Tokio fake session");

    let requested =
        KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.union(KittyKeyboardFlags::REPORT_EVENT_TYPES);

    let request = async move {
        let grant = session
            .request_kitty_keyboard(requested, Duration::from_secs(1))
            .await
            .expect("request kitty keyboard flags");
        (session, grant)
    };
    let peer = async {
        // The session writes the push then the query; the fake terminal answers with the granted
        // set (all requested flags = 3).
        let written = read_fake_until_available(&mut terminal)
            .await
            .expect("read push+query");
        assert_eq!(written, b"\x1b[>3u\x1b[?u");
        terminal
            .feed_input(b"\x1b[?3u")
            .expect("feed granted-flags report");
    };

    let ((session, grant), ()) = tokio::join!(request, peer);

    assert_eq!(grant.requested(), requested);
    assert_eq!(grant.granted(), Some(requested));
    assert!(grant.granted_all_requested());
    assert!(!grant.is_unknown());

    // Leaving pops the granted entry: `CSI < 1 u`, then cooked mode via termios (no bytes).
    session.leave().await.expect("leave Tokio fake session");
    let teardown = terminal.output().expect("read teardown output");
    assert!(
        teardown.windows(5).any(|w| w == b"\x1b[<1u"),
        "leave must pop the granted kitty entry with CSI < 1 u, got {teardown:?}",
    );
}

#[tokio::test]
async fn tokio_session_kitty_keyboard_grant_can_be_a_subset() {
    // Verify-after-push mismatch (helix handshake, design 06): the caller requests more than the
    // terminal grants. The grant reports the smaller set, and the ledger records the *granted*
    // reality (so teardown pops what is actually pushed), not the request.
    let (device, mut terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TokioTerminalSession::from_device(device).expect("open Tokio fake session");

    let requested = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        .union(KittyKeyboardFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES);

    let request = async move {
        let grant = session
            .request_kitty_keyboard(requested, Duration::from_secs(1))
            .await
            .expect("request kitty keyboard flags");
        (session, grant)
    };
    let peer = async {
        let written = read_fake_until_available(&mut terminal)
            .await
            .expect("read push+query");
        // Requested = 1 | 8 = 9.
        assert_eq!(written, b"\x1b[>9u\x1b[?u");
        // The terminal grants only the disambiguate bit (1).
        terminal
            .feed_input(b"\x1b[?1u")
            .expect("feed partial-grant report");
    };

    let ((session, grant), ()) = tokio::join!(request, peer);

    assert_eq!(
        grant.granted(),
        Some(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    assert!(
        !grant.granted_all_requested(),
        "the terminal granted a subset, so the request was not fully satisfied",
    );

    session.leave().await.expect("leave Tokio fake session");
    let teardown = terminal.output().expect("read teardown output");
    assert!(
        teardown.windows(5).any(|w| w == b"\x1b[<1u"),
        "leave pops the granted entry even on a partial grant, got {teardown:?}",
    );
}

#[tokio::test]
async fn tokio_session_kitty_keyboard_degrades_when_terminal_never_answers() {
    // FM-C4: a terminal that never answers the flags query leaves the grant *unknown*, not
    // unsupported. The request degrades gracefully — no error, no ledger entry, no assumed
    // enhancement — so leave has no kitty pop to emit.
    let (device, mut terminal) = FakeDevice::open().expect("open fake device");
    let mut session = TokioTerminalSession::from_device(device).expect("open Tokio fake session");

    let requested = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES;
    let grant = session
        .request_kitty_keyboard(requested, Duration::from_millis(150))
        .await
        .expect("an unanswered query degrades gracefully rather than erroring");

    assert!(grant.is_unknown(), "no answer means unknown support");
    assert_eq!(grant.granted(), None);
    assert!(!grant.granted_all_requested());

    // Drain what the session wrote: the push and the query went out, but no pop is recorded.
    let written = read_fake_until_available(&mut terminal)
        .await
        .expect("read push+query");
    assert_eq!(written, b"\x1b[>1u\x1b[?u");

    session.leave().await.expect("leave Tokio fake session");
    let teardown = terminal.output().expect("read teardown output");
    assert!(
        !teardown.windows(4).any(|w| w == b"\x1b[<u")
            && !teardown.windows(5).any(|w| w == b"\x1b[<1u"),
        "an unknown grant records no kitty entry, so leave emits no pop, got {teardown:?}",
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
