//! The in-terminal byte relay and its adapter-side FIFO transport.
//!
//! Every PTY-hosted target shares one mechanic: something must run *inside* the target terminal
//! with the controlling tty, because that tty is the only place query replies arrive. The relay
//! (`qdb target-relay`) is that something, kept deliberately dumb so all policy stays in the
//! runner: it puts the tty in raw mode and pumps bytes both ways between the tty and a pair of
//! FIFOs — feed FIFO to tty (what an application would write), tty to output FIFO (replies).
//! Closing the feed FIFO ends the session; the relay restores cooked mode and exits.
//!
//! The tty side waits with `FIONREAD` polling: `poll(2)`/`select(2)` on a slave pty are
//! unreliable on macOS — they report "not readable" even when a reply is sitting in the input
//! buffer (confirmed against tmux) — so the relay keeps the capture harness's proven wait. The
//! adapter side ([`RelayTransport`]) reads the output FIFO with a nonblocking read loop, which
//! also distinguishes "quiet" (`EWOULDBLOCK`) from "relay died" (`EOF`), so a dead relay is a
//! transport error, never a run of fabricated `timeout` results.

use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use qwertty::{Terminal, TerminalDevice as _};
use rustix::io::ioctl_fionread;

/// How often the relay's pump loop and the transport's drain loop poll for bytes. A light 5 ms
/// cadence, not a busy spin — the adapter owning the efficient wait per the Target sketch.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How long the adapter retries opening the feed FIFO for writing while the relay launches
/// inside the target (shell startup, binary exec). `ENXIO` means "no reader yet"; a target that
/// never opens the FIFO within this window is declared failed.
const CONNECT_RETRY: Duration = Duration::from_millis(50);

/// The single byte the relay writes on the output FIFO right after opening it. A FIFO read end
/// cannot distinguish "no writer yet" from "writer gone" (both read as EOF), so the hello is
/// the adapter's definitive "relay is up, tty open, both FIFOs attached" signal — after it,
/// EOF on the output FIFO really means the relay died.
const HELLO: u8 = 0x06; // ACK

/// Runs the relay inside the target terminal until the feed FIFO reaches EOF.
///
/// Opening `fifo_in` for reading blocks until the adapter opens it for writing — that pairing
/// is the startup handshake. After EOF the tty is drained one last time (a reply racing the
/// shutdown still ships) and cooked mode is restored best-effort.
///
/// # Errors
///
/// Returns an error if the terminal or a FIFO cannot be opened. Pump-loop I/O errors end the
/// relay quietly instead — the adapter sees EOF on its side and reports the transport failure.
pub fn run(fifo_in: &Path, fifo_out: &Path) -> Result<(), String> {
    let mut terminal = Terminal::open().map_err(|e| format!("opening terminal: {e}"))?;
    terminal
        .set_raw_mode()
        .map_err(|e| format!("entering raw mode: {e}"))?;

    // Blocking open: completes when the adapter opens the write end (the handshake). Then made
    // nonblocking so the pump loop can interleave both directions without stalling on a quiet
    // feed. A nonblocking *open* would defeat EOF detection: with no writer yet connected,
    // reads return 0 immediately, indistinguishable from the adapter hanging up.
    let mut fin = File::open(fifo_in).map_err(|e| format!("opening {}: {e}", fifo_in.display()))?;
    rustix::fs::fcntl_setfl(&fin, rustix::fs::OFlags::NONBLOCK)
        .map_err(|e| format!("setting {} nonblocking: {e}", fifo_in.display()))?;
    let mut fout = File::options()
        .write(true)
        .open(fifo_out)
        .map_err(|e| format!("opening {}: {e}", fifo_out.display()))?;
    // Hello: everything is attached; the adapter may now trust EOF semantics on this FIFO.
    fout.write_all(&[HELLO])
        .and_then(|()| fout.flush())
        .map_err(|e| format!("writing hello: {e}"))?;

    let mut buf = [0u8; 4096];
    loop {
        // Terminal -> output FIFO: ship every reply byte the tty has.
        if !pump_tty(&mut terminal, &mut fout, &mut buf) {
            break;
        }
        // Feed FIFO -> terminal: write what the runner fed, EOF ends the session.
        match fin.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if terminal.write_all(&buf[..n]).is_err() || terminal.flush().is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
        sleep(POLL_INTERVAL);
    }

    // Final drain: a reply that raced the shutdown still reaches the adapter.
    let _ = pump_tty(&mut terminal, &mut fout, &mut buf);
    let _ = fout.flush();
    let _ = terminal.set_cooked_mode();
    Ok(())
}

