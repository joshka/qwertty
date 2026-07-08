#![cfg(unix)]
//! Zero-residue proof for the std-only one-shot query recipe (`examples/oneshot_background.rs`).
//!
//! An `examples/` binary cannot be imported by an integration test, so this test re-runs the
//! recipe's exact public-API composition — probe, poll the session fd, one read, sans-io parse —
//! against a pseudoterminal, with the PTY master standing in for the emulator. It asserts the two
//! residue properties the recipe promises:
//!
//! - **Mode residue:** after the run the slave's termios is byte-for-byte the pre-run cooked state,
//!   so no raw mode leaks.
//! - **Byte residue:** the emulator saw *exactly* the single `CSI 6 n` probe and nothing else, and
//!   the recipe consumed the whole reply, leaving no bytes unread on the wire and none pending in
//!   the parser.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use qwertty::report::CursorPositionReport;
use qwertty::{SyntaxParser, SyntaxToken, Terminal, TerminalSession, commands};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::pty::{grantpt, ptsname, unlockpt};
use rustix::termios::{LocalModes, Termios, tcgetattr};

/// The exact bytes of the cursor-position probe the recipe writes.
const PROBE: &[u8] = b"\x1b[6n";
/// The cursor-position report the fake emulator replies with (`CSI 12 ; 34 R`).
const REPLY: &[u8] = b"\x1b[12;34R";

#[test]
fn oneshot_recipe_leaves_no_mode_or_byte_residue() {
    let Some((master, slave_path)) = open_test_pty() else {
        return;
    };
    set_nonblocking(&master).expect("set pty master nonblocking");

    // The cooked termios the recipe must restore to, captured before the session enters raw mode.
    let slave = open_read_write(&slave_path).expect("open pty slave for termios capture");
    let cooked = tcgetattr(&slave).expect("read cooked termios");

    let terminal = Terminal::open_path(&slave_path).expect("open pty-backed terminal");
    let mut session = TerminalSession::from_terminal(terminal).expect("start terminal session");

    // Sanity: the session did enter raw mode, so restoring back to `cooked` is a real change.
    let raw = tcgetattr(&slave).expect("read raw termios");
    assert_ne!(
        format!("{cooked:?}"),
        format!("{raw:?}"),
        "session should have entered raw mode"
    );

    // The emulator: read the probe off the master, assert it is exactly one `CSI 6 n` and nothing
    // more, then write the CPR reply back. Runs on its own thread so the recipe's poll+read is a
    // genuine round-trip, not a self-fed buffer. It sends the probe bytes back, then blocks on
    // `release_rx` so the master stays open until the recipe has drained the reply — dropping the
    // PTY master early can discard unread slave-bound data on some platforms.
    let (probe_tx, probe_rx) = std::sync::mpsc::channel::<io::Result<Vec<u8>>>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let emulator = thread::spawn(move || {
        let mut master = master;
        let result = (|| {
            let probe = read_available_after_quiet(&mut master)?;
            master.write_all(REPLY)?;
            master.flush()?;
            Ok(probe)
        })();
        probe_tx.send(result).expect("send probe result");
        // Keep the master open until the main thread has finished reading the reply.
        let _ = release_rx.recv();
    });

    // --- The recipe core, byte-for-byte as the example composes it -------------------------------
    session
        .command(commands::cursor::request_position())
        .expect("write probe")
        .flush()
        .expect("flush probe");

    let fd = session
        .as_fd()
        .expect("pty-backed session exposes a pollable fd");
    let timeout = Timespec {
        tv_sec: 0,
        tv_nsec: 150_000_000, // 150 ms, the example's budget.
    };
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    let readable = poll(&mut fds, Some(&timeout)).expect("poll session fd") > 0;
    assert!(readable, "the emulator's reply should make the fd readable");

    let mut buffer = [0u8; 64];
    let input = session.read_input(&mut buffer).expect("read reply");

    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(input.as_bytes());
    tokens.extend(parser.finish());
    let report = tokens
        .iter()
        .find_map(|token| match token {
            SyntaxToken::Csi(csi) => CursorPositionReport::from_control_sequence(csi),
            _ => None,
        })
        .expect("reply parses as a cursor-position report");
    // --------------------------------------------------------------------------------------------

    // Byte residue (read side): the recipe read the whole reply and the parser holds nothing back.
    assert_eq!(
        input.as_bytes(),
        REPLY,
        "the read should be exactly the reply"
    );
    assert_eq!(report.row(), 12);
    assert_eq!(report.column(), 34);
    assert!(
        parser.pending_bytes().is_empty(),
        "the parser should have no residual pending bytes, got {:?}",
        parser.pending_bytes()
    );

    // Orderly restoration. `leave` replays the mode ledger back to cooked mode.
    session.leave().expect("leave restores the terminal");

    // Byte residue (write side): the emulator saw exactly one probe and no extra bytes. Raw-mode
    // entry and the leave path change device *mode*, not bytes, so the only thing ever written to
    // the wire is the single `CSI 6 n`.
    let probe_seen = probe_rx
        .recv()
        .expect("receive probe result")
        .expect("emulator io");
    let _ = release_tx.send(()); // Let the emulator drop the master and finish.
    emulator.join().expect("join emulator");
    assert_eq!(
        probe_seen, PROBE,
        "the emulator must see exactly one CSI 6 n probe and no residual bytes"
    );

    // Mode residue: after the run the slave termios is byte-for-byte the pre-run cooked state.
    let restored = tcgetattr(&slave).expect("read restored termios");
    assert_eq!(
        termios_without_pending_input(cooked),
        termios_without_pending_input(restored),
        "the recipe must restore the captured cooked termios (no leaked raw mode)"
    );
}

// --- PTY + helper plumbing (mirrors tests/session.rs) --------------------------------------------

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

fn open_read_write(path: impl AsRef<Path>) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    let flags = fcntl_getfl(file.as_fd())?;
    fcntl_setfl(file.as_fd(), flags | OFlags::NONBLOCK)?;
    Ok(())
}

/// Reads all currently available master bytes, retrying briefly so a probe written just after the
/// reader starts is not missed.
fn read_available_after_quiet(master: &mut File) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    for _ in 0..40 {
        let before = out.len();
        let mut buf = [0; 4096];
        loop {
            match master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(err) => return Err(err),
            }
        }
        if !out.is_empty() && out.len() == before {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(out)
}

fn termios_without_pending_input(mut termios: Termios) -> String {
    termios.local_modes -= LocalModes::PENDIN;
    format!("{termios:?}")
}
