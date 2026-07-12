//! The kitty adapter: a minimized, task-hidden kitty OS window hosting the byte relay.
//!
//! Headless-ability finding (recorded here, not assumed — B-template step 1): kitty has **no**
//! true headless/server mode on macOS — confirmed against `kitty --help` (`--start-as` choices
//! are `maximized, fullscreen, minimized, normal`, no `hidden`) and the shipped `kitty.conf`
//! reference (no headless/offscreen option exists). The next tier down works cleanly: launch
//! with `--start-as=minimized` plus `macos_hide_from_tasks=yes` (excludes the OS window from
//! Cmd+Tab and the Dock) and an exact cell geometry via `initial_window_width`/
//! `initial_window_height`'s `c` (cell) suffix — verified empirically to land the pty at the
//! requested size with no runtime resize needed. No window is ever focused or requires a human
//! to be present; this is the "hidden-window mode" tier of the B-template's fallback order, one
//! step down from a true headless target like tmux.
//!
//! The relay mechanic is identical to the tmux adapter: `qdb target-relay` runs inside the
//! window as the child process, owns the controlling tty, and pumps bytes over a FIFO pair. A
//! `--listen-on` remote-control socket is opened at launch (`allow_remote_control=yes`) purely
//! for `resize` — `kitten @ resize-os-window --unit cells` — everything else (feed/drain/start
//! readiness) goes through the same relay FIFOs tmux uses, so kitty's remote-control server
//! coming up late never gates session startup.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::relay::RelayTransport;
use super::{AdapterKind, StateProbe, StateReading, Target, TargetIdentity, util};

/// How long `start` waits for the relay to come up inside the kitty window (OS window creation,
/// child bash spawn, and relay exec). Matches the tmux adapter's launch budget; empirically
/// kitty is ready in well under 3s once warm.
const LAUNCH_DEADLINE: Duration = Duration::from_secs(30);

/// A minimized, task-hidden kitty OS window driven through the in-pane relay.
#[derive(Debug, Default)]
pub struct KittyTarget {
    child: Option<Child>,
    transport: Option<RelayTransport>,
    socket: Option<PathBuf>,
    session_dir: Option<PathBuf>,
}

impl KittyTarget {
    /// Creates the adapter (nothing is launched until [`Target::start`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs a `kitten @ --to <socket>` remote-control command against this session.
    fn kitten_at(&self, args: &[&str]) -> Result<(), String> {
        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| "kitty target not started".to_string())?;
        let to = format!("unix:{}", socket.display());
        let mut full_args = vec!["@", "--to", &to];
        full_args.extend_from_slice(args);
        util::run_ok(Command::new("kitten").args(&full_args))
    }
}

impl Target for KittyTarget {
    fn identity(&self) -> TargetIdentity {
        TargetIdentity {
            name: "kitty".to_string(),
            // `kitty --version` prints "kitty 0.39.1 created by Kovid Goyal" — the fallback
            // identity if the pane answers no XTVERSION (the wire reply is authoritative).
            version_hint: util::tool_version(&["kitty", "--version"]),
            adapter: AdapterKind::PtyHosted,
            expected_wire_name: Some("kitty".to_string()),
        }
    }

    fn start(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        util::require_tool("kitty")?;
        util::require_tool("kitten")?;
        let dir = util::session_dir("kitty")?;
        let (fifo_in, fifo_out) = RelayTransport::create_fifos(&dir)?;
        let script = util::write_relay_script(&dir, &fifo_in, &fifo_out)?;
        let socket = dir.join("ctrl.sock");

        let child = Command::new("kitty")
            .args([
                "--start-as=minimized".to_string(),
                format!("--listen-on=unix:{}", socket.display()),
                "-o".to_string(),
                "allow_remote_control=yes".to_string(),
                // Excludes the OS window from Cmd+Tab and the Dock: the closest macOS gets to
                // a headless window (see the module docs' headless-ability finding).
                "-o".to_string(),
                "macos_hide_from_tasks=yes".to_string(),
                "-o".to_string(),
                "remember_window_size=no".to_string(),
                "-o".to_string(),
                format!("initial_window_width={cols}c"),
                "-o".to_string(),
                format!("initial_window_height={rows}c"),
                "-o".to_string(),
                "resize_in_steps=yes".to_string(),
                "bash".to_string(),
                script.to_string_lossy().into_owned(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawning kitty: {e}"))?;
        self.child = Some(child);
        self.socket = Some(socket);
        self.session_dir = Some(dir);

        match RelayTransport::connect(&fifo_in, &fifo_out, LAUNCH_DEADLINE) {
            Ok(transport) => {
                self.transport = Some(transport);
                Ok(())
            }
            Err(e) => {
                // Best-effort cleanup: the relay never came up, so nothing owns ending the
                // window gracefully. Unlike tmux (which pre-cleans by session name next run),
                // we hold the child handle directly, so kill it now rather than leaking it.
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
            .ok_or_else(|| "kitty target not started".to_string())?
            .feed(bytes)
    }

    fn drain_output(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>, String> {
        self.transport
            .as_mut()
            .ok_or_else(|| "kitty target not started".to_string())?
            .drain(deadline)
    }

    fn read_state(&mut self, _probe: StateProbe) -> Result<Option<StateReading>, String> {
        // kitty could answer CellText/ScreenHash via `kitten @ get-text`; that grows additively
        // when a consumer needs it. "Can't answer" is always legal.
        Ok(None)
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        if self.transport.is_none() {
            return Err("kitty target not started".to_string());
        }
        self.kitten_at(&[
            "resize-os-window",
            "--unit",
            "cells",
            "--width",
            &cols.to_string(),
            "--height",
            &rows.to_string(),
            "--match",
            "all",
        ])
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
        self.socket = None;
        if let Some(dir) = self.session_dir.take() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        Ok(())
    }
}
