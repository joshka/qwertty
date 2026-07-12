//! Windows console terminal device — the live console implementation (ADR 0022, milestone MW-1).
//!
//! # Why this file exists
//!
//! This is the Windows analogue of [`unix::Terminal`](super::unix): it owns the process console,
//! captures its modes so they can be restored, enters raw mode, reads input, and writes output —
//! implementing [`TerminalDevice`] so a [`TerminalSession`](crate::TerminalSession) composes over
//! it unchanged. Every method binds a real `windows-sys` console entry point:
//!
//! | trait method              | Win32 mechanism                                              |
//! | ------------------------- | ----------------------------------------------------------- |
//! | [`open`]                  | `CreateFileW("CONIN$"/"CONOUT$")` (owns the console directly) |
//! | [`set_mode`]              | `GetConsoleMode`/`SetConsoleMode` + `SetConsoleOutputCP`     |
//! | [`size`]                  | `GetConsoleScreenBufferInfo` (window rect, FM-Z2 hygiene)    |
//! | [`read`]                  | `ReadConsoleInputW` (records → VT bytes)                     |
//! | [`write_all`]             | `WriteFile` on `CONOUT$` (UTF-8 passthrough, codepage 65001) |
//! | [`flush`]                 | no-op (`WriteFile` writes straight through)                 |
//!
//! [`open`]: Terminal::open
//! [`set_mode`]: TerminalDevice::set_mode
//! [`size`]: TerminalDevice::size
//! [`read`]: TerminalDevice::read
//! [`write_all`]: TerminalDevice::write_all
//! [`flush`]: TerminalDevice::flush
//!
//! # Design commitments (ADR 0022)
//!
//! - **The console is opened by name, not by inheriting stdio.** `CreateFileW("CONIN$")` /
//!   `CreateFileW("CONOUT$")` address the console itself, so the device owns it even when the
//!   process stdin/stdout are redirected — exactly as `open("/dev/tty")` does on Unix.
//! - **VT is mandatory, with no legacy fallback.** Raw mode sets
//!   `ENABLE_VIRTUAL_TERMINAL_PROCESSING` on output and reads the mode back to confirm it stuck; a
//!   host that silently drops the bit fails `set_mode` with a typed error rather than falling back
//!   to a `SetConsoleTextAttribute` rendering path (ADR 0022 §2).
//! - **Input is read as records, not a byte stream.** `ReadConsoleInputW` yields the VT bytes the
//!   host packs into `KEY_EVENT` records *and* the `WINDOW_BUFFER_SIZE_EVENT` resize records a byte
//!   read would discard (Windows has no VT resize sequence). Records are translated to the same VT
//!   byte stream the platform-neutral decoder parses everywhere.
//! - **Output is UTF-8 straight through `WriteFile`.** The output codepage is 65001 for the raw
//!   session's lifetime, so no UTF-16 transcoding is needed; the codepage is console-global state,
//!   captured at open and restored with the rest of the mode.
//!
//! # What is NOT here (later milestones)
//!
//! No async readiness worker (MW-2), no panic-safe `RestoreHandle` or ctrl-handler signals (MW-3),
//! no win32-input-mode toggle (MW-4b), no `ConPTY` (MW-5). [`read`](Terminal::read) blocks in
//! `ReadConsoleInputW`; that is correct for the synchronous device and is what the readiness worker
//! will wrap, not replace.
//!
//! The console-free translation logic (UTF-16 → UTF-8 surrogate carry, record → VT synthesis,
//! mode-bit arithmetic) lives in [`console_translate`](super::console_translate) so it is unit-
//! tested on every platform, not only on the Windows CI host.

// SAFETY SCOPE: this module is the crate's only `unsafe`. Every `unsafe` block wraps a single
// documented `windows-sys` FFI call (or a `std::mem::zeroed` for a plain out-param struct / a union
// field read whose active variant the surrounding code has just checked) with a `// SAFETY:`
// comment stating the contract verified at the call site. The crate lint is `unsafe_code = "deny"`
// (not `forbid`) precisely so this one `#[cfg(windows)]` module can opt in; the Unix and pure
// layers carry no `unsafe`. See the Cargo.toml lint note and ADR 0021.
#![allow(
    unsafe_code,
    reason = "the Windows console device is FFI-only; every Win32 console entry point is an unsafe \
              extern \"system\" call with no safe wrapper in the dependency tree"
)]

