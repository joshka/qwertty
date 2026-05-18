//! Tokio-backed terminal session ownership.
//!
//! This module owns the first async runtime boundary. It uses Tokio readiness for terminal reads
//! and writes instead of wrapping the synchronous [`crate::TerminalSession`] methods in async
//! functions.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::termios::{OptionalActions, Termios, tcgetattr, tcgetwinsize, tcsetattr};
use tokio::io::unix::AsyncFd;
use tokio::time::{Instant, timeout_at};

use crate::{
    Command, CursorPositionReport, InputBytes, InputDecoder, InputEvent, TerminalSize,
    TerminalStatusReport, commands, terminal,
};

const DEV_TTY: &str = "/dev/tty";
const READ_BUFFER_LEN: usize = 1024;

/// A Tokio-backed terminal session.
///
/// `TokioTerminalSession` is available when the `tokio` feature is enabled. It owns a live
/// terminal device registered with Tokio readiness, enters raw mode when the session starts,
/// writes output bytes in method-call order, reads input through runtime-backed I/O, decodes input
/// with [`InputDecoder`], and gives callers an explicit async [`leave`](Self::leave) path for
/// terminal-mode cleanup errors.
///
/// The existing [`crate::TerminalSession`] type remains runtime-neutral. This type is not a thin
/// async wrapper around its blocking methods.
///
/// # Example
///
/// ```no_run
/// use qwertty::{ProtocolPosition, TokioTerminalSession, commands};
///
/// # async fn run() -> qwertty::Result<()> {
/// let mut session = TokioTerminalSession::open()?;
///
/// session.command(commands::screen::clear()).await?;
/// session
///     .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))
///     .await?;
/// session.text("Ready\r\n").await?;
/// session.flush().await?;
/// session.leave().await
/// # }
/// ```
#[derive(Debug)]
pub struct TokioTerminalSession {
    device: AsyncFd<File>,
    original_mode: Termios,
    path: PathBuf,
    decoder: InputDecoder,
    query_routing: QueryRouting,
}

#[derive(Debug, Default)]
struct QueryRouting {
    events: VecDeque<InputEvent>,
}

impl TokioTerminalSession {
    /// Opens the current controlling terminal and starts a Tokio-backed session.
    ///
    /// This opens `/dev/tty`, captures the current terminal mode, enters raw mode, sets the
    /// session file descriptor to nonblocking mode, and registers it with the current Tokio
    /// runtime.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal cannot be opened, configured, or registered with Tokio.
    pub fn open() -> terminal::Result<Self> {
        Self::open_path(DEV_TTY)
    }

    /// Opens a specific terminal device path and starts a Tokio-backed session.
    ///
    /// This is mainly useful for tests, embedding environments, and advanced callers that have
    /// already resolved the terminal device they want qwertty to own.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the path cannot be opened as a terminal device, raw mode cannot be
    /// entered, nonblocking mode cannot be set, or Tokio cannot register the file descriptor.
    pub fn open_path(path: impl Into<PathBuf>) -> terminal::Result<Self> {
        let path = path.into();
        let device = open_read_write(&path).map_err(terminal::Error::open_terminal)?;
        let original_mode = tcgetattr(&device)
            .map_err(io::Error::from)
            .map_err(terminal::Error::get_terminal_mode)?;

        set_nonblocking(&device).map_err(terminal::Error::open_terminal)?;
        set_raw_mode(&device, &original_mode)?;

        let device = match AsyncFd::try_new(device) {
            Ok(device) => device,
            Err(err) => {
                let (device, err) = err.into_parts();
                _ = set_cooked_mode(&device, &original_mode);
                return Err(terminal::Error::open_terminal(err));
            }
        };

        Ok(Self {
            device,
            original_mode,
            path,
            decoder: InputDecoder::new(),
            query_routing: QueryRouting::default(),
        })
    }

    /// Returns the path used to open the terminal device.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the current terminal size.
    ///
    /// The result is a snapshot. This method does not subscribe to future resize events.
    ///
    /// # Errors
    ///
    /// Returns an error when the operating system cannot read the terminal size.
    pub fn size(&self) -> terminal::Result<TerminalSize> {
        let size = tcgetwinsize(self.file())
            .map_err(io::Error::from)
            .map_err(terminal::Error::get_terminal_size)?;

        Ok(TerminalSize::new(size.ws_col, size.ws_row))
    }

