//! Small process helpers shared by the PTY-hosted adapters.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

/// How often [`wait_for_path`] polls for the path to appear.
const WAIT_FOR_PATH_POLL: Duration = Duration::from_millis(50);

/// Errors if `tool` is not on `$PATH`.
pub(crate) fn require_tool(tool: &str) -> Result<(), String> {
    which(tool)
        .map(|_| ())
        .ok_or_else(|| format!("{tool} is not installed"))
}

/// Resolves a tool on `$PATH`.
pub(crate) fn which(tool: &str) -> Option<PathBuf> {
    Command::new("sh")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .filter(|p| !p.as_os_str().is_empty())
}

/// Returns the trimmed stdout of a version command, or empty on failure.
pub(crate) fn tool_version(argv: &[&str]) -> String {
    Command::new(argv[0])
        .args(&argv[1..])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Single-quotes a string for the shell, escaping embedded single quotes.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Runs a command and errors on non-zero exit, surfacing stderr.
pub(crate) fn run_ok(cmd: &mut Command) -> Result<(), String> {
    let output = cmd.output().map_err(|e| format!("spawning {cmd:?}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// A per-process scratch directory for one adapter session (FIFOs, launch script, tape).
///
/// # Errors
///
/// Returns an error if the directory cannot be created.
pub(crate) fn session_dir(slug: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("qdb-target-{slug}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    Ok(dir)
}

/// The path of the currently running `qdb` binary, so the in-terminal relay runs exactly the
/// code the adapter was built from (never a stale `$PATH` copy).
pub(crate) fn current_qdb() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("resolving current executable: {e}"))
}

/// Polls until `path` exists or `deadline` passes — used to wait for a socket a spawned server
/// creates (Xvfb's X11 socket, a headless compositor's Wayland socket) before connecting a
/// client to it.
///
/// # Errors
///
/// Returns an error if `path` does not appear within `deadline`.
pub(crate) fn wait_for_path(path: &Path, deadline: Duration) -> Result<(), String> {
    // Live-driver wall-clock deadline (clippy.toml carve-out).
    #[allow(clippy::disallowed_methods)]
    let give_up = std::time::Instant::now() + deadline;
    loop {
        if path.exists() {
            return Ok(());
        }
        #[allow(clippy::disallowed_methods)]
        if std::time::Instant::now() >= give_up {
            return Err(format!(
                "{} did not appear within {deadline:?}",
                path.display()
            ));
        }
        sleep(WAIT_FOR_PATH_POLL);
    }
}

/// Polls until a Wayland socket (`wayland-*`, not its `.lock` companion) appears in
/// `runtime_dir` or `deadline` passes, returning the socket's file name.
///
/// The directory must be private to one compositor: any Wayland socket appearing there is by
/// construction the one we started. Compositors pick their own socket name — none of them read
/// a requested name back from the environment, and where the first probe starts is an
/// implementation detail (wlroots currently starts at `wayland-1`) — so discovering the name a
/// compositor actually bound beats hard-coding one implementation's current choice.
///
/// # Errors
///
/// Returns an error if the directory cannot be read or no socket appears within `deadline`.
pub(crate) fn wait_for_wayland_socket(
    runtime_dir: &Path,
    deadline: Duration,
) -> Result<String, String> {
    // Live-driver wall-clock deadline (clippy.toml carve-out).
    #[allow(clippy::disallowed_methods)]
    let give_up = std::time::Instant::now() + deadline;
    loop {
        let entries = std::fs::read_dir(runtime_dir)
            .map_err(|e| format!("reading {}: {e}", runtime_dir.display()))?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            // A generated socket name, never user input: exact-case ".lock" is what wayland
            // servers actually write, so a case-insensitive check would be the wrong tool here.
            #[allow(clippy::case_sensitive_file_extension_comparisons)]
            let is_lock_file = name.ends_with(".lock");
            if name.starts_with("wayland-") && !is_lock_file {
                return Ok(name.to_string());
            }
        }
        #[allow(clippy::disallowed_methods)]
        if std::time::Instant::now() >= give_up {
            return Err(format!(
                "no wayland-* socket appeared in {} within {deadline:?}",
                runtime_dir.display()
            ));
        }
        sleep(WAIT_FOR_PATH_POLL);
    }
}

/// Writes the relay launch script: `qdb target-relay` plus a completion marker echo that tape
/// drivers (betamax) wait on. A script file keeps quoting out of tmux `send-keys` and betamax
/// `Type`, both of which word-split their argument.
///
/// # Errors
///
/// Returns an error if the script cannot be written.
pub(crate) fn write_relay_script(
    dir: &std::path::Path,
    fifo_in: &std::path::Path,
    fifo_out: &std::path::Path,
) -> Result<PathBuf, String> {
    let bin = current_qdb()?;
    let body = format!(
        "#!/bin/bash\n{bin} target-relay --in {fin} --out {fout}\necho QDB_RELAY_DONE\n",
        bin = shell_quote(&bin.to_string_lossy()),
        fin = shell_quote(&fifo_in.to_string_lossy()),
        fout = shell_quote(&fifo_out.to_string_lossy()),
    );
    let path = dir.join("relay.sh");
    std::fs::write(&path, body).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(path)
}