use std::collections::VecDeque;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, WriteFile,
};
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, GetConsoleMode, GetConsoleOutputCP, GetConsoleScreenBufferInfo,
    INPUT_RECORD, KEY_EVENT, MOUSE_EVENT, ReadConsoleInputW, SetConsoleMode, SetConsoleOutputCP,
    WINDOW_BUFFER_SIZE_EVENT,
};

use super::console_translate::{self as translate, ConsoleMouse, SurrogateCarry};
use crate::terminal::{DeviceMode, Error, Result, TerminalDevice, TerminalSize};

/// The UTF-8 output codepage set on the console for the raw session's lifetime.
///
/// With codepage 65001, [`write_all`](Terminal::write_all) hands UTF-8 straight to `WriteFile` with
/// no transcoding; the captured original codepage is restored in cooked mode.
const UTF8_CODEPAGE: u32 = 65001;

/// The virtual-key code for the Alt/Menu key (`VK_MENU`).
///
/// Named here rather than pulled from `windows-sys` (which would need the `Win32_UI` feature) to
/// keep the dependency surface minimal; it is only used to recognize conhost's Alt+numpad quirk.
const VK_MENU: u16 = 0x12;

/// The per-record cap on repeat-count expansion, bounding the memory one `KEY_EVENT` can produce.
///
/// A `KEY_EVENT_RECORD` can in principle claim a `wRepeatCount` of up to 65535; expanding that many
/// UTF-16 units per record is unbounded pressure the caller never asked for, so the expansion is
/// capped here. Interactive autorepeat never approaches this.
const MAX_UNITS_PER_RECORD: usize = 1024;

/// The number of `INPUT_RECORD`s drained from the console per `ReadConsoleInputW` call.
///
/// One `read` translates at most this many records, so the bytes a single read can buffer are
/// bounded by `RECORD_BATCH * MAX_UNITS_PER_RECORD * 3` (worst-case UTF-8 width) — a compile-time
/// constant, which is what keeps the pending buffer from growing without bound.
const RECORD_BATCH: usize = 128;

/// A live Windows console terminal device.
///
/// Owns the console input (`CONIN$`) and output (`CONOUT$`) handles opened by name through
/// `CreateFileW`, mirroring the [`unix::Terminal`](super::unix) surface so
/// [`TerminalSession`](crate::TerminalSession) composes over it unchanged. The console modes and
/// output codepage captured at [`open`](Self::open) are restored in cooked mode and on drop.
#[derive(Debug)]
pub struct Terminal {
    /// The console input handle (`CONIN$`), owned and closed on drop.
    input: OwnedHandle,
    /// The console output handle (`CONOUT$`), owned and closed on drop.
    output: OwnedHandle,
    /// The console input mode captured at open, restored as cooked mode on teardown.
    original_input_mode: u32,
    /// The console output mode captured at open, restored on teardown.
    original_output_mode: u32,
    /// The console output codepage captured at open, restored on teardown.
    ///
    /// The codepage is console-global state, so raw mode's switch to UTF-8 (codepage 65001) is
    /// undone alongside the mode bits — otherwise a program that entered raw mode would leave the
    /// whole console on 65001 after exit (the FM-W4 restore discipline, extended to the codepage).
    original_output_codepage: u32,
    /// A synthetic device path, kept only so this type mirrors the Unix `Terminal::path` surface.
    path: PathBuf,
    /// Translated bytes not yet handed to a caller's buffer, retained across [`read`](Self::read)
    /// calls so no byte is lost when the caller's buffer is smaller than one record batch.
    pending: VecDeque<u8>,
    /// The persistent UTF-16 surrogate carry, so an astral character split across two reads pairs
    /// up.
    carry: SurrogateCarry,
    /// The console mouse-button state from the previous `MOUSE_EVENT`, used to tell press from
    /// release when synthesizing SGR reports.
    previous_mouse_buttons: u32,
}