    /// Writes one terminal command through Tokio readiness.
    ///
    /// Commands, raw bytes, and text are written in the order their session methods are awaited.
    /// The command bytes are not flushed until [`TokioTerminalSession::flush`] is called or the
    /// operating system decides to make them visible.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write all encoded bytes.
    pub async fn command(&mut self, command: impl AsRef<Command>) -> terminal::Result<()> {
        let mut bytes = Vec::new();
        command.as_ref().encode(&mut bytes);
        self.bytes(bytes).await
    }

    /// Writes raw bytes through Tokio readiness.
    ///
    /// This method does not inspect, escape, or validate bytes. Use it for renderer output that is
    /// already encoded. Prefer [`TokioTerminalSession::text`] for ordinary UTF-8 render text.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write all bytes.
    pub async fn bytes(&mut self, bytes: impl AsRef<[u8]>) -> terminal::Result<()> {
        let mut bytes = bytes.as_ref();
        while !bytes.is_empty() {
            let mut guard = self
                .device
                .writable_mut()
                .await
                .map_err(terminal::Error::write_terminal)?;

            match guard.try_io(|device| device.get_mut().write(bytes)) {
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

    /// Writes UTF-8 render text through Tokio readiness.
    ///
    /// This method does not escape control characters. Renderers that accept user-controlled text
    /// should perform their own escaping policy before writing to a terminal stream.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot write all text bytes.
    pub async fn text(&mut self, text: impl AsRef<str>) -> terminal::Result<()> {
        self.bytes(text.as_ref()).await
    }

    /// Reads raw terminal input bytes into `buffer` through Tokio readiness.
    ///
    /// This method returns one operating-system read as [`InputBytes`]. It does not decode UTF-8,
    /// parse Escape sequences, match terminal query responses, classify keys, or apply paste,
    /// mouse, focus, graphics, clipboard, or vendor protocol policy.
    ///
    /// In raw mode, the returned bytes are the foundation for later event and query-routing
    /// layers. A zero-length buffer returns an empty input value without reading from the terminal.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot read input.
    pub async fn read_input(&mut self, buffer: &mut [u8]) -> terminal::Result<InputBytes> {
        if buffer.is_empty() {
            return Ok(InputBytes::default());
        }

        loop {
            let mut guard = self
                .device
                .readable_mut()
                .await
                .map_err(terminal::Error::read_terminal)?;

            match guard.try_io(|device| device.get_mut().read(buffer)) {
                Ok(Ok(len)) => return Ok(InputBytes::new(buffer[..len].to_vec())),
                Ok(Err(err)) => return Err(terminal::Error::read_terminal(err)),
                Err(_would_block) => {}
            }
        }
    }

    /// Reads and decodes the next terminal input event.
    ///
    /// This method keeps decoded events that share a terminal read in an internal queue. If a call
    /// is canceled while waiting for the terminal to become readable, previously queued events and
    /// decoder state remain available to later calls.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot read input or returns end-of-file before
    /// another event is available.
    pub async fn next_event(&mut self) -> terminal::Result<InputEvent> {
        loop {
            if let Some(event) = self.query_routing.next_event() {
                return Ok(event);
            }

            let mut buffer = [0; READ_BUFFER_LEN];
            let input = self.read_input(&mut buffer).await?;
            if input.is_empty() {
                return Err(terminal::Error::read_terminal(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "terminal input closed before another event was available",
                )));
            }

            self.query_routing.push_events(self.decoder.decode(input));
        }
    }

    /// Requests and reads the current terminal cursor position.
    ///
    /// This method emits the Device Status Report request `CSI 6 n`, flushes output, and reads
    /// decoded input events until it sees a `CSI row ; column R` cursor position report. Events
    /// read before the report that are not the report remain queued in their original order for
    /// later [`TokioTerminalSession::next_event`] calls.
    ///
    /// `timeout` bounds the whole request/response operation. If the timeout elapses,
    /// [`terminal::Error::QueryTimeout`] is returned. Canceling the future while it is waiting for
    /// terminal input leaves the session usable, and unrelated decoded events already seen by the
    /// query remain queued for later calls.
    ///
    /// This is a single-query convenience method. It does not implement a general query registry,
    /// concurrent query routing, capability probing, or terminal feature detection.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use qwertty::TokioTerminalSession;
    ///
    /// # async fn run() -> qwertty::Result<()> {
    /// let mut session = TokioTerminalSession::open()?;
    /// let report = session
    ///     .request_cursor_position(Duration::from_secs(1))
    ///     .await?;
    ///
    /// assert!(report.row() > 0);
    /// assert!(report.column() > 0);
    ///
    /// session.leave().await
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when writing, flushing, or reading terminal I/O fails, or when the timeout
    /// elapses before a cursor position report is received.
    pub async fn request_cursor_position(
        &mut self,
        timeout: Duration,
    ) -> terminal::Result<CursorPositionReport> {
        self.command(commands::cursor::request_position()).await?;
        self.flush().await?;

        let deadline = Instant::now() + timeout;
        if let Some(report) = self.query_routing.match_cursor_position_report() {
            return Ok(report);
        }

        loop {
            let mut buffer = [0; READ_BUFFER_LEN];
            let input = match timeout_at(deadline, self.read_input(&mut buffer)).await {
                Ok(Ok(input)) => input,
                Ok(Err(err)) => return Err(err),
                Err(_elapsed) => {
                    return Err(terminal::Error::query_timeout(
                        "cursor position query",
                        timeout,
                    ));
                }
            };

            if input.is_empty() {
                return Err(terminal::Error::read_terminal(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "terminal input closed before a cursor position report was available",
                )));
            }

            let matched = CursorPositionReport::match_events(self.decoder.decode(input));
            let report = self.query_routing.push_cursor_position_match(matched);

            if let Some(report) = report {
                return Ok(report);
            }
        }
    }

    /// Requests and reads terminal status.
    ///
    /// This method emits the Device Status Report request `CSI 5 n`, flushes output, and reads
    /// decoded input events until it sees a `CSI 0 n` ready report or a `CSI 3 n` malfunction
    /// report. Events read before the report that are not the report remain queued in their
    /// original order for later [`TokioTerminalSession::next_event`] calls.
    ///
    /// `timeout` bounds the whole request/response operation. If the timeout elapses,
    /// [`terminal::Error::QueryTimeout`] is returned. Canceling the future while it is waiting for
    /// terminal input leaves the session usable, and unrelated decoded events already seen by the
    /// query remain queued for later calls.
    ///
    /// This is a single-query convenience method. It does not implement a general query registry,
    /// concurrent query routing, capability probing, or terminal feature detection.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use qwertty::{TerminalStatus, TokioTerminalSession};
    ///
    /// # async fn run() -> qwertty::Result<()> {
    /// let mut session = TokioTerminalSession::open()?;
    /// let report = session
    ///     .request_terminal_status(Duration::from_secs(1))
    ///     .await?;
    ///
    /// assert_eq!(report.status(), TerminalStatus::Ready);
    ///
    /// session.leave().await
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when writing, flushing, or reading terminal I/O fails, or when the timeout
    /// elapses before a terminal status report is received.
    pub async fn request_terminal_status(
        &mut self,
        timeout: Duration,
    ) -> terminal::Result<TerminalStatusReport> {
        self.command(commands::terminal::request_status()).await?;
        self.flush().await?;

        let deadline = Instant::now() + timeout;
        if let Some(report) = self.query_routing.match_terminal_status_report() {
            return Ok(report);
        }

        loop {
            let mut buffer = [0; READ_BUFFER_LEN];
            let input = match timeout_at(deadline, self.read_input(&mut buffer)).await {
                Ok(Ok(input)) => input,
                Ok(Err(err)) => return Err(err),
                Err(_elapsed) => {
                    return Err(terminal::Error::query_timeout(
                        "terminal status query",
                        timeout,
                    ));
                }
            };

            if input.is_empty() {
                return Err(terminal::Error::read_terminal(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "terminal input closed before a terminal status report was available",
                )));
            }

            let matched = TerminalStatusReport::match_events(self.decoder.decode(input));
            let report = self.query_routing.push_terminal_status_match(matched);

            if let Some(report) = report {
                return Ok(report);
            }
        }
    }

