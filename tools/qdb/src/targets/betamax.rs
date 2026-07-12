//! The betamax adapter: a headless ghostty-vt hosting the byte relay via an on-the-fly tape.
//!
//! betamax drives libghostty from a tape script; the tape types one line — the relay launch
//! script — and then waits on the completion marker the script echoes after the relay exits.
//! While the tape waits, the relay pumps bytes over the FIFOs exactly like every other
//! PTY-hosted target. Known quirks (paid for in M7-S2): the tape shell starts in `$HOME`;
//! `Wait+Screen`, never `Wait`; children DO have a controlling terminal.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::relay::RelayTransport;
use super::{AdapterKind, StateProbe, StateReading, Target, TargetIdentity, util};

/// How long `start` waits for the relay to come up (betamax load + shell + exec).
const LAUNCH_DEADLINE: Duration = Duration::from_secs(60);

/// How long `end` waits for the tape to finish after the relay exits.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(60);

/// A betamax-hosted ghostty-vt driven through the in-tape relay.
#[derive(Debug, Default)]
pub struct BetamaxTarget {
    child: Option<Child>,
    transport: Option<RelayTransport>,
    session_dir: Option<PathBuf>,
}

impl BetamaxTarget {
    /// Creates the adapter (nothing is launched until [`Target::start`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Target for BetamaxTarget {
    fn identity(&self) -> TargetIdentity {
        TargetIdentity {
            name: "betamax".to_string(),
            version_hint: util::tool_version(&["betamax", "--version"]),
            adapter: AdapterKind::PtyHosted,
            // betamax hosts ghostty: the wire XTVERSION says `libghostty`, and that is the
            // identity the cross-check must accept — the emulator naming itself.
            expected_wire_name: Some("ghostty".to_string()),
        }
    }

    fn start(&mut self, _cols: u16, _rows: u16) -> Result<(), String> {
        util::require_tool("betamax")?;
        let dir = util::session_dir("betamax")?;
        let (fifo_in, fifo_out) = RelayTransport::create_fifos(&dir)?;
        let script = util::write_relay_script(&dir, &fifo_in, &fifo_out)?;

        // Geometry note: betamax sizes in pixels (`Set Width/Height`), not cells, so the
        // requested cols/rows are not directly settable; the fixed canvas below matches the
        // M7-S2 harness. Cell geometry is a recorded finding, not an adapter promise.
        let tape = format!(
            "Output {out}\nSet Shell \"bash\"\nSet Width 1000\nSet Height 700\n\
             Type \"bash {script}\"\nEnter\nWait+Screen@120s \"QDB_RELAY_DONE\"\n",
            out = dir.join("session.gif").display(),
            script = script.display(),
        );
        let tape_path = dir.join("session.tape");
        std::fs::write(&tape_path, &tape)
            .map_err(|e| format!("writing tape {}: {e}", tape_path.display()))?;

        let child = Command::new("betamax")
            .arg("run")
            .arg(&tape_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawning betamax: {e}"))?;
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
            .ok_or_else(|| "betamax target not started".to_string())?
            .feed(bytes)
    }

    fn drain_output(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>, String> {
        self.transport
            .as_mut()
            .ok_or_else(|| "betamax target not started".to_string())?
            .drain(deadline)
    }

    fn read_state(&mut self, _probe: StateProbe) -> Result<Option<StateReading>, String> {
        // betamax can dump State JSON (ScreenText/cursor readback); that grows additively when
        // a consumer needs it. "Can't answer" is always legal.
        Ok(None)
    }

    fn resize(&mut self, _cols: u16, _rows: u16) -> Result<(), String> {
        Err("betamax cannot resize a running tape session".to_string())
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
                    "betamax did not exit within {SHUTDOWN_DEADLINE:?}; killed"
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