impl Terminal {
    /// Opens the process console by name.
    ///
    /// Opens `CONIN$` and `CONOUT$` through `CreateFileW` with `GENERIC_READ | GENERIC_WRITE` and
    /// `FILE_SHARE_READ | FILE_SHARE_WRITE`, `OPEN_EXISTING` — the Windows analogue of opening
    /// `/dev/tty`, so the device owns the console even when the process stdin/stdout are
    /// redirected. The current input mode, output mode, and output codepage are captured so
    /// cooked mode can restore exactly what was live at open.
    ///
    /// # Errors
    ///
    /// Returns [`Error::OpenTerminal`] when no console is attached (either device cannot be
    /// opened), or [`Error::GetTerminalMode`] when a captured mode or the codepage cannot be
    /// read.
    pub fn open() -> Result<Self> {
        let input = open_console_handle("CONIN$")?;
        let output = open_console_handle("CONOUT$")?;

        let original_input_mode = get_console_mode(&input)?;
        let original_output_mode = get_console_mode(&output)?;
        let original_output_codepage = get_output_codepage()?;

        Ok(Self {
            input,
            output,
            original_input_mode,
            original_output_mode,
            original_output_codepage,
            path: PathBuf::from("CONIN$"),
            pending: VecDeque::new(),
            carry: SurrogateCarry::default(),
            previous_mouse_buttons: 0,
        })
    }

    /// Opens the process console, recording `path` for surface parity with the Unix device.
    ///
    /// A console has no per-path device the way a Unix pty does: there is exactly one console per
    /// process, addressed as `CONIN$`/`CONOUT$`. The argument is accepted and recorded as
    /// [`path`](Self::path) so this type mirrors [`unix::Terminal::open_path`](super::unix), but it
    /// does not select a different device — [`open`](Self::open) opens the same console regardless.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`open`](Self::open).
    pub fn open_path(path: impl Into<PathBuf>) -> Result<Self> {
        let mut terminal = Self::open()?;
        terminal.path = path.into();
        Ok(terminal)
    }

    /// Returns the path recorded when the terminal was opened.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the current console window size in character cells.
    ///
    /// Derives the *window* dimensions (not the full scrollback buffer) from the inclusive
    /// `srWindow` rectangle of `GetConsoleScreenBufferInfo`. Following the Unix device's FM-Z2
    /// contract, a degenerate rectangle — zero or negative extent in either axis — is reported as
    /// [`Error::InvalidTerminalSize`] rather than a bogus `0`-sized measurement, so the session
    /// falls back to the `COLUMNS`/`LINES` environment.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GetTerminalSize`] when the screen-buffer info cannot be read, or
    /// [`Error::InvalidTerminalSize`] when the window rectangle is degenerate.
    pub fn size(&self) -> Result<TerminalSize> {
        let info = get_screen_buffer_info(&self.output)?;
        let window = info.srWindow;
        let (columns, rows) =
            translate::window_extent(window.Left, window.Top, window.Right, window.Bottom);

        let reported_columns = u16::try_from(columns.max(0)).unwrap_or(u16::MAX);
        let reported_rows = u16::try_from(rows.max(0)).unwrap_or(u16::MAX);
        if translate::is_degenerate(columns, rows) {
            return Err(Error::InvalidTerminalSize {
                columns: reported_columns,
                rows: reported_rows,
            });
        }
        Ok(TerminalSize::new(reported_columns, reported_rows))
    }

    /// Enters raw mode: VT input, VT output, and the UTF-8 codepage.
    ///
    /// Clears line-input/echo/processed-input and sets VT input plus the window/mouse-record and
    /// extended-flags bits on input; sets processing, wrap, VT processing, and the newline-fixup
    /// opt-out on output; and switches the output codepage to UTF-8 (65001). The output mode is
    /// read back afterward: if `ENABLE_VIRTUAL_TERMINAL_PROCESSING` did not stick, every
    /// already-applied change is rolled back and a typed error is returned — VT is required,
    /// with no degraded path (ADR 0022 §2).
    ///
    /// # Errors
    ///
    /// Returns [`Error::SetTerminalMode`] when a mode or the codepage cannot be applied, or when
    /// the console output does not support VT processing. On any failure the captured modes are
    /// restored best-effort before returning.
    pub fn set_raw_mode(&self) -> Result<()> {
        let raw_input = translate::raw_input_mode(self.original_input_mode);
        let raw_output = translate::raw_output_mode(self.original_output_mode);

        set_console_mode(&self.input, raw_input)?;

        if let Err(error) = set_console_mode(&self.output, raw_output) {
            // Only the input mode was changed; restore it before surfacing the failure.
            let _ = set_console_mode(&self.input, self.original_input_mode);
            return Err(error);
        }
        if let Err(error) = set_output_codepage(UTF8_CODEPAGE) {
            self.restore_all();
            return Err(error);
        }

        // Read the output mode back: some hosts accept the call but silently drop the VT bit, which
        // ADR 0022 §2 forbids relying on. A failed readback or a missing VT bit rolls everything
        // back.
        let readback = match get_console_mode(&self.output) {
            Ok(mode) => mode,
            Err(error) => {
                self.restore_all();
                return Err(error);
            }
        };
        if !translate::output_has_vt(readback) {
            self.restore_all();
            return Err(Error::set_terminal_mode(io::Error::new(
                io::ErrorKind::Unsupported,
                "console output does not support ENABLE_VIRTUAL_TERMINAL_PROCESSING",
            )));
        }
        Ok(())
    }

