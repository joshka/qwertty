//! The live-terminal side of the capture harness: `qdb capture-probe`.
//!
//! This runs *inside* the target terminal (a tmux pane, or a betamax-hosted ghostty-vt). It opens
//! the controlling terminal through the qwertty library, writes each query's raw bytes, and drains
//! the reply off the fd with a per-query poll deadline — the `feed`/`drain_output` core of the
//! conformance Target sketch, poll-driven so it never busy-waits. Results are emitted as JSON lines
//! (one [`ProbeReport`]) to `--out`, which the orchestrator reads back.
//!
//! Only compiled on Unix, where qwertty opens a real terminal device. The `capture` orchestrator
//! and the pure minting core (`capture.rs`) are platform-independent.

#![cfg(unix)]

use std::io::Write as _;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use qwertty::{Terminal, TerminalDevice as _};
use rustix::io::ioctl_fionread;

use crate::capture::{Identity, ProbeLine, ProbePlan, ProbeReport, ProbeStatus};
use crate::escape;
use crate::model::Database;

/// How long to wait for any bytes of a reply before declaring a query silent. Terminals that
/// answer do so in single-digit milliseconds; a generous deadline still keeps a 30-query run fast
/// while surviving a loaded CI host.
const QUERY_DEADLINE: Duration = Duration::from_millis(400);

/// After the first reply byte, keep draining until this much quiet passes, so a multi-byte reply
/// (DA1, OSC color) is captured whole rather than truncated at the first read.
const SETTLE_QUIET: Duration = Duration::from_millis(120);

/// DA1 primary device attributes query — sent for identity regardless of db entries.
const DA1: &[u8] = b"\x1b[c";
/// XTVERSION query (`CSI > q`) — the terminal-name/version string, for identity.
const XTVERSION: &[u8] = b"\x1b[>q";

/// Runs the probe against the given entry ids (or all safe query entries when `only` is empty),
/// writing the JSON report to `out_path`. `target` and `version_hint` come from the orchestrator
/// (e.g. `tmux -V` output); `timestamp` is stamped by the orchestrator for a single run clock.
///
/// # Errors
///
/// Returns an error if the terminal cannot be opened, the plan's bytes cannot be read, or the
/// report cannot be written. Individual query timeouts are recorded as data, not errors.
pub fn run(
    db: &Database,
    repo_root: &Path,
    only: &[String],
    target: &str,
    version_hint: &str,
    timestamp: &str,
    out_path: &Path,
) -> Result<(), String> {
    let mut plan = ProbePlan::build(db, only);
    plan.read_bytes(repo_root, db)?;

    let mut terminal = Terminal::open().map_err(|e| format!("opening terminal: {e}"))?;
    terminal
        .set_raw_mode()
        .map_err(|e| format!("entering raw mode: {e}"))?;

    // Identity first, so a terminal that answers nothing else still gets a DA1/XTVERSION record.
    let da1 = drain_query(&mut terminal, DA1);
    let xtversion = drain_query(&mut terminal, XTVERSION);
    let version = resolve_version(version_hint, &xtversion);

    let mut lines = Vec::new();
    for spec in &plan.specs {
        let reply = drain_query(&mut terminal, &spec.query_bytes);
        let status = if reply.is_empty() {
            ProbeStatus::Timeout
        } else {
            ProbeStatus::Answered
        };
        lines.push(ProbeLine {
            query_id: spec.query_id.clone(),
            reply_id: spec.reply_id.clone(),
            reply_escaped: escape::escape(&reply),
            reply_len: reply.len(),
            status,
        });
    }

    // Best-effort restore; a failure here must not lose the captured data.
    let _ = terminal.set_cooked_mode();

    let report = ProbeReport {
        identity: Identity {
            target: target.to_string(),
            da1_escaped: escape::escape(&da1),
            xtversion_escaped: escape::escape(&xtversion),
            version,
        },
        timestamp: timestamp.to_string(),
        lines,
    };

    let json =
        serde_json::to_string_pretty(&report).map_err(|e| format!("serializing report: {e}"))?;
    let mut file = std::fs::File::create(out_path)
        .map_err(|e| format!("creating {}: {e}", out_path.display()))?;
    writeln!(file, "{json}").map_err(|e| format!("writing {}: {e}", out_path.display()))?;
    Ok(())
}

