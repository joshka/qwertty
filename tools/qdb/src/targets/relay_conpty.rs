//! The ConPTY relay child — the Windows sibling of [`super::relay`].
//!
//! Under ConPTY the host cannot make conhost answer a query directly: a query (`ESC[6n`, …) is
//! only processed by conhost's VT engine if *the child* writes it to its own stdout, and conhost
//! delivers the reply to the child's *stdin* (the application-input direction), not back out the
//! host's output pipe. So — exactly like every PTY-hosted target on Unix — a dumb relay must run
//! under the pseudo-console: the host tells it "emit these bytes," it writes them to
//! `STD_OUTPUT_HANDLE` (→ conhost), it reads `STD_INPUT_HANDLE` (← conhost's reply), and it
//! forwards whatever it captured back to the host over a side channel kept separate from the two
//! VT pipes. All policy stays in the runner; this child only pumps bytes.
//!
//! The side channel is a named pipe the host created and named on this process's command line
//! (`--pipe`); this child connects to it as a client. Framing ([`super::conpty_frame`]) carries
//! `EMIT` instructions inbound and `CAPTURED` bytes outbound, plus a one-shot `HELLO` the child
//! sends once it is attached so the host can trust EOF afterward — the analogue of the Unix
//! relay's hello byte.
#![allow(unsafe_code)]

use std::thread::sleep;
use std::time::Duration;

use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};

use super::conpty_frame::{FrameDecoder, KIND_CAPTURED, KIND_EMIT, KIND_HELLO, encode_frame};
use super::conpty_sys::{OwnedHandle, Ready, last_error, peek, read_some, to_wide, write_all};

/// The relay's pump cadence — a light poll, not a busy spin, matching the Unix relay's 5 ms.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Runs the relay child until the host closes the side channel.
///
/// Opens the named side-channel pipe by `pipe_name`, announces itself with a `HELLO` frame, then
/// loops: apply inbound `EMIT` frames by writing their bytes to real stdout (→ conhost), and
/// forward any bytes readable on real stdin (← conhost's replies) as `CAPTURED` frames. A closed
/// side channel (host hung up) ends the session cleanly.
///
/// # Errors
///
/// Returns an error if the side-channel pipe cannot be opened, the standard handles cannot be
/// resolved, or a side-channel write fails.
pub fn run(pipe_name: &str) -> Result<(), String> {
    let side = open_side_channel(pipe_name)?;
    let stdout = std_handle(STD_OUTPUT_HANDLE)?;
    let stdin = std_handle(STD_INPUT_HANDLE)?;

    // Announce attachment: after this frame the host may trust EOF on the side channel.
    write_all(side.raw(), &encode_frame(KIND_HELLO, &[]))?;

    let mut decoder = FrameDecoder::new();
    let mut buf = [0u8; 4096];
    loop {
        // Inbound: drain EMIT instructions from the host and write them to conhost via stdout.
        // A closed side channel (host hung up) ends the session — the relay's feed-EOF analogue.
        match pump_into_decoder(side.raw(), &mut decoder, &mut buf)? {
            PumpOutcome::Closed => break,
            PumpOutcome::Live => {}
        }
        while let Some((kind, payload)) = decoder.next_frame() {
            if kind == KIND_EMIT {
                // SAFETY-adjacent note: stdout here is conhost's input to its VT engine. Writing
                // the query is what makes conhost process it.
                write_all(stdout, &payload)?;
            }
            // A well-behaved host sends only EMIT frames inbound; anything else is ignored.
        }

        // Outbound: forward whatever conhost delivered to our stdin.
        //
        // TODO(windows-host): confirm reply routing against real conhost / Windows Terminal (RR-6).
        // This is the load-bearing hypothesis of the whole adapter: that conhost delivers a query's
        // reply to the child's stdin (this handle), rather than answering some queries itself on
        // the host output pipe. It is version-dependent and cannot be verified from macOS.
        // If a real host proves a query is instead answered on the host's output pipe, the
        // capture point moves to the host side (see `super::conpty`'s `host_output_read`);
        // the relay shape stays.
        match peek(stdin)? {
            Ready::Closed => break,
            Ready::Bytes(0) => {}
            Ready::Bytes(_) => {
                let got = read_some(stdin, &mut buf)?;
                if got > 0 {
                    write_all(side.raw(), &encode_frame(KIND_CAPTURED, &buf[..got]))?;
                }
            }
        }

        sleep(POLL_INTERVAL);
    }
    Ok(())
}

/// Whether a side-channel pump found the host still connected.
enum PumpOutcome {
    /// The host is still connected (bytes may or may not have been read).
    Live,
    /// The host closed the side channel.
    Closed,
}

/// Reads any bytes currently available on the side channel into `decoder`. Non-blocking: a quiet
/// channel is `Live` with nothing added.
fn pump_into_decoder(
    handle: windows_sys::Win32::Foundation::HANDLE,
    decoder: &mut FrameDecoder,
    buf: &mut [u8],
) -> Result<PumpOutcome, String> {
    match peek(handle)? {
        Ready::Closed => Ok(PumpOutcome::Closed),
        Ready::Bytes(0) => Ok(PumpOutcome::Live),
        Ready::Bytes(_) => {
            let got = read_some(handle, buf)?;
            if got == 0 {
                return Ok(PumpOutcome::Closed);
            }
            decoder.push(&buf[..got]);
            Ok(PumpOutcome::Live)
        }
    }
}

/// Connects to the host's named side-channel pipe as a client.
fn open_side_channel(pipe_name: &str) -> Result<OwnedHandle, String> {
    let wide = to_wide(pipe_name);
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 buffer; the security-attributes and
    // template-file pointers are null, which `CreateFileW` accepts.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        )
    };
    OwnedHandle::new(handle)
        .ok_or_else(|| format!("opening side channel {pipe_name}: error {}", last_error()))
}

/// Resolves one of the standard handles (`STD_OUTPUT_HANDLE` / `STD_INPUT_HANDLE`).
///
/// The returned handle is owned by the console, not by us, so it is returned raw (not wrapped in
/// [`OwnedHandle`]) — closing a standard handle is not the relay's job.
fn std_handle(
    which: windows_sys::Win32::System::Console::STD_HANDLE,
) -> Result<windows_sys::Win32::Foundation::HANDLE, String> {
    // SAFETY: `which` is one of the documented `STD_*_HANDLE` selectors; `GetStdHandle` returns a
    // borrowed handle owned by the console.
    let handle = unsafe { GetStdHandle(which) };
    if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(format!("GetStdHandle failed: error {}", last_error()));
    }
    Ok(handle)
}
