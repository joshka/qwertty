//! The tmux adapter: a detached tmux session hosting the byte relay.
//!
//! tmux is the PTY-hosted reference target: `start` creates a detached session at the requested
//! geometry and `send-keys` launches `qdb target-relay` in its pane; from there every byte moves
//! over the relay FIFOs. tmux interprets what the relay writes to the pane tty exactly as it
//! would an application's output, and its query replies arrive back on that tty — no
//! `capture-pane` scraping in the reply path.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::relay::RelayTransport;
use super::{AdapterKind, StateProbe, StateReading, Target, TargetIdentity, util};

/// How long `start` waits for the relay to come up inside the pane (shell start + exec).
const LAUNCH_DEADLINE: Duration = Duration::from_secs(30);

/// A detached tmux session driven through the in-pane relay.
#[derive(Debug, Default)]
pub struct TmuxTarget {
    session: String,
    transport: Option<RelayTransport>,
    session_dir: Option<PathBuf>,
}

impl TmuxTarget {
    /// Creates the adapter (nothing is launched until [`Target::start`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn tmux(args: &[&str]) -> Result<(), String> {
        util::run_ok(Command::new("tmux").args(args))
    }
}

impl Target for TmuxTarget {
    fn identity(&self) -> TargetIdentity {
        TargetIdentity {
            name: "tmux".to_string(),
            // `tmux -V` prints "tmux 3.7b" — the fallback identity if the pane answers no
            // XTVERSION (the wire reply is authoritative when present).
            version_hint: util::tool_version(&["tmux", "-V"]),
            adapter: AdapterKind::PtyHosted,
            expected_wire_name: Some("tmux".to_string()),
        }
    }

    fn start(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        util::require_tool("tmux")?;
        let dir = util::session_dir("tmux")?;
        let (fifo_in, fifo_out) = RelayTransport::create_fifos(&dir)?;
        let script = util::write_relay_script(&dir, &fifo_in, &fifo_out)?;

        self.session = format!("qdb-target-{}", std::process::id());
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &self.session])
            .output();
        Self::tmux(&[
            "new-session",
            "-d",
            "-s",
            &self.session,
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
        ])?;
        // Run the script by path: send-keys word-splits, so a single quoted path is robust
        // where an inline multi-arg command is not.
        Self::tmux(&[
            "send-keys",
            "-t",
            &self.session,
            &format!("bash {}", util::shell_quote(&script.to_string_lossy())),
            "Enter",
        ])?;

        self.transport = Some(RelayTransport::connect(
            &fifo_in,
            &fifo_out,
            LAUNCH_DEADLINE,
        )?);
        self.session_dir = Some(dir);
        Ok(())
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.transport
            .as_mut()
            .ok_or_else(|| "tmux target not started".to_string())?
            .feed(bytes)
    }

    fn drain_output(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>, String> {
        self.transport
            .as_mut()
            .ok_or_else(|| "tmux target not started".to_string())?
            .drain(deadline)
    }

    fn read_state(&mut self, _probe: StateProbe) -> Result<Option<StateReading>, String> {
        // tmux could answer CellText/ScreenHash via `capture-pane`; that grows additively when
        // a consumer needs it. "Can't answer" is always legal.
        Ok(None)
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        if self.transport.is_none() {
            return Err("tmux target not started".to_string());
        }
        Self::tmux(&[
            "resize-window",
            "-t",
            &self.session,
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
        ])
    }

    fn end(&mut self) -> Result<(), String> {
        if let Some(transport) = self.transport.as_mut() {
            transport.close_input();
        }
        self.transport = None;
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &self.session])
            .output();
        if let Some(dir) = self.session_dir.take() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        Ok(())
    }
}
