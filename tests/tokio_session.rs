#![cfg(all(unix, feature = "tokio"))]
//! Tokio-backed Unix terminal session tests.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use qwertty::{Error, InputEvent, KeyInput, ProtocolPosition, TokioTerminalSession, commands};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::pty::{grantpt, ptsname, unlockpt};
use rustix::termios::{LocalModes, Termios, tcgetattr};
use tokio::time::{Duration, sleep, timeout};

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
        InputEvent::Text('A')
    );
    assert_eq!(
        session.next_event().await.expect("read key event"),
        InputEvent::Key(KeyInput::Up)
    );
    assert_eq!(
        session.next_event().await.expect("read utf8 event"),
        InputEvent::Text('é')
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
        InputEvent::Text('x')
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
        InputEvent::Text('x')
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
        InputEvent::Text('x')
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

fn termios_without_pending_input(mut termios: Termios) -> String {
    termios.local_modes -= LocalModes::PENDIN;
    format!("{termios:?}")
}
