//! Windows console terminal device — a compiling derisk STUB for the platform seam (design 07).
//!
//! # Why this file exists
//!
//! It answers ONE pre-freeze question cheaply: does the [`TerminalDevice`] trait, as it will be
//! frozen, accommodate a Windows console implementation without a signature change? The methods
//! here are deliberately THIN — several return [`Error::Unsupported`] for behavior not yet built —
//! but every one binds a *real* `windows-sys` console entry point with its real signature, so the
//! trait shape is proven against the actual Win32 surface rather than against a sketch:
//!
//! | trait method             | Win32 mechanism                                    |
//! | ------------------------ | -------------------------------------------------- |
//! | [`set_mode`]             | `GetConsoleMode` + `SetConsoleMode` (raw ↔ cooked) |
//! | [`size`]                 | `GetConsoleScreenBufferInfo` (window rect)         |
//! | [`read`]                 | `ReadConsoleW` on the `CONIN$` handle              |
//! | [`write_all`] / [`flush`]| `WriteConsoleW` on the `CONOUT$` handle            |
//!
//! [`set_mode`]: TerminalDevice::set_mode
//! [`size`]: TerminalDevice::size
//! [`read`]: TerminalDevice::read
//! [`write_all`]: TerminalDevice::write_all
//! [`flush`]: TerminalDevice::flush
//!
//! # What this file does NOT do (on purpose)
//!
//! - No async readiness. There is no `AsyncFd` equivalent for a console input handle; the readiness
//!   model (worker thread + channel, or overlapped I/O) is the open question analysed in
//!   `work/phase4/windows-readiness-analysis.md`, NOT built here. The trait's `as_fd` hook is
//!   `#[cfg(unix)]`, so this device simply never provides it — confirmed sufficient below.
//! - No VT-input policy, no win32-input-mode toggle, no console-mode restore ledger. Those are
//!   later-milestone concerns (FM-W2/W3/W4). The raw/cooked mapping here flips the classic
//!   line-input/echo bits only, enough to prove the mode seam.
//! - No UTF-16 ↔ UTF-8 transcoding correctness guarantees. `read` reports `Unsupported` rather than
//!   hand a caller half-built surrogate handling (FM-W5); `write_all` performs a lossy UTF-8→UTF-16
//!   transcode purely to exercise the `WriteConsoleW` signature.
//!
//! The point is the *seam*, not the implementation. If a trait method could not be satisfied on
//! Windows, that would be a finding reported to the maintainer; as written, every method is
//! satisfiable.

// SAFETY SCOPE: this module is the crate's only `unsafe`. Every `unsafe` block wraps a single
// documented `windows-sys` FFI call whose contract is checked at the call site (handle validity and
// out-pointer initialization). The crate lint is `unsafe_code = "deny"` (not `forbid`) precisely so
// this one `#[cfg(windows)]` module can opt in; the Unix and pure layers carry no `unsafe`. See the
// Cargo.toml lint note and work/phase4/windows-readiness-analysis.md (pre-freeze finding).
#![allow(
    unsafe_code,
    reason = "the Windows console device is FFI-only; every Win32 console entry point is an unsafe \
              extern \"system\" call with no safe wrapper in the dependency tree"
)]

use std::io;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{
    CONSOLE_MODE, CONSOLE_SCREEN_BUFFER_INFO, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT,
    ENABLE_PROCESSED_INPUT, GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle, ReadConsoleW,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode, WriteConsoleW,
};

use crate::terminal::{DeviceMode, Error, Result, TerminalDevice, TerminalSize};

const PLATFORM: &str = "windows";

/// The console input-mode bits raw mode clears (line buffering, echo, processed input).
///
/// Cooked mode is the inverse: these bits set. This is the classic console analogue of the Unix
/// termios `ICANON`/`ECHO` toggle and is intentionally the *whole* mode model here — enhancement
/// bits such as `ENABLE_VIRTUAL_TERMINAL_INPUT` and `ENABLE_MOUSE_INPUT` are a later-milestone
/// policy decision (FM-W2/W4), not part of this derisk stub.
const RAW_INPUT_CLEAR_BITS: CONSOLE_MODE =
    ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT;