    /// Restores cooked mode: the input mode, output mode, and codepage captured at open.
    ///
    /// Restoring the *captured* values rather than synthesized defaults is what makes console
    /// restore a solved problem rather than the leak crossterm ships (it leaves VT/mouse input bits
    /// set). Every restore is attempted even if an earlier one fails — a half-restored console is
    /// worse than a fully-attempted one — and the first error, if any, is reported.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SetTerminalMode`] carrying the first restore failure; the remaining
    /// restores are still attempted before it is returned.
    pub fn set_cooked_mode(&self) -> Result<()> {
        let mut first_error = None;
        if let Err(error) = set_console_mode(&self.input, self.original_input_mode) {
            first_error.get_or_insert(error);
        }
        if let Err(error) = set_console_mode(&self.output, self.original_output_mode) {
            first_error.get_or_insert(error);
        }
        if let Err(error) = set_output_codepage(self.original_output_codepage) {
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Best-effort restore of every captured value, used to roll back a partial raw-mode entry.
    fn restore_all(&self) {
        let _ = set_console_mode(&self.input, self.original_input_mode);
        let _ = set_console_mode(&self.output, self.original_output_mode);
        let _ = set_output_codepage(self.original_output_codepage);
    }

    /// Writes all bytes to the console output via `WriteFile`, looping over partial writes.
    ///
    /// The bytes are written verbatim: the output codepage is UTF-8 (65001) in raw mode, so UTF-8
    /// text and VT command bytes pass straight through with no transcoding. A short write is
    /// retried from the unwritten offset until every byte is consumed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WriteTerminal`] when a write fails or makes no progress.
    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            let chunk = &bytes[offset..];
            let length = u32::try_from(chunk.len()).unwrap_or(u32::MAX);
            let mut written: u32 = 0;
            // SAFETY: `raw` borrows the live owned output handle; `chunk` is readable for `length`
            // bytes; `written` is a live out-param; the OVERLAPPED pointer is null, valid for the
            // synchronous console handle.
            let ok = unsafe {
                WriteFile(
                    raw(&self.output),
                    chunk.as_ptr(),
                    length,
                    &raw mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(Error::write_terminal(io::Error::last_os_error()));
            }
            if written == 0 {
                return Err(Error::write_terminal(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "WriteFile made no progress on console output",
                )));
            }
            offset += written as usize;
        }
        Ok(())
    }

    /// Reads translated console input bytes into `buffer`, blocking until some are available.
    ///
    /// Any bytes retained from a previous read are drained first. Otherwise `ReadConsoleInputW`
    /// blocks for a batch of input records, which are translated into the VT byte stream the
    /// platform-neutral decoder expects:
    ///
    /// - `KEY_EVENT` records with a non-zero character contribute their UTF-16 unit (repeated
    ///   `wRepeatCount` times, capped), reassembled into UTF-8 across surrogate halves. Key-up and
    ///   zero-character records are dropped, except conhost's Alt+numpad quirk where a `VK_MENU`
    ///   key-up carries the composed character.
    /// - `WINDOW_BUFFER_SIZE_EVENT` records synthesize the in-band resize report `CSI 48 ; h ; w t`
    ///   from the current window rectangle (not the record's buffer size).
    /// - `MOUSE_EVENT` records (conhost; Windows Terminal sends SGR bytes directly) synthesize SGR
    ///   mouse reports.
    /// - `FOCUS_EVENT`/`MENU_EVENT` are internal-use per Microsoft docs and skipped.
    ///
    /// Returns `Ok(0)` only when the console reports no records (a broken console, the
    /// EOF-equivalent) after any dangling surrogate has been flushed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReadTerminal`] when `ReadConsoleInputW` fails.
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if !self.pending.is_empty() {
                return Ok(self.drain_pending(buffer));
            }

