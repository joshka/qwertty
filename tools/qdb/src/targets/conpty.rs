//! The ConPTY adapter: qdb hosts a pseudo-console with a relay child inside it.
//!
//! `CreatePseudoConsole` makes qdb the *host* of a pseudo-console: an input pipe (host → child
//! stdin, via conhost) and an output pipe (child stdout → host, after conhost's VT engine renders
//! it). The conformance runner needs to observe how conhost's VT engine *answers a query*, and
//! that reveals an inversion: a query (`ESC[6n`, …) must be emitted by the child to its stdout for
//! conhost to process it, and conhost delivers the reply to the child's stdin — not back out the
//! host output pipe. So, exactly like the Unix PTY targets, a dumb relay child runs under the
//! pseudo-console ([`super::relay_conpty`]) and talks to this host over a **side channel** (a named
//! pipe) kept separate from the two VT pipes: this host says "emit these bytes," the child writes
//! them to conhost and forwards whatever conhost replied.
//!
//! This is a **draft skeleton**. It compiles (including cross-compiled for
//! `x86_64-pc-windows-msvc`) and mirrors the Unix relay's contracts, but it has never run on a real
//! Windows host; every seam that cannot be settled from the dev machine is marked
//! `// TODO(windows-host):`. The load-bearing one is RR-6 — reply routing — bracketed in
//! [`super::relay_conpty`].
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, FALSE, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, CreatePipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    STARTUPINFOEXW, STARTUPINFOW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::conpty_frame::{FrameDecoder, KIND_CAPTURED, KIND_EMIT, KIND_HELLO, encode_frame};
use super::conpty_sys::{OwnedHandle, Ready, last_error, peek, read_some, to_wide, write_all};
use super::{AdapterKind, StateProbe, StateReading, Target, TargetIdentity};

/// How long `start` waits for the relay child to connect to the side channel and say hello.
const LAUNCH_DEADLINE: Duration = Duration::from_secs(30);

/// How long `end` waits for the relay to exit after teardown before terminating it.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(10);

/// The hello-wait retry cadence, matching the Unix relay transport's connect retry.
const CONNECT_RETRY: Duration = Duration::from_millis(50);

/// The drain poll cadence, matching the Unix relay's 5 ms.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The per-direction buffer for the side-channel named pipe.
const PIPE_BUFFER: u32 = 64 * 1024;

/// A ConPTY-hosted target: a pseudo-console with a relay child, driven over a named-pipe side
/// channel. Nothing is launched until [`Target::start`].
#[derive(Debug, Default)]
pub struct ConptyTarget {
    /// The live session, or `None` before `start` / after `end`.
    session: Option<Session>,
}

/// One live ConPTY session's owned resources.
#[derive(Debug)]
struct Session {
    /// The pseudo-console handle (`HPCON`), resized on `resize`, closed on `end`.
    hpcon: HPCON,
    /// Write end of the host input pipe. Unused for feeding queries (feeds go over the side
    /// channel so the child *emits* them); held open so conhost's input side stays alive.
    host_input_write: OwnedHandle,
    /// Read end of the host output pipe: conhost's rendered VT output. Under the RR-6 hypothesis
    /// query replies do *not* arrive here (they go to the child's stdin, captured over the side
    /// channel), so this is held but not drained in this draft.
    ///
    /// TODO(windows-host): on a real host this pipe must be drained (likely on a dedicated thread)
    /// so a full output pipe never back-pressures conhost — and if RR-6 resolves the other way
    /// (conhost answers a query here rather than on the child's stdin), this becomes the capture
    /// point. Cannot be settled from macOS.
    host_output_read: OwnedHandle,
    /// The relay child's process handle (waited/terminated on `end`).
    child_process: OwnedHandle,
    /// The relay child's primary thread handle (closed on `end`).
    child_thread: OwnedHandle,
    /// The side channel to the relay child.
    side: SideChannel,
}

/// The host half of the side channel: a named-pipe server plus a frame reassembler.
#[derive(Debug)]
struct SideChannel {
    /// The named-pipe server handle the relay child connected to.
    server: OwnedHandle,
    /// Reassembles frames arriving from the child (captured replies, and the one-shot hello).
    decoder: FrameDecoder,
}

