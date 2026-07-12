//! The wezterm adapter: a headless `wezterm-mux-server` hosting the byte relay.
//!
//! Headless-ability finding (recorded here, not assumed — B-template step 1): wezterm has a
//! **genuine** headless server, `wezterm-mux-server` — confirmed empirically to create no
//! window at all (unlike kitty's best-available minimized-window fallback). The server's
//! `mux-startup` Lua event is the spawn mechanism: `wezterm.mux.spawn_window { width, height,
//! args }` spawns a pane at the exact requested cell geometry with no runtime resize needed
//! (verified against a real running server: requesting 120x40 landed the pty at exactly `40
//! 120` via `stty size`). Each session gets its own generated `wezterm.lua` (own unix-domain
//! socket path under the session dir) so this adapter never touches the user's real wezterm
//! configuration or a wezterm instance they may already be running.
//!
//! `wezterm cli` is not used at all: the `mux-startup` event both launches the window *and*
//! runs the relay command in one step, so there is no separate spawn-then-attach round trip.
//! The relay mechanic itself is identical to the tmux/kitty adapters: `qdb target-relay` runs
//! as the spawned pane's command, owns the controlling tty, and pumps bytes over a FIFO pair.
//!
//! `resize` is a genuine capability gap: `wezterm cli` only exposes `adjust-pane-size`, a
//! *relative*, directional resize (grow/shrink by N cells in one of four directions) — there is
//! no absolute cols/rows target, so hitting an exact requested size after start is not
//! possible through the CLI. Returning an error here is the Target trait's documented path for
//! a capability an adapter genuinely lacks.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::relay::RelayTransport;
use super::{AdapterKind, StateProbe, StateReading, Target, TargetIdentity, util};

/// How long `start` waits for the relay to come up inside the spawned pane (mux-server startup,
/// the `mux-startup` event, child bash spawn, and relay exec). Matches the other adapters'
/// launch budget.
const LAUNCH_DEADLINE: Duration = Duration::from_secs(30);

/// A headless `wezterm-mux-server` session driven through the in-pane relay.
#[derive(Debug, Default)]
pub struct WeztermTarget {
    child: Option<Child>,
    transport: Option<RelayTransport>,
    session_dir: Option<PathBuf>,
}

impl WeztermTarget {
    /// Creates the adapter (nothing is launched until [`Target::start`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Writes the per-session `wezterm.lua` config: an isolated unix domain (so this session's mux
/// server never shares a socket with the user's real wezterm setup) and a `mux-startup` handler
/// that spawns the relay script at the exact requested geometry.
///
/// `socket` and `script` are embedded as bare Lua string literals without escaping: both come
/// from [`util::session_dir`], our own generated tempdir path (never user input), so they carry
/// no quotes or backslashes needing Lua escaping.
fn write_config(
    dir: &std::path::Path,
    socket: &std::path::Path,
    script: &std::path::Path,
    cols: u16,
    rows: u16,
) -> Result<PathBuf, String> {
    let body = format!(
        r#"local wezterm = require("wezterm")
local config = wezterm.config_builder()
config.unix_domains = {{ {{ name = "qdb", socket_path = "{socket}" }} }}
wezterm.on("mux-startup", function()
    wezterm.mux.spawn_window({{
        width = {cols},
        height = {rows},
        args = {{ "bash", "{script}" }},
    }})
end)
return config
"#,
        socket = socket.display(),
        script = script.display(),
    );
    let path = dir.join("wezterm.lua");
    std::fs::write(&path, body).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(path)
}

impl Target for WeztermTarget {
    fn identity(&self) -> TargetIdentity {
        TargetIdentity {
            name: "wezterm".to_string(),
            // `wezterm --version` prints e.g. "wezterm 20240203-110809-5046fc22" — the fallback
            // identity if the pane answers no XTVERSION (the wire reply is authoritative).
            version_hint: util::tool_version(&["wezterm", "--version"]),
            adapter: AdapterKind::PtyHosted,
            expected_wire_name: Some("wezterm".to_string()),
        }
    }

    fn start(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        util::require_tool("wezterm")?;
        util::require_tool("wezterm-mux-server")?;
        let dir = util::session_dir("wezterm")?;
        let (fifo_in, fifo_out) = RelayTransport::create_fifos(&dir)?;
        let script = util::write_relay_script(&dir, &fifo_in, &fifo_out)?;
        let socket = dir.join("mux.sock");
        let config = write_config(&dir, &socket, &script, cols, rows)?;

        let child = Command::new("wezterm-mux-server")
            .args(["--config-file", &config.to_string_lossy()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawning wezterm-mux-server: {e}"))?;
        self.child = Some(child);
        self.session_dir = Some(dir);

        match RelayTransport::connect(&fifo_in, &fifo_out, LAUNCH_DEADLINE) {
            Ok(transport) => {
                self.transport = Some(transport);
                Ok(())
            }
            Err(e) => {
                // Best-effort cleanup: the relay never came up, so nothing owns ending the
                // server gracefully. We hold the child handle directly, so kill it now rather
                // than leaking it (mirrors the kitty adapter's failed-start cleanup).
                if let Some(mut child) = self.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                Err(e)
            }
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.transport
            .as_mut()
            .ok_or_else(|| "wezterm target not started".to_string())?
            .feed(bytes)
    }

    fn drain_output(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>, String> {
        self.transport
            .as_mut()
            .ok_or_else(|| "wezterm target not started".to_string())?
            .drain(deadline)
    }

    fn read_state(&mut self, _probe: StateProbe) -> Result<Option<StateReading>, String> {
        // wezterm could answer CellText/ScreenHash via `wezterm cli get-text`; that grows
        // additively when a consumer needs it. "Can't answer" is always legal.
        Ok(None)
    }

    fn resize(&mut self, _cols: u16, _rows: u16) -> Result<(), String> {
        // Genuine capability gap (module docs): `wezterm cli` only exposes a relative,
        // directional `adjust-pane-size`, not an absolute cols/rows target.
        Err("wezterm: no absolute resize; only relative adjust-pane-size is available".to_string())
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
        if let Some(dir) = self.session_dir.take() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        Ok(())
    }
}