            // SAFETY: a zeroed INPUT_RECORD array is a valid initial value; ReadConsoleInputW
            // overwrites the `count`-length prefix it fills, and only that prefix is read
            // afterward.
            let mut records: [INPUT_RECORD; RECORD_BATCH] = unsafe { std::mem::zeroed() };
            let count = read_input_records(&self.input, &mut records)?;
            if count == 0 {
                // Broken console (EOF-equivalent): flush a dangling high surrogate once as U+FFFD
                // so it is not lost, then report EOF on the next iteration's empty
                // pending.
                let mut tail = Vec::new();
                self.carry.flush(&mut tail);
                if tail.is_empty() {
                    return Ok(0);
                }
                self.pending.extend(tail);
                continue;
            }

            let mut out = Vec::new();
            for record in &records[..count] {
                self.translate_record(record, &mut out);
            }
            if out.is_empty() {
                // The batch held only dropped records (key-up chatter, focus/menu events); keep
                // blocking rather than returning a premature `Ok(0)` the session would read as EOF.
                continue;
            }
            self.pending.extend(out);
        }
    }

    /// Copies as many pending bytes as fit into `buffer`, retaining the remainder.
    fn drain_pending(&mut self, buffer: &mut [u8]) -> usize {
        let count = buffer.len().min(self.pending.len());
        for (slot, byte) in buffer.iter_mut().zip(self.pending.drain(..count)) {
            *slot = byte;
        }
        count
    }

    /// Translates one input record into VT bytes, appending to `out`.
    fn translate_record(&mut self, record: &INPUT_RECORD, out: &mut Vec<u8>) {
        match u32::from(record.EventType) {
            KEY_EVENT => self.translate_key(record, out),
            MOUSE_EVENT => {
                // SAFETY: EventType == MOUSE_EVENT, so `MouseEvent` is the active union variant.
                let mouse = unsafe { record.Event.MouseEvent };
                let input = ConsoleMouse {
                    button_state: mouse.dwButtonState,
                    event_flags: mouse.dwEventFlags,
                    x: mouse.dwMousePosition.X,
                    y: mouse.dwMousePosition.Y,
                };
                self.previous_mouse_buttons =
                    translate::translate_mouse(input, self.previous_mouse_buttons, out);
            }
            WINDOW_BUFFER_SIZE_EVENT => self.synthesize_resize(out),
            // FOCUS_EVENT and MENU_EVENT are documented internal-use; any other type is unknown.
            // All are dropped silently.
            _ => {}
        }
    }

    /// Translates a `KEY_EVENT` record's character into UTF-8 bytes, appending to `out`.
    fn translate_key(&mut self, record: &INPUT_RECORD, out: &mut Vec<u8>) {
        // SAFETY: EventType == KEY_EVENT (checked by the caller), so `KeyEvent` is the active union
        // variant.
        let key = unsafe { record.Event.KeyEvent };
        // SAFETY: KEY_EVENT_RECORD_0 read as its `UnicodeChar` (u16) member; every bit pattern is a
        // valid u16, and the console fills the UTF-16 form under VT input.
        let unit = unsafe { key.uChar.UnicodeChar };
        if unit == 0 {
            return; // Modifier/keypad chatter and key events that carry no character.
        }

        let key_down = key.bKeyDown != 0;
        // conhost delivers an Alt+numpad composed character on the key-UP of VK_MENU; that one
        // key-up carries a real character and must be translated. Every other key-up is dropped.
        let alt_numpad_release = !key_down && key.wVirtualKeyCode == VK_MENU;
        if !key_down && !alt_numpad_release {
            return;
        }

        let repeat = usize::from(key.wRepeatCount).min(MAX_UNITS_PER_RECORD);
        for _ in 0..repeat {
            self.carry.push(unit, out);
        }
    }

    /// Synthesizes an in-band resize report from the current window rectangle, appending to `out`.
    fn synthesize_resize(&self, out: &mut Vec<u8>) {
        // The record's dwSize is the scrollback buffer, not the visible window; query the live
        // rect.
        let Ok(info) = get_screen_buffer_info(&self.output) else {
            return; // A transient failure during a resize burst is not worth failing the read for.
        };
        let window = info.srWindow;
        let (columns, rows) =
            translate::window_extent(window.Left, window.Top, window.Right, window.Bottom);
        if translate::is_degenerate(columns, rows) {
            return; // Skip a transient degenerate rectangle rather than report a 0x0 resize.
        }
        let reported_columns = u16::try_from(columns).unwrap_or(u16::MAX);
        let reported_rows = u16::try_from(rows).unwrap_or(u16::MAX);
        translate::format_resize_report(reported_rows, reported_columns, out);
    }

    /// Flushes buffered console output.
    ///
    /// `WriteFile` writes straight to the console with no library-side buffering, so there is
    /// nothing to flush; this is a success, kept for surface parity with the Unix device.
    ///
    /// # Errors
    ///
    /// Never returns an error.
    pub fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Returns the raw Win32 `HANDLE` behind an owned console handle, for a single FFI call.