/// A live Windows console terminal device.
///
/// Wraps the process console input (`CONIN$`) and output (`CONOUT$`) handles fetched through
/// `GetStdHandle`, mirroring the Unix [`Terminal`](crate::Terminal) surface so
/// [`TerminalSession`](crate::TerminalSession) composes over it unchanged.
#[derive(Debug)]
pub struct Terminal {
    /// The console input handle (`CONIN$`), from `GetStdHandle(STD_INPUT_HANDLE)`.
    input: ConsoleHandle,
    /// The console output handle (`CONOUT$`), from `GetStdHandle(STD_OUTPUT_HANDLE)`.
    output: ConsoleHandle,
    /// The input mode captured when the device was opened, restored as cooked mode on teardown.
    ///
    /// This is the Windows analogue of the Unix `cooked_mode()` termios snapshot and is what makes
    /// console-mode restore (FM-W4) a solved problem rather than a leak: teardown writes exactly
    /// the bits that were live at open, not a guessed default.
    original_input_mode: CONSOLE_MODE,
    /// A synthetic device path, kept only so this type mirrors the Unix `Terminal::path` surface.
    path: PathBuf,
}

/// A newtype over a raw Win32 `HANDLE`, so the raw pointer never leaks into the device's public
/// shape and `Debug` stays meaningful.
#[derive(Clone, Copy)]
struct ConsoleHandle(HANDLE);

impl std::fmt::Debug for ConsoleHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ConsoleHandle({:p})", self.0)
    }
}

impl Terminal {
    /// Opens the current console by fetching the standard input and output handles.
    ///
    /// This is the Windows counterpart to `Terminal::open` on Unix. It fetches `CONIN$`/`CONOUT$`
    /// through `GetStdHandle`, captures the current console input mode for restore, and returns a
    /// device ready for the session to enter raw mode through
    /// [`set_mode`](TerminalDevice::set_mode).
    ///
    /// # Errors
    ///
    /// Returns [`Error::OpenTerminal`] when either standard handle is invalid (for example when the
    /// process has no attached console), or [`Error::GetTerminalMode`] when the current input mode
    /// cannot be read.
    pub fn open() -> Result<Self> {
        // SAFETY: GetStdHandle takes a documented STD_*_HANDLE identifier and returns a process
        // pseudo-handle; it has no preconditions. INVALID_HANDLE_VALUE is checked below.
        let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        // SAFETY: as above, for the output handle.
        let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };

        if input == INVALID_HANDLE_VALUE || input.is_null() {
            return Err(Error::open_terminal(io::Error::new(
                io::ErrorKind::NotFound,
                "no console input handle (CONIN$) available",
            )));
        }
        if output == INVALID_HANDLE_VALUE || output.is_null() {
            return Err(Error::open_terminal(io::Error::new(
                io::ErrorKind::NotFound,
                "no console output handle (CONOUT$) available",
            )));
        }

        let original_input_mode = get_console_mode(input)?;