impl SideChannel {
    /// Sends an `EMIT` instruction: the relay writes `bytes` to conhost via its stdout.
    fn send_emit(&mut self, bytes: &[u8]) -> Result<(), String> {
        write_all(self.server.raw(), &encode_frame(KIND_EMIT, bytes))
    }

    /// Moves whatever is readable on the pipe right now into the decoder, distinguishing a quiet
    /// channel from a closed one — the silence-is-data / dead-relay-is-EOF split.
    fn fill_decoder(&mut self, buf: &mut [u8]) -> Result<Ready, String> {
        match peek(self.server.raw())? {
            Ready::Closed => Ok(Ready::Closed),
            Ready::Bytes(0) => Ok(Ready::Bytes(0)),
            Ready::Bytes(_) => {
                let got = read_some(self.server.raw(), buf)?;
                if got == 0 {
                    return Ok(Ready::Closed);
                }
                self.decoder.push(&buf[..got]);
                Ok(Ready::Bytes(got))
            }
        }
    }

    /// Appends every buffered `CAPTURED` payload to `out` (ignoring stray hello/emit frames).
    fn collect_captured(&mut self, out: &mut Vec<u8>) {
        while let Some((kind, payload)) = self.decoder.next_frame() {
            if kind == KIND_CAPTURED {
                out.extend_from_slice(&payload);
            }
        }
    }

    /// Blocks up to `deadline` for the relay's hello frame — the "child attached, both stdio ends
    /// wired, side channel live" signal, after which EOF on the channel is a real death verdict.
    fn await_hello(&mut self, deadline: Duration) -> Result<(), String> {
        // This adapter owns the wall-clock launch deadline (the clippy.toml carve-out for live
        // drivers), allowed at the call site as the Unix relay transport does.
        #[allow(clippy::disallowed_methods)]
        let give_up = Instant::now() + deadline;
        let mut buf = [0u8; 256];
        loop {
            while let Some((kind, _)) = self.decoder.next_frame() {
                if kind == KIND_HELLO {
                    return Ok(());
                }
            }
            match self.fill_decoder(&mut buf)? {
                Ready::Closed => return Err("conpty relay closed before hello".to_string()),
                Ready::Bytes(0) => {
                    #[allow(clippy::disallowed_methods)]
                    let now = Instant::now();
                    if now >= give_up {
                        return Err(format!(
                            "conpty relay did not send hello within {deadline:?}"
                        ));
                    }
                    sleep(CONNECT_RETRY);
                }
                Ready::Bytes(_) => {}
            }
        }
    }

    /// Drains reply bytes: waits up to `deadline` for the first, then returns everything available;
    /// `None` returns what is there now without waiting. A quiet channel is `Ok(vec![])` — silence
    /// is data; a closed channel with nothing pending is a transport error — a dead relay must not
    /// masquerade as a run of timeouts. Mirrors [`super::relay::RelayTransport::drain`].
    fn drain_captured(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>, String> {
        #[allow(clippy::disallowed_methods)]
        let give_up = deadline.map(|d| Instant::now() + d);
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            self.collect_captured(&mut out);
            match self.fill_decoder(&mut buf)? {
                Ready::Closed => {
                    self.collect_captured(&mut out);
                    if out.is_empty() {
                        return Err("conpty relay closed the side channel".to_string());
                    }
                    return Ok(out);
                }
                Ready::Bytes(0) => {
                    if !out.is_empty() {
                        return Ok(out);
                    }
                    #[allow(clippy::disallowed_methods)]
                    let now = Instant::now();
                    match give_up {
                        Some(g) if now < g => {
                            sleep(POLL_INTERVAL.min(g.saturating_duration_since(now)));
                        }
                        _ => return Ok(out),
                    }
                }
                Ready::Bytes(_) => {
                    // Read more raw bytes; loop to decode them.
                }
            }
        }
    }
}

