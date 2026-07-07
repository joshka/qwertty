//! Panic-safe terminal restoration.
//!
//! [`RestoreHandle`] is the emergency exit path from design 01: a cheap handle, obtainable
//! without exclusive session access, that restores the terminal from a panic hook. The handle
//! holds a precomposed teardown blob in a double-buffered cell so the hook path only writes
//! bytes and restores the captured terminal mode; it never composes or allocates.

use std::fs::File;
use std::io::{self, Write};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, PoisonError};
#[cfg(not(loom))]
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[cfg(loom)]
use loom::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicUsize, Ordering},
};
use rustix::termios::{OptionalActions, Termios, tcsetattr};

/// How many times the emergency write retries a blocked terminal before giving up.
const EMERGENCY_WRITE_RETRIES: u32 = 5;
/// How long the emergency write waits between retries on a blocked terminal.
const EMERGENCY_WRITE_RETRY_DELAY: Duration = Duration::from_millis(10);

/// A byte buffer readable while a single writer republishes it.
///
/// The writer renders into the inactive buffer and swaps the active index, so a reader always
/// observes a complete published value: either the previous blob or the new one, never a torn
/// mix. This is the one piece of genuinely concurrent shared state in the session design, and
/// its swap protocol is verified under loom (see the `loom_` tests).
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
#[derive(Clone, Debug)]
pub struct RestoreHandle {
    shared: Arc<RestoreShared>,
}

#[derive(Debug)]
struct RestoreShared {
    device: File,
    cooked: Termios,
    armed: AtomicBool,
    blob: DoubleBuffered,
}

impl RestoreHandle {
    pub(crate) fn new(device: File, cooked: Termios) -> Self {
        Self {
            shared: Arc::new(RestoreShared {
                device,
                cooked,
                armed: AtomicBool::new(false),
                blob: DoubleBuffered::default(),
            }),
        }
    }

    /// Publishes the current teardown bytes for the emergency path.
    pub(crate) fn publish_blob(&self, bytes: &[u8]) {
        self.shared.blob.publish(bytes);
    }

    /// Arms the emergency path after the session (re-)enters terminal state.
    pub(crate) fn arm(&self) {
        self.shared
            .armed
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Claims restoration, disarming the emergency path.
    ///
    /// Returns `true` when this call was the one that disarmed it, so exactly one path restores
    /// per armed period.
    pub(crate) fn disarm(&self) -> bool {
        self.shared
            .armed
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    /// Restores the terminal, best-effort.
    ///
    /// Writes the precomposed teardown bytes with a bounded retry so a stalled terminal cannot
    /// hang the hook, then restores the captured terminal mode. Returns `true` when this call
    /// performed restoration and `false` when the session is not currently in a state that
    /// needs it (never entered, already left, or already restored).
    #[must_use = "the result reports whether this call performed restoration"]
    pub fn restore(&self) -> bool {
        if !self.disarm() {
            return false;
        }

        self.shared
            .blob
            .read_current(|blob| write_bounded(&self.shared.device, blob));
        _ = tcsetattr(
            &self.shared.device,
            OptionalActions::Now,
            &self.shared.cooked,
        );
        true
    }
}

/// Writes bytes with a bounded retry so a blocked terminal cannot hang the emergency path.
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