/// Copies every byte currently readable on the tty to the output FIFO. Returns `false` when the
/// FIFO write fails (adapter hung up) — the signal to end the relay.
fn pump_tty(terminal: &mut Terminal, fout: &mut File, buf: &mut [u8]) -> bool {
    while tty_bytes_ready(terminal) > 0 {
        match terminal.read(buf) {
            Ok(n) if n > 0 => {
                if fout.write_all(&buf[..n]).is_err() || fout.flush().is_err() {
                    return false;
                }
            }
            _ => break,
        }
    }
    true
}

/// Bytes readable on the terminal fd right now, via `FIONREAD` (the macOS-safe wait).
fn tty_bytes_ready(terminal: &Terminal) -> usize {
    terminal
        .as_fd()
        .map(|fd| ioctl_fionread(fd).unwrap_or(0))
        .map_or(0, |n| usize::try_from(n).unwrap_or(0))
}

/// The adapter-side handle: feeds bytes to the relay and drains what the terminal replied.
#[derive(Debug)]
pub struct RelayTransport {
    /// Write end of the feed FIFO (runner -> relay -> tty). Dropping it is the end-of-session
    /// signal the relay's EOF detection keys on.
    fifo_in: Option<File>,
    /// Read end of the output FIFO (tty -> relay -> runner), opened nonblocking.
    fifo_out: File,
}

impl RelayTransport {
    /// Creates the FIFO pair under `dir`: `(feed path, output path)`.
    ///
    /// # Errors
    ///
    /// Returns an error if `mkfifo` fails for either path.
    pub fn create_fifos(dir: &Path) -> Result<(PathBuf, PathBuf), String> {
        let fifo_in = dir.join("feed.fifo");
        let fifo_out = dir.join("out.fifo");
        for path in [&fifo_in, &fifo_out] {
            let _ = std::fs::remove_file(path);
            let status = std::process::Command::new("mkfifo")
                .arg(path)
                .status()
                .map_err(|e| format!("spawning mkfifo: {e}"))?;
            if !status.success() {
                return Err(format!("mkfifo {} failed", path.display()));
            }
        }
        Ok((fifo_in, fifo_out))
    }

    /// Connects to a relay the caller has just launched inside the target.
    ///
    /// The output FIFO opens for reading immediately (nonblocking); the feed FIFO's write end
    /// is retried on `ENXIO` until the relay opens its read end or `deadline` passes — the
    /// launch handshake, bounded so a target that failed to start is an error, not a hang.
    ///
    /// # Errors
    ///
    /// Returns an error if the relay does not open its end of the FIFOs before `deadline`.
    pub fn connect(fifo_in: &Path, fifo_out: &Path, deadline: Duration) -> Result<Self, String> {
        use rustix::fs::{Mode, OFlags};
        let out_fd = rustix::fs::open(fifo_out, OFlags::RDONLY | OFlags::NONBLOCK, Mode::empty())
            .map_err(|e| format!("opening {}: {e}", fifo_out.display()))?;

        // This transport owns the wall-clock launch deadline — the clippy.toml carve-out for
        // live drivers, allowed at the call site.
        #[allow(clippy::disallowed_methods)]
        let give_up = Instant::now() + deadline;
        let in_fd = loop {
            match rustix::fs::open(fifo_in, OFlags::WRONLY | OFlags::NONBLOCK, Mode::empty()) {
                Ok(fd) => break fd,
                Err(rustix::io::Errno::NXIO) => {
                    #[allow(clippy::disallowed_methods)]
                    let now = Instant::now();
                    if now >= give_up {
                        return Err(format!(
                            "relay did not open {} within {deadline:?} (target failed to start?)",
                            fifo_in.display()
                        ));
                    }
                    sleep(CONNECT_RETRY);
                }
                Err(e) => return Err(format!("opening {}: {e}", fifo_in.display())),
            }
        };
        // Writes block from here on: feeds are tiny (query sequences), and blocking semantics
        // surface a wedged relay as a visible stall rather than a silent partial write.
        rustix::fs::fcntl_setfl(&in_fd, rustix::fs::OFlags::empty())
            .map_err(|e| format!("clearing nonblock on {}: {e}", fifo_in.display()))?;
        let mut transport = Self {
            fifo_in: Some(File::from(in_fd)),
            fifo_out: File::from(out_fd),
        };

        // Wait for the relay's hello byte. Until it arrives, EOF reads on the output FIFO only
        // mean "the relay has not opened its write end yet" — never a death verdict.
        let mut hello = [0u8; 1];
        loop {
            match transport.fifo_out.read(&mut hello) {
                Ok(1) if hello[0] == HELLO => break,
                Ok(1) => {
                    return Err(format!(
                        "unexpected first byte {:#04x} from relay (expected hello)",
                        hello[0]
                    ));
                }
                Ok(_) | Err(_) => {
                    #[allow(clippy::disallowed_methods)]
                    let now = Instant::now();
                    if now >= give_up {
                        return Err(format!(
                            "relay did not come up on {} within {deadline:?}",
                            fifo_out.display()
                        ));
                    }
                    sleep(CONNECT_RETRY);
                }
            }
        }
        Ok(transport)
    }

