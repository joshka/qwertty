//! The foot adapter: foot under a self-managed headless Wayland compositor (sway).
//!
//! foot has no headless mode of its own — it is a Wayland client and needs a compositor to
//! attach to. Each session starts its own `sway` instance with `WLR_BACKENDS=headless` (no GPU
//! or real display required) inside a private per-session `XDG_RUNTIME_DIR`, then discovers the
//! socket name the compositor bound there (compositors auto-pick `wayland-N` and cannot be told
//! a name), so concurrent sessions never collide and this adapter never touches a real desktop
//! session.
//! foot's own command-line takes the relay's command positionally (no `-e` flag, unlike
//! xterm/alacritty): `foot bash <relay-script>` runs the same in-terminal byte relay every
//! PTY-hosted adapter uses; the window and the compositor are torn down together in `end`.
//! Linux-only in practice (sway/wlroots and foot are Linux tools) — CI-driven per
//! `work/phase5/tasks/B-target-template.md`, B4 row.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::relay::RelayTransport;
use super::{AdapterKind, StateProbe, StateReading, Target, TargetIdentity, util};

/// How long `start` waits for the relay to come up inside the window (compositor + foot launch).
const LAUNCH_DEADLINE: Duration = Duration::from_secs(30);
/// How long to wait for the headless compositor's Wayland socket to appear before giving up.
const COMPOSITOR_READY_DEADLINE: Duration = Duration::from_secs(10);

/// A self-managed headless-sway foot session driven through the in-pane relay.
#[derive(Debug, Default)]
pub struct FootTarget {
    compositor: Option<Child>,
    child: Option<Child>,
    transport: Option<RelayTransport>,
    session_dir: Option<PathBuf>,
}

impl FootTarget {
    /// Creates the adapter (nothing is launched until [`Target::start`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Target for FootTarget {
    fn identity(&self) -> TargetIdentity {
        TargetIdentity {
            name: "foot".to_string(),
            // `foot --version` prints e.g. "foot version: 1.16.2" — the fallback identity if the
            // pane answers no XTVERSION (the wire reply is authoritative when present).
            version_hint: util::tool_version(&["foot", "--version"]),
            adapter: AdapterKind::PtyHosted,
            expected_wire_name: Some("foot".to_string()),
        }
    }

    fn start(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        util::require_tool("sway")?;
        util::require_tool("foot")?;
        let dir = util::session_dir("foot")?;
        let (fifo_in, fifo_out) = RelayTransport::create_fifos(&dir)?;
        let script = util::write_relay_script(&dir, &fifo_in, &fifo_out)?;

        // sway does not honor `WAYLAND_DISPLAY` to *name* its own listening socket (that env var
        // is a client-side convention, not something the compositor reads back) — confirmed
        // empirically, it auto-picks the first free `wayland-N`. A private, freshly created
        // `XDG_RUNTIME_DIR` per session makes that collision-free (nothing else can appear in a
        // directory only this session touches); which N the compositor starts at is its own
        // implementation detail, so the socket name is *discovered* after launch rather than
        // hard-coded.
        let runtime_dir = dir.join("runtime");
        std::fs::create_dir_all(&runtime_dir)
            .map_err(|e| format!("creating {}: {e}", runtime_dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("setting permissions on {}: {e}", runtime_dir.display()))?;
        }

        // An empty (`/dev/null`) config defaults to enabling Xwayland, which sway tries to spawn
        // on startup — foot is a native Wayland client with no X11 need, and CI images do not
        // ship the Xwayland binary, so an empty config fails sway's own startup outright.
        // `xwayland disable` skips that unnecessary dependency entirely.
        let sway_config = dir.join("sway.conf");
        std::fs::write(&sway_config, "xwayland disable\n")
            .map_err(|e| format!("writing {}: {e}", sway_config.display()))?;

        // sway's own stderr goes to a file rather than /dev/null: when the headless compositor
        // fails to come up, that log is the only way to see why (missing seatd, an unsupported
        // WLR_BACKENDS value on this sway build, etc.) instead of a bare "socket never appeared".
        let compositor_log = dir.join("sway-stderr.log");
        let compositor_stderr = std::fs::File::create(&compositor_log)
            .map_err(|e| format!("creating {}: {e}", compositor_log.display()))?;
        let compositor = Command::new("sway")
            .env("WLR_BACKENDS", "headless")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .env("WLR_RENDERER", "pixman")
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .args(["-c", &sway_config.to_string_lossy()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(compositor_stderr)
            .spawn()
            .map_err(|e| format!("spawning sway: {e}"))?;
        self.compositor = Some(compositor);

        let wayland_display =
            match util::wait_for_wayland_socket(&runtime_dir, COMPOSITOR_READY_DEADLINE) {
                Ok(name) => name,
                Err(e) => {
                    if let Some(mut compositor) = self.compositor.take() {
                        let _ = compositor.kill();
                        let _ = compositor.wait();
                    }
                    let log = std::fs::read_to_string(&compositor_log).unwrap_or_default();
                    return Err(format!(
                        "headless sway did not come up: {e}\nsway stderr:\n{log}"
                    ));
                }
            };

        // foot's window geometry is set via --window-size-chars at launch; no runtime resize
        // path exists through the CLI (documented capability gap, see `resize` below).
        let child = Command::new("foot")
            .env("WAYLAND_DISPLAY", &wayland_display)
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .args(["--window-size-chars", &format!("{cols}x{rows}"), "bash"])
            .arg(&script)
            .spawn()
            .map_err(|e| format!("spawning foot: {e}"))?;
        self.child = Some(child);
        self.session_dir = Some(dir);

        match RelayTransport::connect(&fifo_in, &fifo_out, LAUNCH_DEADLINE) {
            Ok(transport) => {
                self.transport = Some(transport);
                Ok(())
            }
            Err(e) => {
                // Best-effort cleanup: the relay never came up, so nothing owns ending foot and
                // the compositor gracefully. Kill both directly rather than leaking them.
                if let Some(mut child) = self.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                if let Some(mut compositor) = self.compositor.take() {
                    let _ = compositor.kill();
                    let _ = compositor.wait();
                }
                Err(e)
            }
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.transport
            .as_mut()
            .ok_or_else(|| "foot target not started".to_string())?
            .feed(bytes)
    }

    fn drain_output(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>, String> {
        self.transport
            .as_mut()
            .ok_or_else(|| "foot target not started".to_string())?
            .drain(deadline)
    }

    fn read_state(&mut self, _probe: StateProbe) -> Result<Option<StateReading>, String> {
        // No remote-control/state-readback channel is wired for this adapter; "can't answer" is
        // always legal.
        Ok(None)
    }

    fn resize(&mut self, _cols: u16, _rows: u16) -> Result<(), String> {
        // No window-manager control is wired here — recorded as a capability gap, same as the
        // other GUI-window adapters (alacritty, xterm).
        Err("foot adapter cannot resize a running window".to_string())
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
        if let Some(mut compositor) = self.compositor.take() {
            let _ = compositor.kill();
            let _ = compositor.wait();
        }
        if let Some(dir) = self.session_dir.take() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        Ok(())
    }
}
