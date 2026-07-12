//! Console input reader — the shared record-to-VT translation core (ADR 0022 §3, milestone MW-2).
//!
//! # Why this file exists
//!
//! Both the synchronous device ([`windows::Terminal`](super::windows)) and the async readiness
//! worker (`src/tokio_session/readiness.rs`) turn a batch of `ReadConsoleInputW` records into the
//! VT byte stream the platform-neutral decoder parses. The *blocking read* differs between them —
//! the device blocks in `ReadConsoleInputW`; the worker only reads after a cancellable wait says
//! records are pending — but everything after the read is identical: the UTF-16 surrogate carry,
//! the resize synthesis, the mouse-state diff, and the pending-buffer drain. That common core lives
//! here as [`ConsoleInputReader`] so the two callers share one translator instead of forking it.
//!
//! This is a pure refactor of the sealed MW-1 device: the logic and its observable behavior are
//! unchanged; it simply moved so a second owner (the worker, over a *duplicated* input handle) can
//! reuse it. The console-free arithmetic it calls (UTF-16 → UTF-8, record → VT synthesis, mode
//! bits) stays in [`console_translate`](super::console_translate), unit-tested on every platform.
//!
//! The module is `#[cfg(windows)]`: it names `windows-sys` record types directly.

// SAFETY SCOPE: this module reads the active variant of an `INPUT_RECORD` union after checking its
// `EventType`, and zeroes an `INPUT_RECORD` batch for `ReadConsoleInputW` to overwrite. Every
// `unsafe` block wraps one such access with a `// SAFETY:` comment naming the checked precondition.
// The crate lint is `unsafe_code = "deny"` (not `forbid`) so this `#[cfg(windows)]` module can opt
// in; the Unix and pure layers carry no `unsafe`. See ADR 0021.
#![allow(
    unsafe_code,
    reason = "console input translation reads `windows-sys` record unions and drains a record \
              batch the FFI fills; both are FFI-shaped operations with no safe wrapper"
)]

use std::collections::VecDeque;
#[cfg(feature = "tokio")]
use std::os::windows::io::BorrowedHandle;
use std::os::windows::io::OwnedHandle;

use windows_sys::Win32::System::Console::{
    INPUT_RECORD, KEY_EVENT, MOUSE_EVENT, ReadConsoleInputW, WINDOW_BUFFER_SIZE_EVENT,
};

use super::console_translate::{self as translate, ConsoleMouse, SurrogateCarry};
use super::windows::get_screen_buffer_info;
use crate::terminal::{Error, Result};

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
/// One read translates at most this many records, so the bytes a single read can buffer are bounded
/// by `RECORD_BATCH * MAX_UNITS_PER_RECORD * 3` (worst-case UTF-8 width) — a compile-time constant,
/// which is what keeps the pending buffer from growing without bound.
pub(crate) const RECORD_BATCH: usize = 128;

/// The console input and output handles a device exposes for the readiness worker.
///
/// Returned by [`TerminalDevice::as_console_handles`](crate::TerminalDevice::as_console_handles),
/// this is the Windows analogue of a borrowed fd — the public counterpart to the `BorrowedFd` that
/// [`as_fd`](crate::TerminalDevice::as_fd) returns on Unix: two borrowed console handles the async
/// readiness transport duplicates at construction (the input for the worker's waited reads, the
/// output for its writes and resize measurements). The borrows are `BorrowedHandle`s, keeping the
/// raw `HANDLE` out of the seam. It exists only with the `tokio` feature — the async readiness
/// worker is its sole consumer.
#[cfg(feature = "tokio")]
#[derive(Clone, Copy, Debug)]
pub struct ConsoleHandles<'a> {
    /// The console input handle (`CONIN$`), read with `ReadConsoleInputW`.
    pub input: BorrowedHandle<'a>,
    /// The console output handle (`CONOUT$`), written with `WriteFile` and measured for resize.
    pub output: BorrowedHandle<'a>,
}

/// Reads a batch of input records via `ReadConsoleInputW`, returning the count filled.
///
/// Shared by the synchronous device (which calls it inside a blocking read) and the async worker
/// (which calls it only after a wait reports the input handle signalled, so the call returns
/// immediately). It performs no translation — it hands the raw records to [`ConsoleInputReader`].
///
/// # Errors
///
/// Returns [`Error::ReadTerminal`] when `ReadConsoleInputW` fails.
pub(crate) fn read_input_records(
    handle: &OwnedHandle,
    records: &mut [INPUT_RECORD],
) -> Result<usize> {
    use std::os::windows::io::AsRawHandle as _;

    let capacity = u32::try_from(records.len()).unwrap_or(u32::MAX);
    let mut count: u32 = 0;
    // SAFETY: `handle` is a live owned console handle; `records` is writable for `capacity`
    // entries; `count` is a live out-param.
    let ok = unsafe {
        ReadConsoleInputW(
            handle.as_raw_handle(),
            records.as_mut_ptr(),
            capacity,
            &raw mut count,
        )
    };
    if ok == 0 {
        return Err(Error::read_terminal(std::io::Error::last_os_error()));
    }
    Ok(count as usize)
}