impl ConptyTarget {
    /// Creates the adapter (nothing is launched until [`Target::start`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Target for ConptyTarget {
    fn identity(&self) -> TargetIdentity {
        TargetIdentity {
            name: "conpty".to_string(),
            // conhost has no out-of-band CLI version to read; the wire XTVERSION reply (if conhost
            // answers one at all) is authoritative when present.
            version_hint: String::new(),
            // ConPTY is a pseudo-console *host*: headless, driven over a pseudo-console — PtyHosted
            // in spirit, so the results schema stays unchanged. (A dedicated `ConptyHosted` kind
            // could be added later if the matrix wants to distinguish it.)
            adapter: AdapterKind::PtyHosted,
            // conhost asserts no XTVERSION identity to cross-check against; the wire reply is
            // authoritative if present, so the adapter claims nothing.
            expected_wire_name: None,
        }
    }

    fn start(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        // Two pipes: host input (host writes the write end; conhost reads the child-side read end)
        // and host output (conhost writes the child-side write end; host reads the read end).
        let (input_read, input_write) = create_pipe()?;
        let (output_read, output_write) = create_pipe()?;

        // Build the pseudo-console from the child-side ends. conhost dups them, so our child-side
        // copies are closed right after.
        let hpcon = create_pseudo_console(coord(cols, rows), input_read.raw(), output_write.raw())?;
        drop(input_read);
        drop(output_write);

        // Create the side-channel server before the child so the child can connect to it by name.
        let pipe_name = unique_pipe_name();
        let server = create_named_pipe_server(&pipe_name)?;

        // Launch the relay child attached to the pseudo-console.
        let (child_process, child_thread) = spawn_relay(hpcon, &pipe_name)?;

        // Wait for the child to connect and announce itself.
        let mut side = SideChannel {
            server,
            decoder: FrameDecoder::new(),
        };
        connect_named_pipe(side.server.raw())?;
        side.await_hello(LAUNCH_DEADLINE)?;

        self.session = Some(Session {
            hpcon,
            host_input_write: input_write,
            host_output_read: output_read,
            child_process,
            child_thread,
            side,
        });
        Ok(())
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.session
            .as_mut()
            .ok_or_else(|| "conpty target not started".to_string())?
            .side
            .send_emit(bytes)
    }

    fn drain_output(&mut self, deadline: Option<Duration>) -> Result<Vec<u8>, String> {
        self.session
            .as_mut()
            .ok_or_else(|| "conpty target not started".to_string())?
            .side
            .drain_captured(deadline)
    }

    fn read_state(&mut self, _probe: StateProbe) -> Result<Option<StateReading>, String> {
        // Nothing beyond echoed/replied bytes for now — "can't answer" is always legal.
        Ok(None)
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| "conpty target not started".to_string())?;
        resize_pseudo_console(session.hpcon, coord(cols, rows))
    }

    fn end(&mut self) -> Result<(), String> {
        let Some(session) = self.session.take() else {
            return Ok(());
        };
        let Session {
            hpcon,
            host_input_write,
            host_output_read,
            child_process,
            child_thread,
            side,
        } = session;

        // 1. Close the side channel so the relay's pump loop sees EOF and exits — the analogue of
        //    dropping the Unix relay's feed FIFO.
        drop(side);
        // 2. Close the pseudo-console; conhost then tears down the client's console.
        close_pseudo_console(hpcon);
        // 3. Give the relay a bounded window to exit on its own, then force it.
        wait_or_terminate(child_process.raw(), SHUTDOWN_DEADLINE);
        // 4. Release the remaining handles.
        drop(host_input_write);
        drop(host_output_read);
        drop(child_process);
        drop(child_thread);
        // TODO(windows-host): confirm the exact teardown ordering against a real host — whether
        // ClosePseudoConsole must precede or follow closing the host pipe ends to avoid a conhost
        // hang, and whether the relay reliably exits on side-channel EOF, are seams that cannot be
        // verified from macOS.
        Ok(())
    }
}

impl Drop for ConptyTarget {
    fn drop(&mut self) {
        if self.session.is_some() {
            let _ = self.end();
        }
    }
}

/// A fresh, unique side-channel pipe name for this process.
fn unique_pipe_name() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(r"\\.\pipe\qdb-conpty-{}-{n}", std::process::id())
}

/// Clamps a `u16` cell dimension into the `i16` a `COORD` field holds.
fn coord(cols: u16, rows: u16) -> COORD {
    COORD {
        X: i16::try_from(cols).unwrap_or(i16::MAX),
        Y: i16::try_from(rows).unwrap_or(i16::MAX),
    }
}

