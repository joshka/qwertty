//! The Tokio session's internal readiness transport.
//!
//! [`TokioTerminalSession`](super::TokioTerminalSession) does not touch `AsyncFd` or `rustix`
//! directly for its read, write, and reactor-registration paths: it holds an [`FdReadiness`] and
//! drives every byte through this module. The seam is deliberately narrow — it is *transport only*
//! ("await readable and read bytes", "await writable and write bytes") and owns no session state:
//! the decoder, correlator, and pending-event queue stay on the session so cancel-safety keeps
//! falling out of the "state lives on the struct" invariant (design 04).
//!
//! There are two transports, resolved by `cfg`, with the same narrow surface the session body
//! calls (`read`/`write_all`):
//!
//! - **Unix** ([`FdReadiness`]): a pollable descriptor dup registered with the Tokio reactor. This
//!   is the design-04 model — await readiness, then a non-blocking read/write. The Unix-only
//!   lifecycle methods (`reassert_nonblocking`, `flush_input`, `detach`/`reattach`, `dup_fd`,
//!   `restore_flags`) live here because their only callers (suspend/resume, the detached handoff,
//!   the resize stream) are Unix-gated off the Windows build.
//! - **Windows** ([`ConsoleReadiness`]): a worker thread plus a waker event and a channel, aliased
//!   to `FdReadiness` by the session body's `cfg` import so its references resolve on both
//!   platforms. A console handle is not pollable and cannot be registered with a reactor, so the
//!   worker waits on `[console input, waker event]` and only reads after the wait says records are
//!   pending — the cancellable-wait model of ADR 0022 §4. The channel between the worker and the
//!   session owns all in-flight bytes, so the public cancel-safety contract holds unchanged (the
//!   windows-readiness analysis §3–§4 settled that this arrives without one public signature
//!   changing).
//!
//! The type is intentionally not a trait or a generic parameter — design 04 rejects speculative
//! runtime generics; the two transports are cfg siblings, not implementations of a shared trait.

// The Windows transport is FFI-only: the waker event, the cancellable wait, and the handle dups are
// `windows-sys` calls with no safe wrapper. The crate lint is `unsafe_code = "deny"` (not `forbid`)
// so this `#[cfg(windows)]` code can opt in; the Unix transport below carries no `unsafe`, so the
// allow is scoped to Windows and the Unix path stays effectively forbidden. See ADR 0021.
#![cfg_attr(
    windows,
    allow(
        unsafe_code,
        reason = "the Windows readiness worker is FFI-only; the waker event and cancellable wait \
                  are unsafe `windows-sys` calls with no safe wrapper in the dependency tree"
    )
)]

#[cfg(unix)]
use std::io::{self, ErrorKind};
#[cfg(unix)]
use std::os::fd::{BorrowedFd, OwnedFd};

#[cfg(unix)]
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
#[cfg(unix)]
use tokio::io::unix::AsyncFd;

#[cfg(windows)]
pub(super) use self::windows_transport::ConsoleReadiness;
#[cfg(unix)]
use crate::terminal;

/// The Unix readiness transport: an [`AsyncFd`]-registered dup plus its saved `fcntl` flags.
///
/// An `FdReadiness` owns a duplicate of the terminal descriptor registered with the current Tokio
/// reactor. The dup shares the same open file description as the device the session owns, so
/// readiness observed on either applies to both, and every read and write happens on the *exact* fd
/// the reactor registered — which is what keeps readiness correct under edge-triggered polling
/// (kqueue/epoll): doing I/O on a different description would let an edge be missed or fire on the
/// wrong fd.
///
/// Setting the dup non-blocking (required by [`AsyncFd`]) mutates the shared description, so
/// [`original_flags`](Self::original_flags) captures what to put back on teardown; a leaked
/// non-blocking flag would corrupt the parent shell's own reads when the dup came from inherited
/// standard input (FM-L class).
#[cfg(unix)]
#[derive(Debug)]
pub(super) struct FdReadiness {
    /// The dup registered with Tokio readiness. All read/write I/O runs on this fd.
    inner: AsyncFd<OwnedFd>,
    /// The descriptor status flags captured before this transport set the dup non-blocking, put
    /// back on every teardown path so the non-blocking flag never leaks onto the shared
    /// description.
    original_flags: OFlags,
}