///
/// `std::os::windows::raw::HANDLE` and `windows_sys`'s `HANDLE` are both `*mut c_void`, so this is
/// a type-level reinterpretation with no ownership transfer — the [`OwnedHandle`] still owns and
/// will still close the handle.
fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle()
}

/// Opens one console device by name (`CONIN$` or `CONOUT$`) via `CreateFileW`.
fn open_console_handle(name: &str) -> Result<OwnedHandle> {
    // A null-terminated UTF-16 copy of the device name for the `PCWSTR` argument.
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a null-terminated UTF-16 string owned for the duration of the call; the
    // access, share, disposition, and flag arguments are plain values; the security-attributes and
    // template-file pointers are null, which CreateFileW permits. The handle is validated below.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(Error::open_terminal(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no console device available for {name}"),
        )));
    }
    // SAFETY: CreateFileW returned a valid handle (checked non-invalid and non-null above);
    // adopting it transfers sole ownership so it is closed exactly once, when the OwnedHandle
    // drops.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// Reads a console mode via `GetConsoleMode`.
fn get_console_mode(handle: &OwnedHandle) -> Result<u32> {
    let mut mode: u32 = 0;
    // SAFETY: `raw` borrows a live owned handle; `mode` is a live out-param.
    let ok = unsafe { GetConsoleMode(raw(handle), &raw mut mode) };
    if ok == 0 {
        return Err(Error::get_terminal_mode(io::Error::last_os_error()));
    }
    Ok(mode)
}

/// Writes a console mode via `SetConsoleMode`.
fn set_console_mode(handle: &OwnedHandle, mode: u32) -> Result<()> {
    // SAFETY: `raw` borrows a live owned handle; `mode` is a plain value.
    let ok = unsafe { SetConsoleMode(raw(handle), mode) };
    if ok == 0 {
        return Err(Error::set_terminal_mode(io::Error::last_os_error()));
    }
    Ok(())
}

/// Reads the console output codepage via `GetConsoleOutputCP`.
fn get_output_codepage() -> Result<u32> {
    // SAFETY: GetConsoleOutputCP takes no arguments and reads process-global console state.
    let codepage = unsafe { GetConsoleOutputCP() };
    if codepage == 0 {
        return Err(Error::get_terminal_mode(io::Error::last_os_error()));
    }
    Ok(codepage)
}

/// Sets the console output codepage via `SetConsoleOutputCP`.
fn set_output_codepage(codepage: u32) -> Result<()> {
    // SAFETY: SetConsoleOutputCP takes a codepage id and sets process-global console state.
    let ok = unsafe { SetConsoleOutputCP(codepage) };
    if ok == 0 {
        return Err(Error::set_terminal_mode(io::Error::last_os_error()));
    }
    Ok(())
}

/// Reads console screen-buffer info via `GetConsoleScreenBufferInfo`.
fn get_screen_buffer_info(handle: &OwnedHandle) -> Result<CONSOLE_SCREEN_BUFFER_INFO> {
    // SAFETY: a zeroed CONSOLE_SCREEN_BUFFER_INFO is a valid initial value the call overwrites.
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: `raw` borrows a live owned handle; `info` is a live, fully-owned out-param sized
    // exactly to the struct the API writes.
    let ok = unsafe { GetConsoleScreenBufferInfo(raw(handle), &raw mut info) };
    if ok == 0 {
        return Err(Error::get_terminal_size(io::Error::last_os_error()));
    }
    Ok(info)
}