/// Creates an anonymous pipe, returning `(read end, write end)`.
fn create_pipe() -> Result<(OwnedHandle, OwnedHandle), String> {
    let mut read: HANDLE = core::ptr::null_mut();
    let mut write: HANDLE = core::ptr::null_mut();
    // SAFETY: `read`/`write` are valid out-params; null security attributes and a default buffer
    // size (0) are accepted by `CreatePipe`.
    let ok = unsafe { CreatePipe(&raw mut read, &raw mut write, core::ptr::null(), 0) };
    if ok == 0 {
        return Err(format!("CreatePipe failed: error {}", last_error()));
    }
    let read = OwnedHandle::new(read)
        .ok_or_else(|| "CreatePipe returned an invalid read handle".to_string())?;
    let write = OwnedHandle::new(write)
        .ok_or_else(|| "CreatePipe returned an invalid write handle".to_string())?;
    Ok((read, write))
}

/// Creates the pseudo-console from the child-side pipe ends at the given size.
fn create_pseudo_console(
    size: COORD,
    input_read: HANDLE,
    output_write: HANDLE,
) -> Result<HPCON, String> {
    let mut hpcon: HPCON = 0;
    // SAFETY: `input_read`/`output_write` are live child-side pipe ends; `hpcon` is a valid
    // out-param; a `0` flags value is the default.
    let hr = unsafe { CreatePseudoConsole(size, input_read, output_write, 0, &raw mut hpcon) };
    if hr != 0 {
        return Err(format!("CreatePseudoConsole failed: HRESULT {hr:#010x}"));
    }
    Ok(hpcon)
}

/// Resizes the pseudo-console.
fn resize_pseudo_console(hpcon: HPCON, size: COORD) -> Result<(), String> {
    // SAFETY: `hpcon` is a live pseudo-console handle.
    let hr = unsafe { ResizePseudoConsole(hpcon, size) };
    if hr != 0 {
        return Err(format!("ResizePseudoConsole failed: HRESULT {hr:#010x}"));
    }
    Ok(())
}

/// Closes the pseudo-console (signals conhost to tear down the client's console).
fn close_pseudo_console(hpcon: HPCON) {
    // SAFETY: `hpcon` is a live pseudo-console handle, closed exactly once here.
    unsafe { ClosePseudoConsole(hpcon) };
}

/// Creates the single-instance, byte-mode, blocking named-pipe server for the side channel.
fn create_named_pipe_server(name: &str) -> Result<OwnedHandle, String> {
    let wide = to_wide(name);
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 name; null security attributes are accepted;
    // the mode flags request a single-instance duplex byte pipe.
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            PIPE_BUFFER,
            PIPE_BUFFER,
            0,
            core::ptr::null(),
        )
    };
    OwnedHandle::new(handle)
        .ok_or_else(|| format!("CreateNamedPipeW failed: error {}", last_error()))
}

/// Waits for the relay child to connect to the side-channel server.
fn connect_named_pipe(handle: HANDLE) -> Result<(), String> {
    // SAFETY: `handle` is a live server pipe handle; a null overlapped pointer is a synchronous
    // connect.
    let ok = unsafe { ConnectNamedPipe(handle, core::ptr::null_mut()) };
    if ok != 0 {
        return Ok(());
    }
    let err = last_error();
    if err == ERROR_PIPE_CONNECTED {
        // The client connected between server creation and this call — already connected is
        // success.
        return Ok(());
    }
    Err(format!("ConnectNamedPipe failed: error {err}"))
}

