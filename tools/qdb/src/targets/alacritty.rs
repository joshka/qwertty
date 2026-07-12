//! The alacritty adapter: a scripted, briefly-visible GUI window hosting the byte relay.
//!
//! Alacritty has no headless/offscreen mode — checked directly (`alacritty --help`): `--daemon`
//! skips the initial window entirely (unusable, since the relay needs a pty to attach to), and
//! window startup mode is Windowed/Maximized/(Simple)Fullscreen only, no hidden mode like kitty's
//! `--start-as=hidden`. The maintainer approved brief windowed captures for exactly this case
//! (`work/phase5/tasks/B-target-template.md`, B2 row). `-e` runs the relay script directly as the
//! pty's foreground command — no shell-typing dance like tmux/betamax need — and the window
//! closes itself the moment the relay exits (no `--hold`), so the on-screen lifetime is one
//! scripted probe pass, confirmed empirically to be well under a second after the feed FIFO
//! closes. Geometry is set via `-o window.dimensions.{columns,lines}` (verified against the
//! `CSI 18 t` text-area-cells report, not assumed). XTVERSION comes back empty — a recorded
//! finding (alacritty does not implement it), not an adapter bug.

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

use super::relay::RelayTransport;
use super::{AdapterKind, StateProbe, StateReading, Target, TargetIdentity, util};

/// How long `start` waits for the relay to come up inside the window (app launch + exec).
const LAUNCH_DEADLINE: Duration = Duration::from_secs(30);
/// How long `end` waits for the window to close itself after the relay exits.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(15);

/// A scripted alacritty window driven through the in-pane relay.
#[derive(Debug, Default)]
pub struct AlacrittyTarget {
    child: Option<Child>,
    transport: Option<RelayTransport>,
    session_dir: Option<PathBuf>,
}

impl AlacrittyTarget {
    /// Creates the adapter (nothing is launched until [`Target::start`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Target for AlacrittyTarget {
    fn identity(&self) -> TargetIdentity {
        TargetIdentity {
            name: "alacritty".to_string(),
            version_hint: util::tool_version(&["alacritty", "--version"]),
            adapter: AdapterKind::PtyHosted,
            // Recorded finding, not an assumption: alacritty answers no XTVERSION at all, so the
            // cross-check is Unverifiable every run, never Verified. Still declared here (rather
            // than `None`) so a future alacritty release that adds XTVERSION gets checked, not
            // silently ignored.
            expected_wire_name: Some("alacritty".to_string()),
        }
    }

    fn start(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        util::require_tool("alacritty")?;
        let dir = util::session_dir("alacritty")?;
        let (fifo_in, fifo_out) = RelayTransport::create_fifos(&dir)?;
        let script = util::write_relay_script(&dir, &fifo_in, &fifo_out)?;

        let child = Command::new("alacritty")
            .args([
                "-o",
                &format!("window.dimensions.columns={cols}"),
                "-o",
                &format!("window.dimensions.lines={rows}"),
                "--title",
                "qdb-target",
                "-e",
                "bash",
            ])
            .arg(&script)
            .spawn()
            .map_err(|e| format!("spawning alacritty: {e}"))?;
        self.child = Some(child);
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
            .ok_or_else(|| "alacritty target not started".to_string())?
            .feed(bytes)
    }

    fn drain_output(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>, String> {
        self.transport
            .as_mut()
            .ok_or_else(|| "alacritty target not started".to_string())?
            .drain(deadline)
    }

    fn read_state(&mut self, _probe: StateProbe) -> Result<Option<StateReading>, String> {
        // No remote-control/state-readback channel is wired for this adapter; "can't answer" is
        // always legal. Could grow via alacritty's `msg` IPC if a consumer needs it.
        Ok(None)
    }

    fn resize(&mut self, _cols: u16, _rows: u16) -> Result<(), String> {
        // No window-manager control is wired here (would need AppleScript/Accessibility-level
        // window resizing, out of scope for a scripted byte-relay adapter) — recorded as a
        // capability gap, same as betamax's tape-session resize.
        Err("alacritty adapter cannot resize a running window".to_string())
    }

    fn end(&mut self) -> Result<(), String> {
        if let Some(transport) = self.transport.as_mut() {
            transport.close_input();
        }
        self.transport = None;
        let mut result = Ok(());
        if let Some(mut child) = self.child.take() {
            if !wait_with_deadline(&mut child, SHUTDOWN_DEADLINE) {
                let _ = child.kill();
                let _ = child.wait();
                result = Err(format!(
                    "alacritty did not exit within {SHUTDOWN_DEADLINE:?}; killed"
                ));
            }
        }
        if let Some(dir) = self.session_dir.take() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        result
    }
}

/// Polls `try_wait` until the child exits or `deadline` passes. Returns whether it exited.
fn wait_with_deadline(child: &mut Child, deadline: Duration) -> bool {
    // Live-driver wall-clock deadline (clippy.toml carve-out).
    #[allow(clippy::disallowed_methods)]
    let give_up = std::time::Instant::now() + deadline;
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return true;
        }
        #[allow(clippy::disallowed_methods)]
        if std::time::Instant::now() >= give_up {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