    /// Flushes buffered terminal output through Tokio readiness.
    ///
    /// Call this when the preceding command, byte, and text writes must be visible before later
    /// application work continues.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal device cannot flush output.
    pub async fn flush(&mut self) -> terminal::Result<()> {
        loop {
            let mut guard = self
                .device
                .writable_mut()
                .await
                .map_err(terminal::Error::write_terminal)?;

            match guard.try_io(|device| device.get_mut().flush()) {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(err)) => return Err(terminal::Error::write_terminal(err)),
                Err(_would_block) => {}
            }
        }
    }

    /// Leaves the session and restores cooked mode.
    ///
    /// This is the orderly cleanup path. It reports terminal-mode restoration errors to the
    /// caller. It does not flush pending output, route query responses, or clean up protocol state
    /// such as alternate screen, cursor visibility, mouse mode, paste mode, graphics, clipboard,
    /// or vendor extensions.
    ///
    /// Call [`TokioTerminalSession::flush`] before `leave` when output visibility matters. Drop
    /// still attempts best-effort cooked-mode restoration, but drop-time failures cannot be
    /// returned.
    ///
    /// # Errors
    ///
    /// Returns an error when cooked mode cannot be restored or Tokio cannot join the cleanup task.
    pub async fn leave(self) -> terminal::Result<()> {
        tokio::task::spawn_blocking(move || self.set_cooked_mode())
            .await
            .map_err(|err| terminal::Error::set_terminal_mode(io::Error::other(err)))?
    }

    fn file(&self) -> &File {
        self.device.get_ref()
    }

    fn set_cooked_mode(&self) -> terminal::Result<()> {
        set_cooked_mode(self.file(), &self.original_mode)
    }
}

