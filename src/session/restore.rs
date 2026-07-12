//! Panic-safe terminal restoration.
//!
//! [`RestoreHandle`] is the emergency exit path from design 01: a cheap handle, obtainable
//! without exclusive session access, that restores the terminal from a panic hook. The handle
//! holds a precomposed teardown blob in a double-buffered cell so the hook path only writes
//! bytes and restores the captured terminal mode; it never composes or allocates.
//!
//! The concurrent core is platform-neutral. [`DoubleBuffered`] (the teardown-blob cell) and
//! [`RestoreCore`] (the `armed` flag plus the blob, with the arm/disarm-once protocol) are shared
//! by both platforms; only the *restore action* differs. On Unix it writes the blob to a `File`
//! and restores the captured [`Termios`] with `tcsetattr`; on Windows it writes the blob to a
//! console output handle and restores the captured console modes and codepage with
//! `SetConsoleMode`/`SetConsoleOutputCP` — the same cooked-mode restore the device performs. Each
//! platform's [`RestoreHandle`] wraps one [`RestoreCore`], so the disarm-once guarantee and the
//! loom-checked blob swap are written once and reused.

// The Windows restore action is FFI-only: `WriteFile`, `SetConsoleMode`, and `SetConsoleOutputCP`
// are `windows-sys` calls with no safe wrapper. The crate lint is `unsafe_code = "deny"` (not
// `forbid`) so this `#[cfg(windows)]` code can opt in; the neutral core and the Unix action carry
// no `unsafe`, so the allow is scoped to Windows. See ADR 0021.
#![cfg_attr(
    windows,
    allow(
        unsafe_code,
        reason = "the Windows restore action is FFI-only; WriteFile/SetConsoleMode/\
                  SetConsoleOutputCP are unsafe `windows-sys` calls with no safe wrapper"
    )
)]

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::{self, Write};
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, PoisonError};
#[cfg(not(loom))]
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicUsize, Ordering},
};
#[cfg(unix)]
use std::time::Duration;

#[cfg(loom)]
use loom::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicUsize, Ordering},
};
#[cfg(unix)]
use rustix::termios::{OptionalActions, Termios, tcsetattr};
#[cfg(windows)]
use windows_sys::Win32::Foundation::HANDLE;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::WriteFile;
#[cfg(windows)]
use windows_sys::Win32::System::Console::{SetConsoleMode, SetConsoleOutputCP};

/// How many times the emergency write retries a blocked terminal before giving up.
#[cfg(unix)]
const EMERGENCY_WRITE_RETRIES: u32 = 5;
/// How long the emergency write waits between retries on a blocked terminal.
#[cfg(unix)]
const EMERGENCY_WRITE_RETRY_DELAY: Duration = Duration::from_millis(10);

/// A byte buffer readable while a single writer republishes it.
///
/// The writer renders into the inactive buffer and swaps the active index, so a reader always
/// observes a complete published value: either the previous blob or the new one, never a torn
/// mix. This is the one piece of genuinely concurrent shared state in the session design, and
/// its swap protocol is verified under loom (see the `loom_` tests). It is platform-neutral —
/// both the Unix and the Windows [`RestoreHandle`] hold one through [`RestoreCore`].
#[derive(Debug, Default)]
pub(crate) struct DoubleBuffered {
    active: AtomicUsize,
    buffers: [Mutex<Vec<u8>>; 2],
}

impl DoubleBuffered {
    /// Publishes a new value by rendering into the inactive buffer and swapping it active.
    ///
    /// Only one publisher may exist; sessions guarantee this through exclusive access.
    pub(crate) fn publish(&self, bytes: &[u8]) {
        let inactive = 1 - self.active.load(Ordering::Acquire);
        {
            let mut buffer = lock(&self.buffers[inactive]);
            buffer.clear();
            buffer.extend_from_slice(bytes);
        }
        self.active.store(inactive, Ordering::Release);
    }

    /// Reads the currently published value.
    pub(crate) fn read_current<R>(&self, read: impl FnOnce(&[u8]) -> R) -> R {
        let active = self.active.load(Ordering::Acquire);
        let buffer = lock(&self.buffers[active]);
        read(&buffer)
    }
}

fn lock(buffer: &Mutex<Vec<u8>>) -> MutexGuard<'_, Vec<u8>> {
    buffer.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The platform-neutral heart of a [`RestoreHandle`]: the armed flag and the teardown blob.
///
/// Both platforms' restore handles wrap one of these. It owns the whole concurrency contract —
/// the loom-checked [`DoubleBuffered`] blob swap and the disarm-once `armed` protocol — so the two
/// platform handles differ only in *what* their restore action does with the blob and the captured
/// mode, never in *when* it runs. The `armed` flag is a plain `std` [`AtomicBool`]: it is set on
/// [`arm`](Self::arm) and claimed exactly once by [`disarm`](Self::disarm)'s swap, so whichever of
/// the panic hook, orderly leave, or drop calls it first restores and the rest skip.
#[derive(Debug)]
struct RestoreCore {
    armed: AtomicBool,
    blob: DoubleBuffered,
}

