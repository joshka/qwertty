//! The xterm adapter: xterm under a self-managed headless Xvfb X server.
//!
//! xterm is a normal X11 client with no headless mode of its own; Xvfb (X virtual framebuffer)
//! provides the headless X server it needs. Each session starts its own Xvfb on a private
//! display number derived from this process's pid, so concurrent sessions never collide and this
//! adapter never touches a real X server the host may already be running. `xterm -e bash
//! <relay-script>` runs the same in-terminal byte relay every PTY-hosted adapter uses; the
//! window and the Xvfb server are torn down together in `end`. Linux-only in practice (Xvfb and
//! xterm are X11/Linux tools) — CI-driven per `work/phase5/tasks/B-target-template.md`, B4 row.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::relay::RelayTransport;
use super::{AdapterKind, StateProbe, StateReading, Target, TargetIdentity, util};

/// How long `start` waits for the relay to come up inside the window (Xvfb + xterm launch).
const LAUNCH_DEADLINE: Duration = Duration::from_secs(30);
/// How long to wait for Xvfb's X11 socket to appear before giving up.
const XVFB_READY_DEADLINE: Duration = Duration::from_secs(10);

/// A self-managed headless-Xvfb xterm session driven through the in-pane relay.
#[derive(Debug, Default)]
pub struct XtermTarget {
    xvfb: Option<Child>,
    child: Option<Child>,
    transport: Option<RelayTransport>,
    session_dir: Option<PathBuf>,
}

impl XtermTarget {
    /// Creates the adapter (nothing is launched until [`Target::start`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Target for XtermTarget {
    fn identity(&self) -> TargetIdentity {
        TargetIdentity {
            name: "xterm".to_string(),
            // `xterm -version` prints e.g. "XTerm(390)" — the fallback identity if the pane
            // answers no XTVERSION (the wire reply is authoritative when present).
            version_hint: util::tool_version(&["xterm", "-version"]),
            adapter: AdapterKind::PtyHosted,
            expected_wire_name: Some("xterm".to_string()),
        }
    }

    fn start(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        util::require_tool("Xvfb")?;
        util::require_tool("xterm")?;
        let dir = util::session_dir("xterm")?;
        let (fifo_in, fifo_out) = RelayTransport::create_fifos(&dir)?;
        let script = util::write_relay_script(&dir, &fifo_in, &fifo_out)?;

        // A private display number derived from our own pid: never collides with another
        // session's Xvfb, and never touches a real X server the host might be running.
        let display_num = std::process::id() % 9000 + 1000;
        let display = format!(":{display_num}");
        let socket = PathBuf::from(format!("/tmp/.X11-unix/X{display_num}"));

        // Xvfb's own stderr goes to a file rather than /dev/null: when it fails to come up, that
        // log is the only way to see why instead of a bare "socket never appeared".
        let xvfb_log = dir.join("xvfb-stderr.log");
        let xvfb_stderr = std::fs::File::create(&xvfb_log)
            .map_err(|e| format!("creating {}: {e}", xvfb_log.display()))?;
        let xvfb = Command::new("Xvfb")
            .arg(&display)
            .args(["-screen", "0", "800x600x24", "-nolisten", "tcp"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(xvfb_stderr)
            .spawn()
            .map_err(|e| format!("spawning Xvfb: {e}"))?;
        self.xvfb = Some(xvfb);

        if let Err(e) = util::wait_for_path(&socket, XVFB_READY_DEADLINE) {
            if let Some(mut xvfb) = self.xvfb.take() {
                let _ = xvfb.kill();
                let _ = xvfb.wait();
            }
            let log = std::fs::read_to_string(&xvfb_log).unwrap_or_default();
            return Err(format!("Xvfb did not come up: {e}\nXvfb stderr:\n{log}"));
        }

        // xterm's -geometry is character cells (COLSxROWS) for a terminal, matching what the
        // runner requests directly — no pixel/cell conversion needed.
        let child = Command::new("xterm")
            .env("DISPLAY", &display)
            .args(["-geometry", &format!("{cols}x{rows}"), "-e", "bash"])
            .arg(&script)
            .spawn()
            .map_err(|e| format!("spawning xterm: {e}"))?;
        self.child = Some(child);
        self.session_dir = Some(dir);

        match RelayTransport::connect(&fifo_in, &fifo_out, LAUNCH_DEADLINE) {
            Ok(transport) => {
                self.transport = Some(transport);
                Ok(())
            }
            Err(e) => {
                // Best-effort cleanup: the relay never came up, so nothing owns ending xterm and
                // Xvfb gracefully. Kill both directly rather than leaking them.
                if let Some(mut child) = self.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                if let Some(mut xvfb) = self.xvfb.take() {
                    let _ = xvfb.kill();
                    let _ = xvfb.wait();
                }
                Err(e)
            }
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.transport
            .as_mut()
            .ok_or_else(|| "xterm target not started".to_string())?
            .feed(bytes)
    }

    fn drain_output(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>, String> {
        self.transport
            .as_mut()
            .ok_or_else(|| "xterm target not started".to_string())?
            .drain(deadline)
    }

    fn read_state(&mut self, _probe: StateProbe) -> Result<Option<StateReading>, String> {
        // No remote-control/state-readback channel is wired for this adapter; "can't answer" is
        // always legal.
        Ok(None)
    }

    fn resize(&mut self, _cols: u16, _rows: u16) -> Result<(), String> {
        // No window-manager control is wired here — recorded as a capability gap, same as the
        // other GUI-window adapters (alacritty, foot).
        Err("xterm adapter cannot resize a running window".to_string())
    }

    fn end(&mut self) -> Result<(), String> {
        if let Some(transport) = self.transport.as_mut() {
            transport.close_input();
        }
        self.transport = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut xvfb) = self.xvfb.take() {
            let _ = xvfb.kill();
            let _ = xvfb.wait();
        }
        if let Some(dir) = self.session_dir.take() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        Ok(())
    }
}