/// Writes `query` to the terminal, then drains the reply: waits up to [`QUERY_DEADLINE`] for the
/// first byte, then reads until [`SETTLE_QUIET`] passes with no further bytes. Returns the raw
/// reply (empty on timeout). Never propagates I/O errors as failures — a silent terminal is data.
///
/// This is the live-terminal driver the sans-io policy defers real time to, so it reads the
/// monotonic clock directly (allowed at the call site per `clippy.toml`).
fn drain_query(terminal: &mut Terminal, query: &[u8]) -> Vec<u8> {
    if terminal.write_all(query).is_err() || terminal.flush().is_err() {
        return Vec::new();
    }
    let mut reply = Vec::new();
    let mut buf = [0u8; 256];

    // Phase 1: wait for the first byte up to the hard deadline.
    if !wait_readable(terminal, QUERY_DEADLINE) {
        return reply; // silent: timed out with no bytes.
    }
    match terminal.read(&mut buf) {
        Ok(n) if n > 0 => reply.extend_from_slice(&buf[..n]),
        _ => return reply,
    }

    // Phase 2: settle — keep reading until a quiet gap, so multi-byte replies arrive whole.
    while wait_readable(terminal, SETTLE_QUIET) {
        match terminal.read(&mut buf) {
            Ok(n) if n > 0 => reply.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    reply
}

/// How often FIONREAD is polled while waiting for reply bytes.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Waits up to `timeout` for at least one byte to be readable on the terminal fd, by polling
/// `FIONREAD` (bytes available) on a short interval.
///
/// `poll(2)`/`select(2)` on a slave pty are unreliable on macOS — they report "not readable" even
/// when a query reply is sitting in the input buffer (confirmed against tmux). `FIONREAD` reads the
/// exact count without blocking and is portable, so the drain never blocks past the deadline and
/// never misses a reply that is actually present. This is the adapter owning the efficient wait the
/// Target sketch assigns it (a light 5 ms poll, not a busy spin).
///
/// This live-terminal driver owns a real wall-clock deadline — the `clippy.toml` carve-out — so it
/// reads the monotonic clock directly.
fn wait_readable(terminal: &Terminal, timeout: Duration) -> bool {
    let Some(borrowed) = terminal.as_fd() else {
        return false;
    };
    #[allow(clippy::disallowed_methods)]
    let deadline = Instant::now() + timeout;
    loop {
        if ioctl_fionread(borrowed).unwrap_or(0) > 0 {
            return true;
        }
        #[allow(clippy::disallowed_methods)]
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let remaining = deadline.saturating_duration_since(now);
        sleep(POLL_INTERVAL.min(remaining));
    }
}

/// Picks the terminal's identity version string. The XTVERSION reply is authoritative — it is the
/// emulator naming *itself* (betamax hosts ghostty, so its XTVERSION is `libghostty`, not the
/// betamax tool version) — so it wins over the orchestrator hint (`tmux -V` / `betamax --version`),
/// which is only a fallback when the terminal answers no XTVERSION.
fn resolve_version(hint: &str, xtversion: &[u8]) -> String {
    // XTVERSION reply: ESC P > | <text> ESC \  — extract the text between `>|` and `ESC`.
    if let Some(start) = find_subslice(xtversion, b">|") {
        let after = &xtversion[start + 2..];
        let end = after.iter().position(|&b| b == 0x1b).unwrap_or(after.len());
        let name = String::from_utf8_lossy(&after[..end]).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    hint.trim().to_string()
}

/// Finds the first index of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_falls_back_to_hint_without_xtversion() {
        assert_eq!(resolve_version("tmux 3.7b", b""), "tmux 3.7b");
    }

    #[test]
    fn version_prefers_xtversion_over_hint() {
        // betamax hosts ghostty: its XTVERSION says `libghostty`, which must win over the
        // `betamax --version` hint so the origin header names the emulator, not the harness.
        let reply = b"\x1bP>|libghostty\x1b\\";
        assert_eq!(resolve_version("betamax 0.1.15", reply), "libghostty");
    }

    #[test]
    fn version_empty_when_nothing() {
        assert_eq!(resolve_version("", b""), "");
        assert_eq!(resolve_version("  ", b"garbage"), "");
    }
}