impl RestoreCore {
    /// Creates a disarmed core with an empty teardown blob.
    fn new() -> Self {
        Self {
            armed: AtomicBool::new(false),
            blob: DoubleBuffered::default(),
        }
    }

    /// Publishes the current teardown bytes into the blob's inactive buffer, then swaps.
    fn publish_blob(&self, bytes: &[u8]) {
        self.blob.publish(bytes);
    }

    /// Arms the emergency path after the session (re-)enters terminal state.
    fn arm(&self) {
        self.armed.store(true, std::sync::atomic::Ordering::Release);
    }

    /// Claims restoration, disarming the emergency path.
    ///
    /// Returns `true` when this call was the one that disarmed it (the atomic swap observed the
    /// armed state), so exactly one path restores per armed period. Every later caller — and every
    /// caller while disarmed — sees `false`.
    fn disarm(&self) -> bool {
        self.armed.swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    /// Reads the currently published teardown blob.
    fn read_blob<R>(&self, read: impl FnOnce(&[u8]) -> R) -> R {
        self.blob.read_current(read)
    }
}

/// An emergency terminal-restore handle usable from a panic hook.
///
/// A `RestoreHandle` is obtained from a session and stays valid without borrowing it, so a
/// panic hook installed once can restore the terminal no matter where the session currently
/// lives. Restoration writes the session's precomposed teardown bytes, then restores the
/// terminal mode captured when the session opened. Both steps are best-effort: the emergency
/// path exists to leave the terminal usable, not to report errors.
///
/// Restoration runs at most once per entered period: whichever of the panic hook, orderly
/// leave, or drop runs first wins, and the others skip. Re-entering the session arms the handle
/// again, so one hook installation covers the whole lifetime of a cycling session.
///
/// # Example
///
/// ```no_run
/// use qwertty::TerminalSession;
///
/// fn main() -> qwertty::Result<()> {
///     let mut session = TerminalSession::open()?;
///
///     let restore = session.restore_handle();
///     let previous = std::panic::take_hook();
///     std::panic::set_hook(Box::new(move |info| {
///         _ = restore.restore();
///         previous(info);
///     }));
///
///     // ... run the application ...
///
///     session.leave()
/// }
/// ```
///
/// # Coverage
///
/// A panic hook runs for unwinding panics on any thread. It does not run on `abort`, on fatal
/// signals, or when a panic occurs while already panicking. Those paths still get the
/// underlying device's best-effort drop restoration when the process unwinds far enough, and
/// nothing otherwise.
#[cfg(unix)]
#[derive(Clone, Debug)]
pub struct RestoreHandle {
    shared: Arc<RestoreShared>,
}

#[cfg(unix)]
#[derive(Debug)]
struct RestoreShared {
    core: RestoreCore,
    device: File,
    cooked: Termios,
}

#[cfg(unix)]
impl RestoreHandle {
    pub(crate) fn new(device: File, cooked: Termios) -> Self {
        Self {
            shared: Arc::new(RestoreShared {
                core: RestoreCore::new(),
                device,
                cooked,
            }),
        }
    }

    /// Publishes the current teardown bytes for the emergency path.
    pub(crate) fn publish_blob(&self, bytes: &[u8]) {
        self.shared.core.publish_blob(bytes);
    }

    /// Arms the emergency path after the session (re-)enters terminal state.
    pub(crate) fn arm(&self) {
        self.shared.core.arm();
    }

    /// Claims restoration, disarming the emergency path.
    ///
    /// Returns `true` when this call was the one that disarmed it, so exactly one path restores
    /// per armed period.
    pub(crate) fn disarm(&self) -> bool {
        self.shared.core.disarm()
    }