    /// Sends bytes to the relay, which writes them to the target's tty verbatim.
    ///
    /// # Errors
    ///
    /// Returns an error if the feed FIFO is closed or the write fails (relay died).
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), String> {
        let Some(fifo_in) = self.fifo_in.as_mut() else {
            return Err("feed after close_input".to_string());
        };
        fifo_in
            .write_all(bytes)
            .and_then(|()| fifo_in.flush())
            .map_err(|e| format!("feeding relay: {e}"))
    }

    /// Drains reply bytes: waits up to `deadline` for the first byte, then returns everything
    /// available; `None` returns what is there right now without waiting.
    ///
    /// # Errors
    ///
    /// Returns an error if the relay closed the output FIFO with nothing pending — a dead
    /// relay must surface as a transport failure, never as fabricated `timeout` results.
    pub fn drain(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>, String> {
        // Live-driver wall-clock deadline (clippy.toml carve-out).
        #[allow(clippy::disallowed_methods)]
        let give_up = deadline.map(|d| Instant::now() + d);
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match self.fifo_out.read(&mut buf) {
                Ok(0) => {
                    // EOF: the relay hung up. Pending bytes still count; emptiness is fatal.
                    if out.is_empty() {
                        return Err("relay closed the output stream".to_string());
                    }
                    return Ok(out);
                }
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    // Keep reading immediately — gather the whole batch that has arrived.
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if !out.is_empty() {
                        return Ok(out);
                    }
                    #[allow(clippy::disallowed_methods)]
                    let now = Instant::now();
                    match give_up {
                        Some(give_up) if now < give_up => {
                            sleep(POLL_INTERVAL.min(give_up.saturating_duration_since(now)));
                        }
                        _ => return Ok(out),
                    }
                }
                Err(e) => return Err(format!("draining relay: {e}")),
            }
        }
    }

    /// Closes the feed FIFO — the end-of-session signal. The relay sees EOF, restores the
    /// terminal, and exits.
    pub fn close_input(&mut self) {
        self.fifo_in = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory unique to this test run.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("qdb-relay-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Round-trips feed and drain against a fake relay thread that echoes each fed chunk back
    /// uppercased — proving the handshake order, the batch drain, and EOF-on-close semantics
    /// without a terminal.
    #[test]
    fn transport_round_trips_against_fake_relay() {
        let dir = scratch("roundtrip");
        let (fifo_in, fifo_out) = RelayTransport::create_fifos(&dir).unwrap();

        let relay_in = fifo_in.clone();
        let relay_out = fifo_out.clone();
        let relay = std::thread::spawn(move || {
            // Mirror the real relay's open order: feed end first (blocking), then output end,
            // then the hello byte.
            let mut fin = File::open(&relay_in).unwrap();
            let mut fout = File::options().write(true).open(&relay_out).unwrap();
            fout.write_all(&[HELLO]).unwrap();
            fout.flush().unwrap();
            let mut buf = [0u8; 256];
            loop {
                match fin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let upper: Vec<u8> = buf[..n].iter().map(u8::to_ascii_uppercase).collect();
                        fout.write_all(&upper).unwrap();
                        fout.flush().unwrap();
                    }
                }
            }
        });

        let mut transport =
            RelayTransport::connect(&fifo_in, &fifo_out, Duration::from_secs(5)).unwrap();
        transport.feed(b"da1 query").unwrap();
        let reply = transport.drain(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(reply, b"DA1 QUERY");

        // A quiet relay drains empty without error — silence is data.
        let quiet = transport.drain(Some(Duration::from_millis(30))).unwrap();
        assert!(quiet.is_empty());
        let now = transport.drain(None).unwrap();
        assert!(now.is_empty());

        // Closing the feed ends the fake relay; a drain then reports the hangup as an error.
        transport.close_input();
        relay.join().unwrap();
        assert!(transport.drain(Some(Duration::from_secs(5))).is_err());
    }

    #[test]
    fn connect_times_out_when_no_relay_launches() {
        let dir = scratch("timeout");
        let (fifo_in, fifo_out) = RelayTransport::create_fifos(&dir).unwrap();
        let err =
            RelayTransport::connect(&fifo_in, &fifo_out, Duration::from_millis(120)).unwrap_err();
        assert!(err.contains("did not open"), "unexpected error: {err}");
    }
}