impl QueryRouting {
    fn next_event(&mut self) -> Option<InputEvent> {
        self.events.pop_front()
    }

    fn push_events(&mut self, events: Vec<InputEvent>) {
        self.events.extend(events);
    }

    fn match_cursor_position_report(&mut self) -> Option<CursorPositionReport> {
        if self.events.is_empty() {
            return None;
        }

        let events = self.events.drain(..);
        let matched = CursorPositionReport::match_events(events);
        let (report, remaining) = matched.into_parts();
        self.push_front_events(remaining);
        report
    }

    fn push_cursor_position_match(
        &mut self,
        matched: crate::CursorPositionReportMatch,
    ) -> Option<CursorPositionReport> {
        let (report, remaining) = matched.into_parts();
        self.push_events(remaining);
        report
    }

    fn match_terminal_status_report(&mut self) -> Option<TerminalStatusReport> {
        if self.events.is_empty() {
            return None;
        }

        let events = self.events.drain(..);
        let matched = TerminalStatusReport::match_events(events);
        let (report, remaining) = matched.into_parts();
        self.push_front_events(remaining);
        report
    }

    fn push_terminal_status_match(
        &mut self,
        matched: crate::TerminalStatusReportMatch,
    ) -> Option<TerminalStatusReport> {
        let (report, remaining) = matched.into_parts();
        self.push_events(remaining);
        report
    }

    fn push_front_events(&mut self, events: Vec<InputEvent>) {
        for event in events.into_iter().rev() {
            self.events.push_front(event);
        }
    }
}

impl Drop for TokioTerminalSession {
    fn drop(&mut self) {
        _ = self.set_cooked_mode();
    }
}

fn open_read_write(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    let flags = fcntl_getfl(file.as_fd())?;
    fcntl_setfl(file.as_fd(), flags | OFlags::NONBLOCK)?;
    Ok(())
}

fn set_raw_mode(file: &File, original_mode: &Termios) -> terminal::Result<()> {
    let mut raw = original_mode.clone();
    raw.make_raw();
    tcsetattr(file, OptionalActions::Flush, &raw)
        .map_err(io::Error::from)
        .map_err(terminal::Error::set_terminal_mode)
}

fn set_cooked_mode(file: &File, original_mode: &Termios) -> terminal::Result<()> {
    tcsetattr(file, OptionalActions::Now, original_mode)
        .map_err(io::Error::from)
        .map_err(terminal::Error::set_terminal_mode)
}