    /// Restores the terminal, best-effort.
    ///
    /// Writes the precomposed teardown bytes with a bounded retry so a stalled terminal cannot
    /// hang the hook, then restores the captured terminal mode. Returns `true` when this call
    /// performed restoration and `false` when the session is not currently in a state that
    /// needs it (never entered, already left, or already restored).
    #[must_use = "the result reports whether this call performed restoration"]
    pub fn restore(&self) -> bool {
        if !self.shared.core.disarm() {
            return false;
        }

        self.shared
            .core
            .read_blob(|blob| write_bounded(&self.shared.device, blob));
        _ = tcsetattr(
            &self.shared.device,
            OptionalActions::Now,
            &self.shared.cooked,
        );
        true
    }
}

/// Writes bytes with a bounded retry so a blocked terminal cannot hang the emergency path.
#[cfg(unix)]
fn write_bounded(mut device: &File, bytes: &[u8]) {
    let mut remaining = bytes;
    let mut retries = 0;
    while !remaining.is_empty() {
        match device.write(remaining) {
            Ok(0) => return,
            Ok(written) => remaining = &remaining[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if retries == EMERGENCY_WRITE_RETRIES {
                    return;
                }
                retries += 1;
                std::thread::sleep(EMERGENCY_WRITE_RETRY_DELAY);
            }
            Err(_) => return,
        }
    }
    _ = device.flush();
}

/// The captured console modes and output codepage a Windows [`RestoreHandle`] restores.
///
/// These are the values live at [`open`](crate::Terminal::open) — the same ones the device's
/// cooked-mode restore puts back. The restore targets the *captured originals*, never synthesized
/// defaults, so a session that entered raw mode never leaves the console on VT input, mouse
/// records, or codepage 65001 (the FM-W4 restore discipline). Kept as a plain value type so which
/// values the restore chooses is a pure function, unit-tested on every platform.
#[cfg(any(windows, all(test, not(loom))))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConsoleModeRestore {
    /// The console input mode captured at open (`SetConsoleMode` target for the input handle).
    input_mode: u32,
    /// The console output mode captured at open (`SetConsoleMode` target for the output handle).
    output_mode: u32,
    /// The console output codepage captured at open (`SetConsoleOutputCP` target).
    output_codepage: u32,
}

#[cfg(any(windows, all(test, not(loom))))]
impl ConsoleModeRestore {
    /// Records the captured console modes and codepage the restore action will put back.
    pub(crate) fn new(input_mode: u32, output_mode: u32, output_codepage: u32) -> Self {
        Self {
            input_mode,
            output_mode,
            output_codepage,
        }
    }

    /// The input console mode this restore targets.
    fn input_mode(&self) -> u32 {
        self.input_mode
    }

    /// The output console mode this restore targets.
    fn output_mode(&self) -> u32 {
        self.output_mode
    }

    /// The output codepage this restore targets.
    fn output_codepage(&self) -> u32 {
        self.output_codepage
    }
}

/// An emergency terminal-restore handle usable from a panic hook.
///
/// The Windows sibling of the Unix [`RestoreHandle`](struct@RestoreHandle): identical public
/// surface and identical disarm-once guarantee (both wrap one [`RestoreCore`]), differing only in
/// the restore action. Instead of a `tcsetattr`, it writes the precomposed teardown blob to a
/// duplicated console output handle with `WriteFile`, then restores the captured console input
/// mode, output mode, and output codepage — matching the device's cooked-mode restore. See the
/// Unix handle's docs for the hook pattern and coverage; both are re-exported as `RestoreHandle`.
#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct RestoreHandle {
    shared: Arc<RestoreShared>,
}

#[cfg(windows)]
#[derive(Debug)]
struct RestoreShared {
    core: RestoreCore,
    /// A duplicate of the console output handle, written by the restore action via `WriteFile`.
    output: OwnedHandle,
    /// A duplicate of the console input handle, whose mode the restore action resets.
    input: OwnedHandle,
    /// The console modes and codepage captured at open, put back by the restore action.
    restore: ConsoleModeRestore,
}

#[cfg(windows)]
impl RestoreHandle {
    pub(crate) fn new(
        output: OwnedHandle,
        input: OwnedHandle,
        restore: ConsoleModeRestore,
    ) -> Self {
        Self {
            shared: Arc::new(RestoreShared {
                core: RestoreCore::new(),
                output,
                input,
                restore,
            }),
        }
    }

    /// Publishes the current teardown bytes for the emergency path.
    pub(crate) fn publish_blob(&self, bytes: &[u8]) {
        self.shared.core.publish_blob(bytes);
    }

    /// Arms the emergency path after the session (re-)enters terminal state.
    pub(crate) fn arm(&self) {
        self.shared.core.arm();
    }

    /// Claims restoration, disarming the emergency path.
    ///
    /// Returns `true` when this call was the one that disarmed it, so exactly one path restores
    /// per armed period.
    pub(crate) fn disarm(&self) -> bool {
        self.shared.core.disarm()
    }

    /// Restores the console, best-effort.
    ///
    /// Writes the precomposed teardown bytes to the duplicated console output handle, then restores
    /// the captured input mode, output mode, and output codepage. Returns `true` when this call
    /// performed restoration and `false` when the session is not currently in a state that needs it
    /// (never entered, already left, or already restored) — the identical disarm-once contract as
    /// the Unix handle, since both go through the shared [`RestoreCore`].
    #[must_use = "the result reports whether this call performed restoration"]
    pub fn restore(&self) -> bool {
        if !self.shared.core.disarm() {
            return false;
        }

        self.shared
            .core
            .read_blob(|blob| write_bounded(&self.shared.output, blob));
        // Restore the captured console modes and codepage, best-effort and in the device's cooked
        // order (input, output, codepage); every step is attempted even if an earlier one fails.
        set_console_mode(&self.shared.input, self.shared.restore.input_mode());
        set_console_mode(&self.shared.output, self.shared.restore.output_mode());
        set_output_codepage(self.shared.restore.output_codepage());
        true
    }
}

