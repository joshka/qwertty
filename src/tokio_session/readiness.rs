//! The Tokio session's internal readiness transport.
//!
//! [`TokioTerminalSession`](super::TokioTerminalSession) does not touch `AsyncFd` or `rustix`
//! directly for its read, write, and reactor-registration paths: it holds an [`FdReadiness`] and
//! drives every byte through this module. The seam is deliberately narrow — it is *transport only*
//! ("await readable and read bytes", "await writable and write bytes") and owns no session state:
//! the decoder, correlator, and pending-event queue stay on the session so cancel-safety keeps
//! falling out of the "state lives on the struct" invariant (design 04).
//!
//! This is the Unix transport over a pollable descriptor. The type is intentionally not a trait or
//! a generic parameter — design 04 rejects speculative runtime generics, and the windows-readiness
//! analysis (§3–§4) settles that a later `#[cfg(windows)]` transport (a worker thread plus a waker
//! event and a channel, which never has an fd) arrives as a cfg-gated sibling in this same module
//! without changing one public signature. Everything here is therefore Unix-specific lifecycle
//! detail — the fd dup, the `O_NONBLOCK` bookkeeping, the `fcntl` flag restore — and stays that
//! way.

use std::io::{self, ErrorKind};
use std::os::fd::{BorrowedFd, OwnedFd};

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use tokio::io::unix::AsyncFd;

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
#[derive(Debug)]
pub(super) struct FdReadiness {
    /// The dup registered with Tokio readiness. All read/write I/O runs on this fd.
    inner: AsyncFd<OwnedFd>,
    /// The descriptor status flags captured before this transport set the dup non-blocking, put
    /// back on every teardown path so the non-blocking flag never leaks onto the shared
    /// description.
    original_flags: OFlags,
}

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
fn fd_write(fd: &OwnedFd, bytes: &[u8]) -> io::Result<usize> {
    rustix::io::write(fd, bytes).map_err(io::Error::from)
}

/// Reads into `buffer` from the readiness-registered descriptor with one `read(2)`.
///
/// Returns `Ok(0)` at end of input. Runs on the registered fd for the same readiness-correctness
/// reason as [`fd_write`].
fn fd_read(fd: &OwnedFd, buffer: &mut [u8]) -> io::Result<usize> {
    rustix::io::read(fd, buffer).map_err(io::Error::from)
}