#[cfg(unix)]
impl FdReadiness {
    /// Duplicates `fd`, sets the dup non-blocking, and registers it with the current Tokio reactor.
    ///
    /// The dup shares `fd`'s open file description, so readiness is shared with the session's
    /// device. The pre-`O_NONBLOCK` flags are captured for teardown restore. If the reactor
    /// registration fails, the original flags are put back on the dup before the error is returned,
    /// so a failed construction leaves the shared description as it was found.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime ([`AsyncFd::try_new`] needs the current reactor).
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::OpenTerminal`] when the descriptor cannot be duplicated, its
    /// flags cannot be read or set, or Tokio cannot register the dup.
    pub(super) fn new(fd: BorrowedFd<'_>) -> terminal::Result<Self> {
        let dup: OwnedFd = rustix::io::dup(fd)
            .map_err(io::Error::from)
            .map_err(terminal::Error::open_terminal)?;

        let original_flags = fcntl_getfl(&dup)
            .map_err(io::Error::from)
            .map_err(terminal::Error::open_terminal)?;
        fcntl_setfl(&dup, original_flags | OFlags::NONBLOCK)
            .map_err(io::Error::from)
            .map_err(terminal::Error::open_terminal)?;

        let inner = match AsyncFd::try_new(dup) {
            Ok(inner) => inner,
            Err(err) => {
                let (dup, err) = err.into_parts();
                // Put the original flags back on the shared description before giving up.
                _ = fcntl_setfl(&dup, original_flags);
                return Err(terminal::Error::open_terminal(err));
            }
        };

        Ok(Self {
            inner,
            original_flags,
        })
    }

    /// Awaits readable, performs exactly one `read(2)` on the registered dup, and returns the
    /// count.
    ///
    /// Returns `Ok(0)` at end of input; the caller decides how to treat EOF. Awaiting readiness
    /// performs no read, so a future dropped mid-await has read nothing — cancel-safety is
    /// preserved because no byte leaves the OS inside the abandoned future. A `WouldBlock`
    /// classified by `try_io` clears the guard's readiness and retries on the next readable
    /// notification.
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::ReadTerminal`] when awaiting readiness or the underlying read
    /// fails.
    pub(super) async fn read(&mut self, buffer: &mut [u8]) -> terminal::Result<usize> {
        loop {
            let mut guard = self
                .inner
                .readable()
                .await
                .map_err(terminal::Error::read_terminal)?;

            match guard.try_io(|inner| fd_read(inner.get_ref(), buffer)) {
                Ok(Ok(len)) => return Ok(len),
                Ok(Err(err)) => return Err(terminal::Error::read_terminal(err)),
                Err(_would_block) => {}
            }
        }
    }

    /// Awaits writable and writes every byte of `bytes` to the registered dup.
    ///
    /// Loops over partial writes on the non-blocking descriptor, advancing the slice by each
    /// write's count; a `WouldBlock` classified by `try_io` clears the guard's readiness and
    /// retries on the next writable notification. A zero-length write is surfaced as an error
    /// rather than spun on. The I/O runs on the registered dup, so the bytes are the device's
    /// bytes and readiness stays correct under edge-triggered polling.
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::WriteTerminal`] when awaiting writability fails, the device
    /// reports a zero-length write, or the underlying write fails.
    pub(super) async fn write_all(&mut self, mut bytes: &[u8]) -> terminal::Result<()> {
        while !bytes.is_empty() {
            let mut guard = self
                .inner
                .writable()
                .await
                .map_err(terminal::Error::write_terminal)?;

            match guard.try_io(|inner| fd_write(inner.get_ref(), bytes)) {
                Ok(Ok(0)) => {
                    return Err(terminal::Error::write_terminal(io::Error::new(
                        ErrorKind::WriteZero,
                        "failed to write terminal output",
                    )));
                }
                Ok(Ok(len)) => bytes = &bytes[len..],
                Ok(Err(err)) => return Err(terminal::Error::write_terminal(err)),
                Err(_would_block) => {}
            }
        }

        Ok(())
    }

    /// Re-asserts the registered dup's non-blocking flag, preserving every other status flag.
    ///
    /// [`AsyncFd`] requires the registered descriptor to be non-blocking. The transport set it so
    /// at construction, but a shell the process returned from after `SIGCONT`, or a child that
    /// owned the terminal during a detached handoff, may have cleared `O_NONBLOCK` on the
    /// shared open file description; this reads the current flags and sets non-blocking on top
    /// so nothing else the other side set is disturbed.
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::SetTerminalMode`] when the flags cannot be read or set.
    pub(super) fn reassert_nonblocking(&self) -> terminal::Result<()> {
        let fd = self.inner.get_ref();
        let flags = fcntl_getfl(fd)
            .map_err(io::Error::from)
            .map_err(terminal::Error::set_terminal_mode)?;
        fcntl_setfl(fd, flags | OFlags::NONBLOCK)
            .map_err(io::Error::from)
            .map_err(terminal::Error::set_terminal_mode)
    }

    /// Discards the terminal's pending input queue (`tcflush` of the input side) on the registered
    /// dup.
    ///
    /// Drops stale bytes typed at the shell while the process was stopped so they are not delivered
    /// to the application as if typed into it. Only the *input* queue is flushed; queued output is
    /// untouched.
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::SetTerminalMode`] when the flush `ioctl` fails.
    pub(super) fn flush_input(&self) -> terminal::Result<()> {
        rustix::termios::tcflush(self.inner.get_ref(), rustix::termios::QueueSelector::IFlush)
            .map_err(io::Error::from)
            .map_err(terminal::Error::set_terminal_mode)
    }

    /// Restores the descriptor status flags captured at construction, best-effort.
    ///
    /// The registered dup and the session device share one open file description, so restoring the
    /// flags here restores them for both. Called on every teardown path (leave and drop); a
    /// redundant set is harmless, so failures are ignored — teardown has nothing better to do with
    /// one.
    pub(super) fn restore_flags(&self) {
        _ = fcntl_setfl(self.inner.get_ref(), self.original_flags);
    }

    /// Duplicates the registered dup into a fresh owned descriptor on the same open file
    /// description.
    ///
    /// Used to hand the resize stream a private descriptor for its size reads without borrowing the
    /// session: because the new dup shares this transport's open file description (itself a dup of
    /// the device), the size it measures is the session's terminal size.
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::OpenTerminal`] when the descriptor cannot be duplicated.
    pub(super) fn dup_fd(&self) -> terminal::Result<OwnedFd> {
        rustix::io::dup(self.inner.get_ref())
            .map_err(io::Error::from)
            .map_err(terminal::Error::open_terminal)
    }

    /// Releases the reactor registration, returning the raw dup and its saved flags to hold across
    /// a synchronous handoff.
    ///
    /// Dropping the [`AsyncFd`] (via [`AsyncFd::into_inner`]) tears down the edge-triggered
    /// registration entirely, so a later [`reattach`](Self::reattach) over the same fd performs a
    /// fresh readiness assessment with no stale edge carried over from before a child ran. The
    /// saved flags travel with the fd so the reattached transport can still restore the
    /// construction-time flags on the eventual teardown.
    pub(super) fn detach(self) -> (OwnedFd, OFlags) {
        (self.inner.into_inner(), self.original_flags)
    }

    /// Re-registers a fresh reactor registration over an already-owned dup, preserving its saved
    /// flags.
    ///
    /// The sibling of [`detach`](Self::detach): it takes the raw fd and the flags detach handed out
    /// and registers a **fresh** [`AsyncFd`] over the *same* fd. It does **not** change the
    /// descriptor's status flags — a detached handoff restores blocking flags for the child before
    /// detaching and re-asserts non-blocking separately afterward
    /// ([`reassert_nonblocking`](Self::reassert_nonblocking)), so this only rebuilds the
    /// registration. On failure the fd is dropped with the error.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime ([`AsyncFd::try_new`] needs the current reactor).
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::OpenTerminal`] when Tokio cannot register the fd (matching
    /// construction).
    pub(super) fn reattach(fd: OwnedFd, original_flags: OFlags) -> terminal::Result<Self> {
        let inner = AsyncFd::try_new(fd).map_err(|err| {
            let (_fd, err) = err.into_parts();
            terminal::Error::open_terminal(err)
        })?;
        Ok(Self {
            inner,
            original_flags,
        })
    }

    /// Borrows the registered dup for tests that inspect or mutate its status flags directly.
    #[cfg(test)]
    pub(super) fn get_ref(&self) -> &OwnedFd {
        self.inner.get_ref()
    }
}

/// Writes bytes to the readiness-registered descriptor with one `write(2)`, returning the count.
///
/// I/O runs on the fd Tokio registered — the dup that shares the device's open file description —
/// so readiness stays correct under edge-triggered polling and the bytes are still the device's
/// bytes. On the non-blocking descriptor a short write advances the caller's remaining slice, and a
/// `WouldBlock` surfaces as an error so `try_io` clears the readiness guard and the caller retries
/// on the next writable notification.
#[cfg(unix)]
fn fd_write(fd: &OwnedFd, bytes: &[u8]) -> io::Result<usize> {
    rustix::io::write(fd, bytes).map_err(io::Error::from)
}

/// Reads into `buffer` from the readiness-registered descriptor with one `read(2)`.
///
/// Returns `Ok(0)` at end of input. Runs on the registered fd for the same readiness-correctness
/// reason as [`fd_write`].
#[cfg(unix)]
fn fd_read(fd: &OwnedFd, buffer: &mut [u8]) -> io::Result<usize> {
    rustix::io::read(fd, buffer).map_err(io::Error::from)
}

/// The Windows readiness transport: a cancellable-wait worker thread feeding a channel.
#[cfg(windows)]
mod windows_transport {
    use std::collections::VecDeque;
    use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
    use std::ptr;
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use tokio::sync::mpsc;
    use windows_sys::Win32::Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Storage::FileSystem::WriteFile;
    use windows_sys::Win32::System::Console::INPUT_RECORD;
    use windows_sys::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects,
    };

    use crate::terminal::{
        self, ConsoleHandles, ConsoleInputReader, RECORD_BATCH, read_input_records,
    };

    /// How many byte chunks the worker→session channel buffers before the worker's `blocking_send`
    /// parks. Human-paced console input never fills this; teardown drops the receiver so a parked
    /// send unblocks regardless (see [`ConsoleReadiness`]'s teardown contract).
    const CHANNEL_CAPACITY: usize = 256;

    /// The Windows readiness transport: a worker thread, a waker event, and a channel.
    ///
    /// # The cancellable-wait model (ADR 0022 §4)
    ///
    /// A console input handle is not pollable, and `ReadConsoleInputW` blocks. The classic hazard
    /// (FM-A1, tokio's own `io::Stdin` "impossible to cancel" trap) is a worker parked *inside* a
    /// blocking read that runtime shutdown then cannot join. This transport defeats it by never
    /// parking inside the read: the worker waits on `[console input, waker event]` with
    /// `WaitForMultipleObjects` and calls `read_input_records` **only after** the wait reports the
    /// input handle signalled, so the read returns immediately with records already pending. The
    /// worker is therefore always unblockable — it is either in the wait (which the waker frees) or
    /// in a bounded, non-blocking stretch about to re-enter it.
    ///
    /// # Cancel-safety of the session `read`
    ///
    /// The only await in [`read`](Self::read) is the channel `recv`. A `recv` future dropped
    /// mid-await strands nothing: bytes the worker already sent sit in the channel (on-struct
    /// state), and the leftover buffer holds any remainder of a short read — exactly as a dropped
    /// Unix `AsyncFd` read leaves bytes in the OS. State lives on the struct, so the public
    /// cancel-safety contract holds unchanged.
    ///
    /// # Teardown cannot wedge shutdown (RR-2 / FM-A1)
    ///
    /// Drop signals the waker event ([`SetEvent`]) and drops the channel receiver, then joins the
    /// worker. The worker observes the teardown one of two ways with no third option: if it is
    /// parked in `WaitForMultipleObjects`, the waker signal frees it immediately and it breaks; if
    /// it is instead parked in `blocking_send` on a full channel, the dropped receiver makes the
    /// send return an error and it breaks. Either path reaches the loop exit within one bounded
    /// step — a `WaitForMultipleObjects` return or a `blocking_send` return — so the join always
    /// completes and a wedged `ReadConsoleInputW` (which the worker never enters uncancellably) can
    /// never hang shutdown.
    #[derive(Debug)]
    pub(in crate::tokio_session) struct ConsoleReadiness {
        /// The manual-reset waker event. Teardown and [`pause`](Self::pause) [`SetEvent`] it to
        /// free a worker parked in the wait; [`resume`](Self::resume) [`ResetEvent`]s it
        /// before respawn. A duplicate of the same underlying event object
        /// ([`worker_waker`](Self::worker_waker)) is what the worker waits on.
        waker: OwnedHandle,
        /// The worker's console input dup, shared with the spawned worker through an [`Arc`] so
        /// [`resume`](Self::resume) can respawn a fresh worker over the *same* input handle. The
        /// worker holds its own [`Arc`] clone for the duration of its loop; the transport's clone
        /// keeps the handle alive across a [`pause`](Self::pause).
        worker_input: Arc<OwnedHandle>,
        /// The worker's console output dup (for resize measurement), shared with the worker
        /// through an [`Arc`] for the same respawn reason as
        /// [`worker_input`](Self::worker_input).
        worker_output: Arc<OwnedHandle>,
        /// The worker's waker dup — a duplicate of the same event object as
        /// [`waker`](Self::waker) — shared through an [`Arc`] so a respawned worker waits on the
        /// same event the transport signals.
        worker_waker: Arc<OwnedHandle>,
        /// The receiver end of the worker→session byte channel. Dropped on teardown (and on
        /// [`pause`](Self::pause)) so a worker parked in `blocking_send` unblocks;
        /// [`resume`](Self::resume) replaces it with the receiver of a fresh channel. `Option` so
        /// it can be dropped *before* the join inside [`Drop`] and [`pause`](Self::pause).
        receiver: Option<mpsc::Receiver<Vec<u8>>>,
        /// A duplicate of the console output handle, written by [`write_all`](Self::write_all).
        output: OwnedHandle,
        /// Bytes recv'd from the channel but not yet returned to a short caller buffer, drained
        /// before the next `recv`. This is the on-struct state that makes `read` cancel-safe
        /// across a buffer smaller than one channel chunk.
        leftover: VecDeque<u8>,
        /// The worker thread's join handle, joined on teardown and on [`pause`](Self::pause).
        /// `Option` so [`Drop`] and [`pause`](Self::pause) can take it, and so a paused (or
        /// failed-resume) transport has no live worker to double-join.
        worker: Option<JoinHandle<()>>,
    }

    impl ConsoleReadiness {
        /// Duplicates the console handles, spawns the cancellable-wait worker, and wires the
        /// channel.
        ///
        /// Both handles are duplicated so the worker and transport own descriptors independent of
        /// the device's: the worker gets an input dup (for its waited reads) and an output dup (to
        /// measure resize geometry); the transport keeps an output dup (for writes) and the waker
        /// event. A second waker handle — a dup of the same event object — travels to the worker so
        /// a `SetEvent` on the transport's copy frees the worker's wait.
        ///
        /// # Errors
        ///
        /// Returns [`terminal::Error::OpenTerminal`] when a handle or the waker event cannot be
        /// duplicated or created.
        pub(in crate::tokio_session) fn new(handles: ConsoleHandles<'_>) -> terminal::Result<Self> {
            let worker_input = Arc::new(duplicate_handle(handles.input)?);
            let worker_output = Arc::new(duplicate_handle(handles.output)?);
            let write_output = duplicate_handle(handles.output)?;

            let waker = create_manual_reset_event()?;
            let worker_waker = Arc::new(duplicate_handle(waker.as_handle())?);

            let (worker, receiver) = spawn_worker(&worker_input, &worker_output, &worker_waker)?;

            Ok(Self {
                waker,
                worker_input,
                worker_output,
                worker_waker,
                receiver: Some(receiver),
                output: write_output,
                leftover: VecDeque::new(),
                worker: Some(worker),
            })
        }

        /// Pauses the reader worker for a `run_detached` console handoff, keeping the transport.
        ///
        /// While a synchronous child owns the console, the worker must **not** be calling
        /// `ReadConsoleInputW` on the same input, or the child and the worker would steal each
        /// other's keystrokes. This signals the waker to free the worker from its wait, drops the
        /// channel receiver so a worker parked in `blocking_send` also unblocks (the same two-way
        /// unblock as [`Drop`]), and joins the worker — so after `pause` returns there is no live
        /// worker reading the console. The console handles and the waker event survive on the
        /// struct for [`resume`](Self::resume).
        ///
        /// Buffered input is **discarded**: any channel chunks the worker had already sent, and any
        /// [`leftover`](Self::leftover) tail, are dropped. Stale pre-child bytes are surprising
        /// after a child consumed the console, so post-child reads start clean (ADR 0022
        /// §7).
        pub(in crate::tokio_session) fn pause(&mut self) {
            // Free a worker parked in the wait, then close the channel so a worker parked in
            // `blocking_send` unblocks too; either path breaks within one bounded step. Only then
            // join — the worker can never wedge (it never parks uncancellably inside a read).
            set_event(&self.waker);
            drop(self.receiver.take());
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            // Discard any bytes buffered before the child ran so post-child reads start clean.
            self.leftover.clear();
        }

        /// Resumes the reader worker after a `run_detached` handoff, over the same console handles.
        ///
        /// Re-arms the waker (`ResetEvent`, so the fresh worker's wait is not immediately freed by
        /// the pause signal), then respawns the worker over the same input/output/waker handles
        /// with a **fresh** channel, whose receiver replaces the one [`pause`](Self::pause)
        /// dropped. On success the transport again has one live worker feeding the channel.
        ///
        /// On failure — the waker cannot be reset or the reader thread cannot be spawned — the
        /// transport is left with **no** live worker and no receiver (both `None`), so a subsequent
        /// [`Drop`] does not double-join and the failed-resume state is well-defined: the caller's
        /// `run_detached` surfaces the error rather than wedging the session.
        ///
        /// # Errors
        ///
        /// Returns [`terminal::Error::SetTerminalMode`] when the waker cannot be reset, or
        /// [`terminal::Error::OpenTerminal`] when the reader thread cannot be spawned.
        pub(in crate::tokio_session) fn resume(&mut self) -> terminal::Result<()> {
            // Re-arm the waker before respawning so the fresh worker does not observe the pause
            // signal still set and break out of its very first wait.
            reset_event(&self.waker)?;
            let (worker, receiver) =
                spawn_worker(&self.worker_input, &self.worker_output, &self.worker_waker)?;
            self.receiver = Some(receiver);
            self.worker = Some(worker);
            Ok(())
        }

        /// Drains the leftover buffer first; otherwise awaits the next channel chunk and returns as
        /// many of its bytes as fit, stashing the remainder.
        ///
        /// The awaited `recv` is the sole await and the sole cancellation point. `recv` returning
        /// `None` means the worker is gone (a broken console, the EOF-equivalent), reported as
        /// `Ok(0)`.
        ///
        /// # Errors
        ///
        /// Never returns an error today; the `Result` shape matches the Unix transport so the
        /// session body's `read` call site is identical on both platforms.
        pub(in crate::tokio_session) async fn read(
            &mut self,
            buffer: &mut [u8],
        ) -> terminal::Result<usize> {
            if !self.leftover.is_empty() {
                return Ok(take_leftover(&mut self.leftover, buffer));
            }
            let receiver = self.receiver.as_mut().expect(
                "the receiver is present for the whole session lifetime; Drop takes it last",
            );
            match receiver.recv().await {
                Some(chunk) => {
                    self.leftover.extend(chunk);
                    Ok(take_leftover(&mut self.leftover, buffer))
                }
                None => Ok(0),
            }
        }

        /// Writes every byte of `bytes` to the console output dup via `WriteFile`.
        ///
        /// Console writes complete immediately, so there is no meaningful await; the method stays
        /// `async` to match the Unix transport's `write_all` at the session call site. A short
        /// write advances the slice; a zero-progress write is surfaced as an error.
        ///
        /// # Errors
        ///
        /// Returns [`terminal::Error::WriteTerminal`] when a write fails or makes no progress.
        #[expect(
            clippy::unused_async,
            reason = "console WriteFile completes synchronously, but the async shape matches the \
                      Unix transport so the session's `write_all` call site is identical"
        )]
        pub(in crate::tokio_session) async fn write_all(
            &mut self,
            bytes: &[u8],
        ) -> terminal::Result<()> {
            write_console(&self.output, bytes)
        }
    }

    impl Drop for ConsoleReadiness {
        fn drop(&mut self) {
            // Free a worker parked in the wait, then close the channel so a worker parked in
            // `blocking_send` also unblocks; either way it breaks within one bounded step. Only
            // then join — the worker cannot wedge shutdown (see the type's teardown contract).
            set_event(&self.waker);
            drop(self.receiver.take());
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    /// Spawns a reader worker over `Arc`-shared handles and returns it with the channel receiver.
    ///
    /// Shared by [`ConsoleReadiness::new`] and [`ConsoleReadiness::resume`]: it builds a fresh
    /// channel, clones the three shared handles into the worker thread, and spawns the
    /// [`worker_loop`]. The receiver is returned to the transport; the sender lives in the worker
    /// and is dropped when the loop exits, so the transport's `recv` sees the channel close as EOF.
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::OpenTerminal`] when the reader thread cannot be spawned.
    fn spawn_worker(
        input: &Arc<OwnedHandle>,
        output: &Arc<OwnedHandle>,
        waker: &Arc<OwnedHandle>,
    ) -> terminal::Result<(JoinHandle<()>, mpsc::Receiver<Vec<u8>>)> {
        let (sender, receiver) = mpsc::channel::<Vec<u8>>(CHANNEL_CAPACITY);
        let input = Arc::clone(input);
        let output = Arc::clone(output);
        let waker = Arc::clone(waker);
        let worker = std::thread::Builder::new()
            .name("qwertty-console-reader".to_owned())
            .spawn(move || worker_loop(&input, &output, &waker, &sender))
            .map_err(terminal::Error::open_terminal)?;
        Ok((worker, receiver))
    }

    /// The cancellable-wait worker loop: wait, read only when input is ready, translate, send.
    ///
    /// Waits on `[input, waker]`; a waker signal (or a wait failure) breaks; an input signal reads
    /// a record batch — which returns immediately, records already pending — translates it
    /// through the worker's own [`ConsoleInputReader`], and `blocking_send`s the bytes. A
    /// closed channel (receiver dropped) or a broken console (`Ok(0)` records) breaks the loop,
    /// dropping the sender so the session's `recv` returns `None` (EOF).
    fn worker_loop(
        input: &OwnedHandle,
        output: &OwnedHandle,
        waker: &OwnedHandle,
        sender: &mpsc::Sender<Vec<u8>>,
    ) {
        let mut reader = ConsoleInputReader::new();
        // SAFETY: a zeroed INPUT_RECORD array is a valid initial value; `read_input_records`
        // overwrites the `count`-length prefix it fills, and only that prefix is read afterward.
        let mut records: [INPUT_RECORD; RECORD_BATCH] = unsafe { std::mem::zeroed() };
        let handles = [
            input.as_raw_handle() as HANDLE,
            waker.as_raw_handle() as HANDLE,
        ];

        loop {
            // SAFETY: `handles` is a live 2-element array of borrowed console/event handles; the
            // count matches; `INFINITE` waits until one signals; `false` waits for any, not all.
            let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            if wait != WAIT_OBJECT_0 {
                // WAIT_OBJECT_0 + 1 is the waker (shutdown); WAIT_FAILED/abandoned also break.
                break;
            }

            // A broken console (read error) reads as EOF for the session: break, dropping the
            // sender so the session's `recv` returns `None`.
            let Ok(count) = read_input_records(input, &mut records) else {
                break;
            };
            if count == 0 {
                reader.flush_carry();
                let tail = reader.take_pending();
                if !tail.is_empty() {
                    let _ = sender.blocking_send(tail);
                }
                break; // EOF: drop the sender so the session sees the channel close.
            }

            reader.translate_records(&records[..count], output);
            let bytes = reader.take_pending();
            if !bytes.is_empty() && sender.blocking_send(bytes).is_err() {
                break; // The session dropped the receiver: shut the worker down.
            }
        }
    }

    /// Copies as many leftover bytes as fit into `buffer`, retaining the remainder, returning the
    /// count.
    ///
    /// Pure (no FFI), so the short-read carry is unit-tested cross-platform below.
    fn take_leftover(leftover: &mut VecDeque<u8>, buffer: &mut [u8]) -> usize {
        let count = buffer.len().min(leftover.len());
        for (slot, byte) in buffer.iter_mut().zip(leftover.drain(..count)) {
            *slot = byte;
        }
        count
    }

    /// Creates a manual-reset, initially-unset event for the shutdown waker via `CreateEventW`.
    fn create_manual_reset_event() -> terminal::Result<OwnedHandle> {
        // SAFETY: null security attributes and name are permitted; `1` (manual reset) and `0`
        // (initially unset) are plain flags. The handle is validated before adoption.
        let handle = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if handle.is_null() {
            return Err(terminal::Error::open_terminal(
                std::io::Error::last_os_error(),
            ));
        }
        // SAFETY: CreateEventW returned a non-null event handle; adopting it transfers sole
        // ownership so it is closed exactly once when the OwnedHandle drops.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    /// Duplicates a borrowed handle into an independently-owned one with the same access rights.
    fn duplicate_handle(source: BorrowedHandle<'_>) -> terminal::Result<OwnedHandle> {
        let mut out: HANDLE = ptr::null_mut();
        // SAFETY: the current-process pseudo-handle is always valid; `source` is a live borrowed
        // handle; `out` is a live out-param; DUPLICATE_SAME_ACCESS copies the source's rights and a
        // null options/inherit pair is permitted.
        let ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                source.as_raw_handle() as HANDLE,
                GetCurrentProcess(),
                &raw mut out,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if ok == 0 {
            return Err(terminal::Error::open_terminal(
                std::io::Error::last_os_error(),
            ));
        }
        // SAFETY: DuplicateHandle produced a valid owned handle; adopting it transfers sole
        // ownership so it is closed exactly once when the OwnedHandle drops.
        Ok(unsafe { OwnedHandle::from_raw_handle(out) })
    }

    /// Signals the manual-reset waker event via `SetEvent`, best-effort (teardown has nothing
    /// better to do with a failure).
    fn set_event(event: &OwnedHandle) {
        // SAFETY: `event` is a live owned event handle.
        let _ = unsafe { SetEvent(event.as_raw_handle() as HANDLE) };
    }

    /// Resets the manual-reset waker event via `ResetEvent`, so a respawned worker's first wait is
    /// not immediately freed by an earlier pause signal.
    ///
    /// # Errors
    ///
    /// Returns [`terminal::Error::SetTerminalMode`] when `ResetEvent` fails.
    fn reset_event(event: &OwnedHandle) -> terminal::Result<()> {
        // SAFETY: `event` is a live owned event handle.
        let ok = unsafe { ResetEvent(event.as_raw_handle() as HANDLE) };
        if ok == 0 {
            return Err(terminal::Error::set_terminal_mode(
                std::io::Error::last_os_error(),
            ));
        }
        Ok(())
    }

    /// Writes all of `bytes` to the console output handle via `WriteFile`, looping over partial
    /// writes.
    fn write_console(output: &OwnedHandle, bytes: &[u8]) -> terminal::Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            let chunk = &bytes[offset..];
            let length = u32::try_from(chunk.len()).unwrap_or(u32::MAX);
            let mut written: u32 = 0;
            // SAFETY: `output` is a live owned console handle; `chunk` is readable for `length`
            // bytes; `written` is a live out-param; a null OVERLAPPED is valid for the synchronous
            // console handle.
            let ok = unsafe {
                WriteFile(
                    output.as_raw_handle() as HANDLE,
                    chunk.as_ptr(),
                    length,
                    &raw mut written,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(terminal::Error::write_terminal(
                    std::io::Error::last_os_error(),
                ));
            }
            if written == 0 {
                return Err(terminal::Error::write_terminal(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "WriteFile made no progress on console output",
                )));
            }
            offset += written as usize;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn take_leftover_carries_a_chunk_longer_than_the_buffer_across_two_reads() {
            // A channel chunk larger than the caller buffer: `read` copies what fits and stashes
            // the rest in the leftover buffer, so the next `read` returns it — no byte
            // is lost across a short read, and the leftover is on-struct state a
            // dropped future never strands.
            let mut leftover = VecDeque::from(b"abcdef".to_vec());

            let mut first = [0u8; 4];
            assert_eq!(take_leftover(&mut leftover, &mut first), 4);
            assert_eq!(&first, b"abcd");

            let mut second = [0u8; 4];
            assert_eq!(
                take_leftover(&mut leftover, &mut second),
                2,
                "only the tail remains"
            );
            assert_eq!(&second[..2], b"ef");
            assert!(leftover.is_empty(), "the leftover buffer drains fully");
        }
    }
}