        Ok(Self {
            input: ConsoleHandle(input),
            output: ConsoleHandle(output),
            original_input_mode,
            path: PathBuf::from("CONIN$"),
        })
    }

    /// Opens a specific console path.
    ///
    /// A console has no per-path device the way a Unix pty does; the argument is accepted for
    /// surface parity with the Unix [`Terminal::open_path`](crate::Terminal) and ignored beyond
    /// being recorded as [`path`](Self::path). Opening `CONIN$`/`CONOUT$` explicitly via
    /// `CreateFileW` is a later-milestone concern (it matters when standard handles are
    /// redirected); this stub reuses [`open`](Self::open).
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`open`](Self::open).
    pub fn open_path(path: impl Into<PathBuf>) -> Result<Self> {
        let mut terminal = Self::open()?;
        terminal.path = path.into();
        Ok(terminal)
    }

    /// Returns the path used to open the terminal device.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the current console window size in character cells.
    ///
    /// Reads `GetConsoleScreenBufferInfo` and derives the *window* dimensions (not the full
    /// scrollback buffer) from `srWindow`, matching what a terminal caller means by size. The
    /// degenerate-size hygiene the Unix path applies (FM-Z2) is a later-milestone concern here;
    /// this stub returns the raw window rect.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GetTerminalSize`] when the console screen-buffer info cannot be read.
    pub fn size(&self) -> Result<TerminalSize> {
        let info = get_screen_buffer_info(self.output.0)?;
        let window = info.srWindow;
        // srWindow is inclusive on both ends, so add one to each span.
        let columns = (window.Right - window.Left + 1).max(0);
        let rows = (window.Bottom - window.Top + 1).max(0);
        Ok(TerminalSize::new(
            u16::try_from(columns).unwrap_or(0),
            u16::try_from(rows).unwrap_or(0),
        ))
    }

    /// Enters raw mode by clearing the line-input, echo, and processed-input bits.
    ///
    /// This is the Windows analogue of disabling termios `ICANON`/`ECHO`: with these bits cleared,
    /// `ReadConsoleW` returns bytes as typed instead of a cooked line. Enhancement bits
    /// (`ENABLE_VIRTUAL_TERMINAL_INPUT`, win32-input-mode) are deliberately NOT toggled here — that
    /// is the all-or-nothing policy decision deferred to the Windows milestone (FM-W2/W3).
    ///
    /// # Errors
    ///
    /// Returns [`Error::GetTerminalMode`]/[`Error::SetTerminalMode`] when the console input mode
    /// cannot be read or written.
    pub fn set_raw_mode(&self) -> Result<()> {
        let current = get_console_mode(self.input.0)?;
        set_console_mode(self.input.0, current & !RAW_INPUT_CLEAR_BITS)
    }

    /// Restores cooked mode by putting back the input mode captured at open.
    ///
    /// Restoring the *captured* mode rather than a synthesized default is what makes console-mode
    /// restore a solved problem rather than the FM-W4 leak crossterm ships (it leaves
    /// `ENABLE_MOUSE_INPUT`/`ENABLE_VIRTUAL_TERMINAL_INPUT` set); a session's mode ledger (design
    /// 01) drives this on every teardown path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SetTerminalMode`] when the console input mode cannot be written.
    pub fn set_cooked_mode(&self) -> Result<()> {
        set_console_mode(self.input.0, self.original_input_mode)
    }

    /// Writes all bytes to the console output handle via `WriteConsoleW`.
    ///
    /// The bytes are treated as UTF-8, transcoded to UTF-16, and written with `WriteConsoleW`. This
    /// exercises the real output-path signature; a production impl must decide the CONOUT$ encoding
    /// policy and handle partial writes and non-console (redirected) output (FM-W6), which this
    /// stub does not.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WriteTerminal`] when the console write fails or does not consume every
    /// UTF-16 unit.
    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        // Lossy transcode is acceptable for a signature-proving stub; a real impl owns the encoding
        // policy. This proves the WriteConsoleW call shape, nothing more.
        let utf16: Vec<u16> = String::from_utf8_lossy(bytes).encode_utf16().collect();
        let mut written: u32 = 0;
        let len = u32::try_from(utf16.len()).unwrap_or(u32::MAX);

        // SAFETY: `self.output.0` was validated non-invalid at open; `utf16` owns `len` u16 units;
        // `written` is a live out-param. The reserved fifth argument is null per the API contract.
        let ok = unsafe {
            WriteConsoleW(
                self.output.0,
                utf16.as_ptr().cast(),
                len,
                &raw mut written,
                std::ptr::null(),
            )
        };
        if ok == 0 {
            return Err(Error::write_terminal(io::Error::last_os_error()));
        }
        if written != len {
            return Err(Error::write_terminal(io::Error::new(
                io::ErrorKind::WriteZero,
                "WriteConsoleW wrote fewer units than requested",
            )));
        }
        Ok(())
    }

    /// Reads console input bytes into `buffer`.
    ///
    /// **Not yet implemented.** Reads bind `ReadConsoleW`, which returns UTF-16 units; a correct
    /// UTF-16→UTF-8 read (including split surrogate pairs across reads, FM-W5, and the VT-vs-legacy
    /// input decision, FM-W3) is a Windows-milestone deliverable, not a derisk stub. Returning
    /// [`Error::Unsupported`] here is the honest boundary: the signature is proven (see
    /// [`read_console_raw`]), the semantics are declared future work.
    ///
    /// # Errors
    ///
    /// Always returns [`Error::Unsupported`] on this platform for now.
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        let _ = buffer;
        Err(Error::unsupported("read console input", PLATFORM))
    }

    /// Flushes buffered console output.
    ///
    /// `WriteConsoleW` writes straight to the console screen buffer with no library-side buffering,
    /// so there is nothing to flush; this is a success. Kept for trait/surface parity.
    ///
    /// # Errors
    ///
    /// Never returns an error today.
    pub fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Proves the `ReadConsoleW` call shape without committing to a UTF-16 decode policy.