/// Reads a batch of input records via `ReadConsoleInputW`, returning the count filled.
fn read_input_records(handle: &OwnedHandle, records: &mut [INPUT_RECORD]) -> Result<usize> {
    let capacity = u32::try_from(records.len()).unwrap_or(u32::MAX);
    let mut count: u32 = 0;
    // SAFETY: `raw` borrows a live owned handle; `records` is writable for `capacity` entries;
    // `count` is a live out-param.
    let ok =
        unsafe { ReadConsoleInputW(raw(handle), records.as_mut_ptr(), capacity, &raw mut count) };
    if ok == 0 {
        return Err(Error::read_terminal(io::Error::last_os_error()));
    }
    Ok(count as usize)
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
    // never provides it — correct, because a console input HANDLE is not a pollable fd and could
    // not be registered with `AsyncFd`. The readiness seam Windows needs (a cancellable wait on
    // the input handle, ADR 0022 §4) is a later milestone (MW-2), not a `TerminalDevice`
    // method.
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Last line of defense, mirroring the Unix device: restore the captured modes so a program
        // that skipped orderly shutdown does not leave the console in raw mode / on codepage 65001.
        // Drop cannot report errors, so the result is discarded; the owned handles close afterward.
        let _ = self.set_cooked_mode();
    }
}

#[cfg(test)]
mod live_tests {
    //! Live-console tests, run only on the Windows CI host (a real console is required).
    //!
    //! These cannot run on the Unix development machine; the read path's logic coverage comes from
    //! the platform-neutral tests in [`console_translate`](super::super::console_translate).
    //! Reading real input is not exercised here — no one types on CI — per the MW-1 spec.

    use std::sync::{Mutex, Once};

    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    };

    use super::*;

    /// Serializes console access: the modes and codepage are process-global, so two tests entering
    /// raw mode at once would corrupt each other's captured/restored state.
    static CONSOLE: Mutex<()> = Mutex::new(());
    /// Ensures a console is attached exactly once for the whole test binary.
    static ALLOC: Once = Once::new();

    /// Attaches a console if the test process has none, then takes the serialization lock.
    fn console_guard() -> std::sync::MutexGuard<'static, ()> {
        ALLOC.call_once(|| {
            // SAFETY: AllocConsole takes no arguments. It fails harmlessly when a console already
            // exists (the CI host may attach one), so its result is intentionally ignored.
            let _ = unsafe { windows_sys::Win32::System::Console::AllocConsole() };
        });
        CONSOLE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn open_captures_and_restores_modes_round_trip() {
        let _guard = console_guard();
        let terminal = Terminal::open().expect("open console");

        let original_input = terminal.original_input_mode;
        let original_output = terminal.original_output_mode;
        let original_codepage = terminal.original_output_codepage;

        terminal.set_raw_mode().expect("enter raw mode");

        // Raw mode sets VT on both input and output; confirm via a fresh readback.
        let raw_input = get_console_mode(&terminal.input).expect("read input mode");
        let raw_output = get_console_mode(&terminal.output).expect("read output mode");
        assert_ne!(
            raw_input & ENABLE_VIRTUAL_TERMINAL_INPUT,
            0,
            "VT input bit set"
        );
        assert_ne!(
            raw_output & ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            0,
            "VT output bit set"
        );
        assert_eq!(get_output_codepage().expect("read codepage"), UTF8_CODEPAGE);

        terminal.set_cooked_mode().expect("restore cooked mode");

        assert_eq!(
            get_console_mode(&terminal.input).expect("read input mode"),
            original_input
        );
        assert_eq!(
            get_console_mode(&terminal.output).expect("read output mode"),
            original_output
        );
        assert_eq!(
            get_output_codepage().expect("read codepage"),
            original_codepage
        );
    }

    #[test]
    fn size_reports_a_sane_window() {
        let _guard = console_guard();
        let terminal = Terminal::open().expect("open console");
        // A degenerate size is a typed error, never a panic; a real console reports positive
        // extents.
        if let Ok(size) = terminal.size() {
            assert!(size.columns() > 0, "columns should be positive");
            assert!(size.rows() > 0, "rows should be positive");
        }
    }

    #[test]
    fn write_all_survives_a_large_buffer() {
        let _guard = console_guard();
        let mut terminal = Terminal::open().expect("open console");
        // Larger than the 64 KiB console write ceiling, to exercise the partial-write loop.
        let payload = vec![b'.'; 100 * 1024];
        terminal.write_all(&payload).expect("write large buffer");
        terminal
            .write_all(b"")
            .expect("empty write is a no-op success");
    }
}