/// Launches `qdb conpty-relay --pipe <name>` attached to the pseudo-console, returning the child's
/// `(process, thread)` handles.
fn spawn_relay(hpcon: HPCON, pipe_name: &str) -> Result<(OwnedHandle, OwnedHandle), String> {
    let exe = std::env::current_exe().map_err(|e| format!("resolving current executable: {e}"))?;
    let exe_str = exe.to_string_lossy().into_owned();
    let command = format!("\"{exe_str}\" conpty-relay --pipe {pipe_name}");
    let app_wide = to_wide(&exe_str);
    let mut command_wide = to_wide(&command);

    // Size, then build, the proc-thread attribute list that carries the pseudo-console.
    let mut attr_size: usize = 0;
    // SAFETY: the documented sizing call — a null list with a valid size out-param. It returns
    // FALSE by design (buffer too small), so its result is intentionally ignored.
    unsafe {
        InitializeProcThreadAttributeList(core::ptr::null_mut(), 1, 0, &raw mut attr_size);
    }
    let mut attr_buf = vec![0u8; attr_size];
    let attr_list = attr_buf.as_mut_ptr().cast::<core::ffi::c_void>();
    // SAFETY: `attr_list` points at `attr_size` writable bytes just sized for one attribute.
    let ok = unsafe { InitializeProcThreadAttributeList(attr_list, 1, 0, &raw mut attr_size) };
    if ok == 0 {
        return Err(format!(
            "InitializeProcThreadAttributeList failed: error {}",
            last_error()
        ));
    }

    // The attribute value is the HPCON itself, passed by value as the pointer (the ConPTY
    // contract), sized as one HPCON.
    // SAFETY: `attr_list` is an initialized list with room for one attribute; the value is the live
    // `hpcon`, sized correctly; the previous-value out-params are null.
    let ok = unsafe {
        UpdateProcThreadAttribute(
            attr_list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            hpcon as *const core::ffi::c_void,
            core::mem::size_of::<HPCON>(),
            core::ptr::null_mut(),
            core::ptr::null(),
        )
    };
    if ok == 0 {
        let err = last_error();
        // SAFETY: `attr_list` was initialized above; deleting it once is correct.
        unsafe {
            DeleteProcThreadAttributeList(attr_list);
        }
        return Err(format!("UpdateProcThreadAttribute failed: error {err}"));
    }

    // SAFETY: `STARTUPINFOEXW` is plain-old-data; an all-zero value is a valid initial state.
    let mut startup: STARTUPINFOEXW = unsafe { core::mem::zeroed() };
    startup.StartupInfo.cb =
        u32::try_from(core::mem::size_of::<STARTUPINFOEXW>()).unwrap_or(u32::MAX);
    startup.lpAttributeList = attr_list;
    // SAFETY: `PROCESS_INFORMATION` is plain-old-data; an all-zero value is a valid initial state.
    let mut info: PROCESS_INFORMATION = unsafe { core::mem::zeroed() };

    // SAFETY: `app_wide` is a NUL-terminated exe path; `command_wide` is a NUL-terminated mutable
    // command line; the startup pointer aliases `STARTUPINFOEXW` as its leading `STARTUPINFOW`;
    // `info` is a valid out-param. Handles are not inherited — the pseudo-console attribute wires
    // the child's stdio.
    let created = unsafe {
        CreateProcessW(
            app_wide.as_ptr(),
            command_wide.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            FALSE,
            EXTENDED_STARTUPINFO_PRESENT,
            core::ptr::null(),
            core::ptr::null(),
            (&raw const startup).cast::<STARTUPINFOW>(),
            &raw mut info,
        )
    };
    // SAFETY: `attr_list` was initialized; delete it exactly once now that `CreateProcessW` has
    // consumed it.
    unsafe {
        DeleteProcThreadAttributeList(attr_list);
    }
    if created == 0 {
        return Err(format!("CreateProcessW failed: error {}", last_error()));
    }

    let process = OwnedHandle::new(info.hProcess)
        .ok_or_else(|| "CreateProcessW returned an invalid process handle".to_string())?;
    let thread = OwnedHandle::new(info.hThread)
        .ok_or_else(|| "CreateProcessW returned an invalid thread handle".to_string())?;
    Ok((process, thread))
}

/// Waits up to `deadline` for the process to exit, terminating it if it overstays.
fn wait_or_terminate(process: HANDLE, deadline: Duration) {
    let ms = u32::try_from(deadline.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: `process` is a live process handle.
    let waited = unsafe { WaitForSingleObject(process, ms) };
    if waited != WAIT_OBJECT_0 {
        // SAFETY: `process` is a live process handle; force-exit the overstaying relay.
        unsafe {
            TerminateProcess(process, 1);
        }
    }
}