///
/// This helper exists to keep [`ReadConsoleW`] a *used* import and to show the read binding is
/// satisfiable: it reads UTF-16 units into a scratch buffer and reports how many were read. The
/// public [`Terminal::read`] deliberately does not call it yet — turning these units into correct
/// UTF-8 events (surrogate reassembly, FM-W5) is the Windows-milestone work. Present but
/// unexported.
#[allow(
    dead_code,
    reason = "signature-proving helper; wired up in the Windows milestone"
)]
fn read_console_raw(handle: HANDLE, units: &mut [u16]) -> Result<usize> {
    let mut read: u32 = 0;
    let len = u32::try_from(units.len()).unwrap_or(u32::MAX);
    // SAFETY: `handle` is a caller-supplied console input handle; `units` owns `len` u16 slots;
    // `read` is a live out-param. The final `CONSOLE_READCONSOLE_CONTROL` argument is null (no
    // wakeup-mask control), which the API permits.
    let ok = unsafe {
        ReadConsoleW(
            handle,
            units.as_mut_ptr().cast(),
            len,
            &raw mut read,
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(Error::read_terminal(io::Error::last_os_error()));
    }
    Ok(read as usize)
}

/// Reads a console mode via `GetConsoleMode`.
fn get_console_mode(handle: HANDLE) -> Result<CONSOLE_MODE> {
    let mut mode: CONSOLE_MODE = 0;
    // SAFETY: `handle` is a console handle validated by the caller; `mode` is a live out-param.
    let ok = unsafe { GetConsoleMode(handle, &raw mut mode) };
    if ok == 0 {
        return Err(Error::get_terminal_mode(io::Error::last_os_error()));
    }
    Ok(mode)
}

/// Writes a console mode via `SetConsoleMode`.
fn set_console_mode(handle: HANDLE, mode: CONSOLE_MODE) -> Result<()> {
    // SAFETY: `handle` is a console handle validated by the caller; `mode` is a plain value.
    let ok = unsafe { SetConsoleMode(handle, mode) };
    if ok == 0 {
        return Err(Error::set_terminal_mode(io::Error::last_os_error()));
    }
    Ok(())
}

/// Reads console screen-buffer info via `GetConsoleScreenBufferInfo`.
fn get_screen_buffer_info(handle: HANDLE) -> Result<CONSOLE_SCREEN_BUFFER_INFO> {
    // A zeroed CONSOLE_SCREEN_BUFFER_INFO is a valid initial value; the call fills it.
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: `handle` is a console output handle validated by the caller; `info` is a live,
    // fully-owned out-param sized exactly to the struct the API writes.
    let ok = unsafe { GetConsoleScreenBufferInfo(handle, &raw mut info) };
    if ok == 0 {
        return Err(Error::get_terminal_size(io::Error::last_os_error()));
    }
    Ok(info)
}

impl TerminalDevice for Terminal {
    fn set_mode(&mut self, mode: DeviceMode) -> Result<()> {
        match mode {
            DeviceMode::Raw => self.set_raw_mode(),
            DeviceMode::Cooked => self.set_cooked_mode(),
        }
    }

    fn size(&self) -> Result<TerminalSize> {
        Self::size(self)
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        Self::read(self, buffer)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        Self::write_all(self, bytes)
    }

    fn flush(&mut self) -> Result<()> {
        Self::flush(self)
    }

    // NOTE: no `as_fd` override. The trait's `as_fd` hook is `#[cfg(unix)]`, so a Windows device
    // simply does not provide it — which is correct: a console input HANDLE is not a pollable fd
    // and could not be registered with `AsyncFd` anyway. The readiness seam Windows needs
    // instead is analysed in work/phase4/windows-readiness-analysis.md; it is NOT a
    // `TerminalDevice` method.
}