/// The console-free state that turns `ReadConsoleInputW` records into VT bytes.
///
/// Owns everything the translation carries across reads: the UTF-16 surrogate half awaiting its
/// partner, the previous mouse-button state (to tell press from release), and the pending byte
/// buffer holding translated bytes not yet handed to a caller. It is deliberately free of any
/// console handle — [`translate_records`](Self::translate_records) takes the output handle it needs
/// for resize measurement as an argument — so two owners (the synchronous device and the async
/// worker, each over a different input handle) can each hold their own reader.
#[derive(Debug, Default)]
pub(crate) struct ConsoleInputReader {
    /// Translated bytes not yet handed to a caller's buffer, retained across reads so no byte is
    /// lost when the caller's buffer is smaller than one record batch.
    pending: VecDeque<u8>,
    /// The persistent UTF-16 surrogate carry, so an astral character split across two reads pairs
    /// up.
    carry: SurrogateCarry,
    /// The console mouse-button state from the previous `MOUSE_EVENT`, used to tell press from
    /// release when synthesizing SGR reports.
    previous_mouse_buttons: u32,
}

impl ConsoleInputReader {
    /// Creates a reader with an empty pending buffer, no carry, and no held mouse buttons.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Returns whether the pending buffer holds no bytes.
    pub(crate) fn is_pending_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Translates a slice of input records into VT bytes appended to the pending buffer.
    ///
    /// `output` is the console output handle the resize path measures the current window rectangle
    /// with (`WINDOW_BUFFER_SIZE_EVENT` records carry the scrollback size, not the visible window).
    /// Key-up chatter and zero-character records contribute nothing; the batch producing no bytes
    /// leaves the pending buffer as it was, which the caller reads as "keep waiting".
    pub(crate) fn translate_records(&mut self, records: &[INPUT_RECORD], output: &OwnedHandle) {
        let mut out = Vec::new();
        for record in records {
            self.translate_record(record, output, &mut out);
        }
        self.pending.extend(out);
    }

    /// Flushes a dangling high surrogate into the pending buffer as `U+FFFD`, used at end of input.
    ///
    /// Called when the console reports no more records (a broken console, the EOF-equivalent): a
    /// high surrogate that never received its low half is emitted as the replacement character
    /// rather than dropped silently.
    pub(crate) fn flush_carry(&mut self) {
        let mut tail = Vec::new();
        self.carry.flush(&mut tail);
        self.pending.extend(tail);
    }

    /// Copies as many pending bytes as fit into `buffer`, retaining the remainder, returning the
    /// count.
    pub(crate) fn drain_pending(&mut self, buffer: &mut [u8]) -> usize {
        let count = buffer.len().min(self.pending.len());
        for (slot, byte) in buffer.iter_mut().zip(self.pending.drain(..count)) {
            *slot = byte;
        }
        count
    }

    /// Drains every pending byte into a fresh vector, leaving the buffer empty.
    ///
    /// The async worker takes the whole batch at once to hand to its channel, where the transport's
    /// leftover buffer — not this pending buffer — absorbs a short caller read. Only the Tokio
    /// worker calls this (the synchronous device drains into the caller's buffer instead), so it is
    /// gated to the `tokio` feature to stay out of a default Windows build.
    #[cfg(any(feature = "tokio", test))]
    pub(crate) fn take_pending(&mut self) -> Vec<u8> {
        self.pending.drain(..).collect()
    }

    /// Translates one input record into VT bytes, appending to `out`.
    fn translate_record(&mut self, record: &INPUT_RECORD, output: &OwnedHandle, out: &mut Vec<u8>) {
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
            WINDOW_BUFFER_SIZE_EVENT => synthesize_resize(output, out),
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
}

/// Synthesizes an in-band resize report from the current window rectangle, appending to `out`.
///
/// Reads the live window rect from `output` — never the record's `dwSize`, which is the scrollback
/// buffer, not the visible window. A transient failure or degenerate rectangle during a resize
/// burst is skipped rather than reported as a bogus `0x0` resize.
fn synthesize_resize(output: &OwnedHandle, out: &mut Vec<u8>) {
    let Ok(info) = get_screen_buffer_info(output) else {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_pending_copies_up_to_the_buffer_length_and_retains_the_rest() {
        // A chunk longer than the caller buffer spans two reads: the reader hands out what fits and
        // keeps the remainder for the next drain, so no byte is lost across a short read.
        let mut reader = ConsoleInputReader::new();
        reader.pending.extend(*b"abcdef");

        let mut first = [0u8; 4];
        assert_eq!(reader.drain_pending(&mut first), 4);
        assert_eq!(&first, b"abcd");
        assert!(
            !reader.is_pending_empty(),
            "the tail is retained for the next read"
        );

        let mut second = [0u8; 4];
        assert_eq!(
            reader.drain_pending(&mut second),
            2,
            "only the tail remains"
        );
        assert_eq!(&second[..2], b"ef");
        assert!(
            reader.is_pending_empty(),
            "the buffer is drained after the tail read"
        );
    }

    #[test]
    fn take_pending_drains_the_whole_buffer() {
        let mut reader = ConsoleInputReader::new();
        reader.pending.extend(*b"hello");
        assert_eq!(reader.take_pending(), b"hello");
        assert!(reader.is_pending_empty());
        assert_eq!(
            reader.take_pending(),
            Vec::<u8>::new(),
            "a second take yields nothing"
        );
    }
}