/// Writes bytes to the duplicated console output handle with `WriteFile`, best-effort.
///
/// Loops over partial writes from the unwritten offset; a failed or zero-progress write ends the
/// attempt. A console `WriteFile` completes synchronously and never reports `WouldBlock`, so there
/// is no blocked-terminal retry to bound — the emergency path is already non-hanging.
#[cfg(windows)]
fn write_bounded(output: &OwnedHandle, bytes: &[u8]) {
    let mut offset = 0;
    while offset < bytes.len() {
        let chunk = &bytes[offset..];
        let length = u32::try_from(chunk.len()).unwrap_or(u32::MAX);
        let mut written: u32 = 0;
        // SAFETY: `output` is a live owned console handle; `chunk` is readable for `length` bytes;
        // `written` is a live out-param; a null OVERLAPPED is valid for the synchronous console
        // handle.
        let ok = unsafe {
            WriteFile(
                output.as_raw_handle() as HANDLE,
                chunk.as_ptr(),
                length,
                &raw mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || written == 0 {
            return;
        }
        offset += written as usize;
    }
}

/// Sets a console mode via `SetConsoleMode`, best-effort (the emergency path ignores failures).
#[cfg(windows)]
fn set_console_mode(handle: &OwnedHandle, mode: u32) {
    // SAFETY: `handle` is a live owned console handle; `mode` is a plain value.
    let _ = unsafe { SetConsoleMode(handle.as_raw_handle() as HANDLE, mode) };
}

/// Sets the console output codepage via `SetConsoleOutputCP`, best-effort.
#[cfg(windows)]
fn set_output_codepage(codepage: u32) {
    // SAFETY: SetConsoleOutputCP takes a codepage id and sets process-global console state.
    let _ = unsafe { SetConsoleOutputCP(codepage) };
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    #[test]
    fn read_current_sees_the_latest_published_blob() {
        let blob = DoubleBuffered::default();

        blob.read_current(|bytes| assert!(bytes.is_empty()));

        blob.publish(b"first");
        blob.read_current(|bytes| assert_eq!(bytes, b"first"));

        blob.publish(b"second");
        blob.read_current(|bytes| assert_eq!(bytes, b"second"));
    }

    #[test]
    fn restore_core_disarms_exactly_once() {
        // The disarm-once protocol both platforms' `restore()` rely on: arm, then the first
        // `disarm` wins and every later one loses, until the next `arm` re-opens the window. This
        // is the neutral guarantee that keeps at most one restore per entered period on Unix and
        // Windows alike.
        let core = RestoreCore::new();
        assert!(!core.disarm(), "a fresh core is disarmed");

        core.arm();
        assert!(core.disarm(), "the first disarm after arm wins");
        assert!(!core.disarm(), "a second disarm loses");

        core.arm();
        assert!(core.disarm(), "re-arming re-opens the window");
    }

    #[test]
    fn console_mode_restore_targets_the_captured_originals() {
        // The Windows restore chooses the captured-at-open values, not synthesized defaults, so a
        // raw-mode session never leaks VT/mouse input bits or codepage 65001 (the FM-W4 discipline,
        // matching the device's cooked-mode restore). This asserts the values the restore action
        // will feed to SetConsoleMode/SetConsoleOutputCP are exactly what was captured.
        let restore = ConsoleModeRestore::new(0x00A7, 0x0005, 437);
        assert_eq!(restore.input_mode(), 0x00A7);
        assert_eq!(restore.output_mode(), 0x0005);
        assert_eq!(restore.output_codepage(), 437);
    }
}

#[cfg(all(test, loom))]
mod loom_tests {
    use super::DoubleBuffered;

    /// A reader racing a republishing writer observes a complete blob, never a torn one.
    #[test]
    fn loom_reader_always_observes_a_complete_blob() {
        loom::model(|| {
            let blob = loom::sync::Arc::new(DoubleBuffered::default());
            blob.publish(b"aa");

            let writer_blob = loom::sync::Arc::clone(&blob);
            let writer = loom::thread::spawn(move || {
                writer_blob.publish(b"bbbb");
                writer_blob.publish(b"cccccc");
            });

            let observed = blob.read_current(<[u8]>::to_vec);
            assert!(
                [b"aa".as_slice(), b"bbbb".as_slice(), b"cccccc".as_slice()]
                    .contains(&observed.as_slice()),
                "reader observed a torn blob: {observed:?}"
            );

            writer.join().expect("writer thread");
        });
    }
}
