//! Pure translation logic for the Windows console device — no live console required.
//!
//! # Why this file exists
//!
//! The device in [`windows`](super::windows) can only run on Windows, and its live behavior can
//! only be exercised on a real console (the `windows-latest` CI job). Every piece of logic that
//! does *not* need a console handle lives here instead, as plain functions with no `windows-sys`
//! dependency, so it compiles and is unit-tested on **every** platform — including the Unix
//! development machine where the rest of the Windows device cannot even be built. This is the
//! locally executable coverage for the read path; the FFI-only glue in [`windows`](super::windows)
//! is what remains for CI and the reserved Windows VM.
//!
//! The module is compiled under `#[cfg(any(test, windows))]`: it is real code in the Windows build,
//! and test-only code everywhere else, so a Unix release build never carries it.
//!
//! # What lives here
//!
//! | concern                     | entry point                                   |
//! | --------------------------- | --------------------------------------------- |
//! | raw/cooked mode-bit math    | [`raw_input_mode`], [`raw_output_mode`], [`output_has_vt`] |
//! | UTF-16 → UTF-8 with carry   | [`SurrogateCarry`]                            |
//! | mouse record → SGR bytes    | [`translate_mouse`]                           |
//! | resize record → VT report   | [`format_resize_report`]                      |
//! | window-rect → cell extent   | [`window_extent`], [`is_degenerate`]          |
//!
//! The console-mode and event bit values are redeclared here as plain `u32` constants rather than
//! imported from `windows-sys` (which does not exist off Windows). A `#[cfg(windows)]` test
//! (`local_bit_values_match_windows_sys`) asserts each one equals the real header value, so the
//! portable copies can never silently drift from the Win32 definitions they mirror.

use std::io::Write as _;

// ---------------------------------------------------------------------------
// Console-mode bit values (mirrors `windows_sys::Win32::System::Console`).
// ---------------------------------------------------------------------------

/// `ENABLE_PROCESSED_INPUT`: Ctrl+C is handled by the system rather than delivered as input.
const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
/// `ENABLE_LINE_INPUT`: reads return only when a full line is typed (canonical mode).
const ENABLE_LINE_INPUT: u32 = 0x0002;
/// `ENABLE_ECHO_INPUT`: typed characters are echoed to the screen by the console host.
const ENABLE_ECHO_INPUT: u32 = 0x0004;
/// `ENABLE_WINDOW_INPUT`: `WINDOW_BUFFER_SIZE_EVENT` records are delivered on resize.
const ENABLE_WINDOW_INPUT: u32 = 0x0008;
/// `ENABLE_MOUSE_INPUT`: `MOUSE_EVENT` records are delivered for pointer activity.
const ENABLE_MOUSE_INPUT: u32 = 0x0010;
/// `ENABLE_EXTENDED_FLAGS`: required for the other enhancement bits to take effect together.
const ENABLE_EXTENDED_FLAGS: u32 = 0x0080;
/// `ENABLE_VIRTUAL_TERMINAL_INPUT`: the host re-encodes keys as xterm-style VT byte sequences.
const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

/// `ENABLE_PROCESSED_OUTPUT`: the host interprets control characters such as backspace and bell.
const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
/// `ENABLE_WRAP_AT_EOL_OUTPUT`: writing past the last column wraps to the next line.
const ENABLE_WRAP_AT_EOL_OUTPUT: u32 = 0x0002;
/// `ENABLE_VIRTUAL_TERMINAL_PROCESSING`: the host parses VT escape sequences in written output.
const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
/// `DISABLE_NEWLINE_AUTO_RETURN`: a bare `\n` moves down without an implicit carriage return, so
/// cursor-addressing sequences are not corrupted by newline fixups.
const DISABLE_NEWLINE_AUTO_RETURN: u32 = 0x0008;

/// The input bits raw mode clears: line buffering, echo, and system input processing.
const RAW_INPUT_CLEAR: u32 = ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT;
/// The input bits raw mode sets: VT input plus the window/mouse record and extended-flags bits.
const RAW_INPUT_SET: u32 = ENABLE_VIRTUAL_TERMINAL_INPUT
    | ENABLE_WINDOW_INPUT
    | ENABLE_MOUSE_INPUT
    | ENABLE_EXTENDED_FLAGS;
/// The output bits raw mode sets: processing, wrap, VT processing, and the newline fixup opt-out.
const RAW_OUTPUT_SET: u32 = ENABLE_PROCESSED_OUTPUT
    | ENABLE_WRAP_AT_EOL_OUTPUT
    | ENABLE_VIRTUAL_TERMINAL_PROCESSING
    | DISABLE_NEWLINE_AUTO_RETURN;

/// Computes the raw-mode console **input** mode from the mode captured at open.
///
/// Raw mode is a delta on the captured value, not a fixed constant: bits the host set that raw mode
/// does not care about (for example insert or quick-edit mode) are preserved so cooked-mode restore
/// has less to undo. Line input, echo, and processed input are cleared; VT input, window/mouse
/// records, and extended flags are set. This is the Windows analogue of `termios::make_raw`.
pub(super) const fn raw_input_mode(captured: u32) -> u32 {
    (captured & !RAW_INPUT_CLEAR) | RAW_INPUT_SET
}

/// Computes the raw-mode console **output** mode from the mode captured at open.
///
/// The output side is purely additive: VT processing (ADR 0022 §2 makes this mandatory), plus the
/// processing, wrap, and newline-fixup-opt-out bits, are OR-ed onto whatever the host already had.
pub(super) const fn raw_output_mode(captured: u32) -> u32 {
    captured | RAW_OUTPUT_SET
}

/// Returns whether a console output mode has `ENABLE_VIRTUAL_TERMINAL_PROCESSING` set.
///
/// The device sets the bit and then reads the mode back to confirm it stuck: some console hosts
/// accept the `SetConsoleMode` call but silently drop the VT bit, and ADR 0022 §2 requires VT, with
/// no degraded path. A `false` here after a set is the signal to restore and fail.
pub(super) const fn output_has_vt(mode: u32) -> bool {
    mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0
}

// ---------------------------------------------------------------------------
// UTF-16 → UTF-8 with a persistent surrogate carry.
// ---------------------------------------------------------------------------

/// The high (leading) surrogate range `U+D800..=U+DBFF`.
const HIGH_SURROGATES: std::ops::RangeInclusive<u16> = 0xD800..=0xDBFF;
/// The low (trailing) surrogate range `U+DC00..=U+DFFF`.
const LOW_SURROGATES: std::ops::RangeInclusive<u16> = 0xDC00..=0xDFFF;

/// A persistent carry for reassembling UTF-16 surrogate pairs across console reads.
///
/// With VT input enabled the console host hands the device one UTF-16 unit per `KEY_EVENT` record,
/// and an astral-plane character (emoji, less-common CJK) arrives as a high surrogate in one record
/// and a low surrogate in the next — which can straddle two separate `read` calls. This type holds
/// the pending high surrogate between calls so the pair is never split. It follows the WHATWG
/// "convert UTF-16 to a scalar value" replacement rule for malformed input:
///
/// | pending | next unit         | output                                             |
/// | ------- | ----------------- | -------------------------------------------------- |
/// | none    | BMP scalar        | the character, UTF-8 encoded                       |
/// | none    | high surrogate    | nothing yet; the surrogate is held                 |
/// | none    | low surrogate     | `U+FFFD` (an unpaired low surrogate)               |
/// | high    | low surrogate     | the combined astral character, UTF-8 encoded       |
/// | high    | anything else     | `U+FFFD` for the orphaned high, then the unit fresh |
///
/// A high surrogate left pending at end of input is flushed as `U+FFFD` by [`flush`](Self::flush).
#[derive(Debug, Default)]
pub(super) struct SurrogateCarry {
    /// A high surrogate seen but not yet paired, awaiting its low half on the next unit.
    pending_high: Option<u16>,
}

impl SurrogateCarry {
    /// Feeds one UTF-16 unit, appending zero or more UTF-8 bytes to `out`.
    pub(super) fn push(&mut self, unit: u16, out: &mut Vec<u8>) {
        if let Some(high) = self.pending_high.take() {
            if LOW_SURROGATES.contains(&unit) {
                encode_char(combine_surrogates(high, unit), out);
                return;
            }
            // The held high surrogate has no low partner: emit its replacement, then fall through
            // and process `unit` on its own (it may itself open a new pair or stand alone).
            encode_replacement(out);
        }

        if HIGH_SURROGATES.contains(&unit) {
            self.pending_high = Some(unit);
        } else if LOW_SURROGATES.contains(&unit) {
            encode_replacement(out);
        } else {
            // Not a surrogate, so it is a valid BMP scalar value.
            encode_char(char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}'), out);
        }
    }

    /// Flushes a dangling high surrogate as `U+FFFD`, used at end of input.
    ///
    /// The device calls this only when the console reports no more records (a broken console, the
    /// EOF-equivalent): a high surrogate that never received its low half is emitted as the
    /// replacement character rather than being dropped silently.
    pub(super) fn flush(&mut self, out: &mut Vec<u8>) {
        if self.pending_high.take().is_some() {
            encode_replacement(out);
        }
    }
}

/// Combines a high and low surrogate into the astral scalar value they encode.
fn combine_surrogates(high: u16, low: u16) -> char {
    let code = 0x1_0000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
    char::from_u32(code).unwrap_or('\u{FFFD}')
}

/// Appends the UTF-8 encoding of `ch` to `out`.
fn encode_char(ch: char, out: &mut Vec<u8>) {
    out.extend_from_slice(ch.encode_utf8(&mut [0u8; 4]).as_bytes());
}

/// Appends the UTF-8 encoding of the replacement character `U+FFFD` to `out`.
fn encode_replacement(out: &mut Vec<u8>) {
    encode_char('\u{FFFD}', out);
}

// ---------------------------------------------------------------------------
// Mouse record → SGR (DEC 1006) byte sequence.
// ---------------------------------------------------------------------------

/// The five console mouse-button bits of `MOUSE_EVENT_RECORD::dwButtonState`.
const BUTTON_MASK: u32 = 0x1F;
/// `FROM_LEFT_1ST_BUTTON_PRESSED`: the left (primary) button.
const BUTTON_LEFT: u32 = 0x0001;
/// `RIGHTMOST_BUTTON_PRESSED`: the right (secondary) button.
const BUTTON_RIGHT: u32 = 0x0002;
/// `FROM_LEFT_2ND_BUTTON_PRESSED`: the middle button.
const BUTTON_MIDDLE: u32 = 0x0004;
/// `FROM_LEFT_3RD_BUTTON_PRESSED`: the first extra (X1) button.
const BUTTON_X1: u32 = 0x0008;
/// `FROM_LEFT_4TH_BUTTON_PRESSED`: the second extra (X2) button.
const BUTTON_X2: u32 = 0x0010;

/// `MOUSE_MOVED`: the pointer changed cell (a drag when a button is held).
const MOUSE_MOVED: u32 = 0x0001;
/// `DOUBLE_CLICK`: the second click of a double-click on the reported button.
const DOUBLE_CLICK: u32 = 0x0002;
/// `MOUSE_WHEELED`: vertical wheel motion; the high word of `dwButtonState` is the signed delta.
const MOUSE_WHEELED: u32 = 0x0004;
/// `MOUSE_HWHEELED`: horizontal wheel motion; the high word of `dwButtonState` is the signed delta.
const MOUSE_HWHEELED: u32 = 0x0008;

/// The SGR button-code motion flag (bit 5): the report is a pointer move.
const SGR_MOTION: u16 = 0x20;
/// The SGR button-code wheel flag (bit 6): the low bits select a scroll direction.
const SGR_WHEEL: u16 = 0x40;
/// The SGR "no button" placeholder used by a bare motion report with nothing held.
const SGR_NO_BUTTON: u16 = 3;

/// The fields of a `MOUSE_EVENT_RECORD` this translator needs, decoupled from `windows-sys` so the
/// mapping is testable off Windows.
#[derive(Clone, Copy, Debug)]
pub(super) struct ConsoleMouse {
    /// `dwButtonState`: button bits in the low word, signed wheel delta in the high word.
    pub button_state: u32,
    /// `dwEventFlags`: move / double-click / wheel discriminators.
    pub event_flags: u32,
    /// `dwMousePosition.X`: zero-based cell column.
    pub x: i16,
    /// `dwMousePosition.Y`: zero-based cell row.
    pub y: i16,
}

/// Translates one console mouse record into an SGR mouse report, returning the new button state.
///
/// conhost delivers pointer activity as `MOUSE_EVENT` records (Windows Terminal instead delivers
/// SGR VT bytes directly, which never reach this path). This synthesizes the modern SGR (DEC 1006)
/// encoding `CSI < b ; x ; y M` for press/motion/wheel and `CSI < b ; x ; y m` for release, with
/// **one-based** coordinates, matching what the platform-neutral mouse decoder parses. The button
/// code `b` is built from the button number (`0` left, `1` middle, `2` right, `128`/`129` for the
/// extra buttons) OR-ed with the motion (`32`) or wheel (`64`) flag.
///
/// Press and release are recovered by diffing `button_state` against `prev_buttons`: a bit that
/// turned on is a press, a bit that turned off is a release. The caller threads the returned value
/// back in as `prev_buttons` on the next record. Keyboard modifier bits (Ctrl/Alt/Shift on a click)
/// are intentionally not encoded in this slice; see the module tests and the device decision log.
///
/// # Examples
///
/// ```ignore
/// // Left-button press at cell (9, 19) → `CSI < 0 ; 10 ; 20 M`.
/// let mut out = Vec::new();
/// let mouse = ConsoleMouse { button_state: 0x0001, event_flags: 0, x: 9, y: 19 };
/// let held = translate_mouse(mouse, 0, &mut out);
/// assert_eq!(out, b"\x1b[<0;10;20M");
/// assert_eq!(held, 0x0001);
/// ```
pub(super) fn translate_mouse(mouse: ConsoleMouse, prev_buttons: u32, out: &mut Vec<u8>) -> u32 {
    let column = coord_to_sgr(mouse.x);
    let row = coord_to_sgr(mouse.y);
    let flags = mouse.event_flags;
    let current = mouse.button_state & BUTTON_MASK;

    if flags & MOUSE_WHEELED != 0 {
        // Codes 64 (up) / 65 (down): a non-negative delta scrolls up.
        let down = wheel_delta(mouse.button_state) < 0;
        emit_sgr(SGR_WHEEL | u16::from(down), column, row, false, out);
        return prev_buttons;
    }
    if flags & MOUSE_HWHEELED != 0 {
        // Codes 66 (left) / 67 (right): a non-negative delta scrolls right.
        let right = wheel_delta(mouse.button_state) >= 0;
        emit_sgr(SGR_WHEEL | 2 | u16::from(right), column, row, false, out);
        return prev_buttons;
    }
    if flags & MOUSE_MOVED != 0 {
        let code = match lowest_button(current) {
            Some(bit) => sgr_button_number(bit) | SGR_MOTION,
            None => SGR_NO_BUTTON | SGR_MOTION,
        };
        emit_sgr(code, column, row, false, out);
        return current;
    }
    if flags & DOUBLE_CLICK != 0 {
        // The second click reports the button already held; emit it as another press.
        if let Some(bit) = lowest_button(current) {
            emit_sgr(sgr_button_number(bit), column, row, false, out);
        }
        return current;
    }

    // A plain button event: diff against the previous state to tell press from release.
    if let Some(bit) = lowest_button(current & !prev_buttons) {
        emit_sgr(sgr_button_number(bit), column, row, false, out);
    } else if let Some(bit) = lowest_button(prev_buttons & !current) {
        emit_sgr(sgr_button_number(bit), column, row, true, out);
    }
    current
}

/// Extracts the signed wheel delta from the high word of `dwButtonState`.
const fn wheel_delta(button_state: u32) -> i16 {
    (button_state >> 16) as i16
}

/// Isolates the lowest set console button bit within [`BUTTON_MASK`], if any.
fn lowest_button(buttons: u32) -> Option<u32> {
    let masked = buttons & BUTTON_MASK;
    (masked != 0).then(|| 1 << masked.trailing_zeros())
}

/// Maps a single console button bit to its SGR button number.
///
/// The three standard buttons use SGR numbers `0`/`1`/`2`; the two extra buttons use `128`/`129`
/// (the SGR high-button range, decoded back as `MouseButton::Other(8)` / `Other(9)`).
const fn sgr_button_number(button_bit: u32) -> u16 {
    match button_bit {
        BUTTON_LEFT => 0,
        BUTTON_MIDDLE => 1,
        BUTTON_RIGHT => 2,
        BUTTON_X1 => 128,
        BUTTON_X2 => 129,
        _ => SGR_NO_BUTTON,
    }
}

/// Converts a zero-based console cell coordinate to a one-based SGR coordinate, clamped to `>= 1`.
fn coord_to_sgr(coord: i16) -> u16 {
    u16::try_from((i32::from(coord) + 1).max(1)).unwrap_or(u16::MAX)
}

/// Appends `CSI < code ; column ; row (M|m)` to `out`, `m` for a release and `M` otherwise.
fn emit_sgr(code: u16, column: u16, row: u16, release: bool, out: &mut Vec<u8>) {
    let final_byte = if release { 'm' } else { 'M' };
    // Writing to a `Vec<u8>` is infallible, so the result is discarded.
    let _ = write!(out, "\x1b[<{code};{column};{row}{final_byte}");
}

// ---------------------------------------------------------------------------
// Window size: resize report synthesis and degenerate-extent detection.
// ---------------------------------------------------------------------------

/// Appends the in-band resize report `CSI 48 ; rows ; cols t` for `rows`×`cols` cells to `out`.
///
/// This is the DEC-mode-2048 cells-only report the platform-neutral resize decoder already parses:
/// the leading `48` is the discriminator, followed by cell height (rows) then width (cols), with
/// the optional pixel fields omitted. Windows has no VT resize sequence of its own
/// (microsoft/terminal#19618), so on a `WINDOW_BUFFER_SIZE_EVENT` the device synthesizes this
/// report from the *current window rectangle* — never from the record's `dwSize`, which is the
/// scrollback buffer, not the visible window.
///
/// # Examples
///
/// ```ignore
/// // A resize to 24 rows by 80 columns → `CSI 48 ; 24 ; 80 t`.
/// let mut out = Vec::new();
/// format_resize_report(24, 80, &mut out);
/// assert_eq!(out, b"\x1b[48;24;80t");
/// ```
pub(super) fn format_resize_report(rows: u16, cols: u16, out: &mut Vec<u8>) {
    // Writing to a `Vec<u8>` is infallible, so the result is discarded.
    let _ = write!(out, "\x1b[48;{rows};{cols}t");
}

/// Computes `(columns, rows)` cell extents from an inclusive `srWindow` rectangle.
///
/// `GetConsoleScreenBufferInfo` reports the window as an inclusive rectangle, so each extent is
/// `far - near + 1`. The result is `i32` (not `u16`) precisely so a degenerate rectangle with
/// `Right < Left` or `Bottom < Top` surfaces as a non-positive value that [`is_degenerate`]
/// rejects, rather than silently wrapping when narrowed to `u16`.
pub(super) const fn window_extent(left: i16, top: i16, right: i16, bottom: i16) -> (i32, i32) {
    let columns = right as i32 - left as i32 + 1;
    let rows = bottom as i32 - top as i32 + 1;
    (columns, rows)
}

/// Returns whether a `(columns, rows)` extent is degenerate (zero or negative in either axis).
///
/// Mirrors the Unix device's FM-Z2 contract: a zero or negative measurement is not a real size, so
/// the device reports [`Error::InvalidTerminalSize`](crate::terminal::Error::InvalidTerminalSize)
/// and the session falls back to `COLUMNS`/`LINES`.
pub(super) const fn is_degenerate(columns: i32, rows: i32) -> bool {
    columns <= 0 || rows <= 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes a sequence of UTF-16 units through one carry, returning the accumulated UTF-8 and a
    /// flag for whether a high surrogate is still pending afterward.
    fn decode(units: &[u16]) -> Vec<u8> {
        let mut carry = SurrogateCarry::default();
        let mut out = Vec::new();
        for &unit in units {
            carry.push(unit, &mut out);
        }
        out
    }

    #[test]
    fn ascii_and_bmp_units_pass_through() {
        assert_eq!(decode(&[u16::from(b'h'), u16::from(b'i')]), b"hi");
        // U+00E9 é and U+20AC € are BMP scalars.
        assert_eq!(decode(&[0x00E9]), "é".as_bytes());
        assert_eq!(decode(&[0x20AC]), "€".as_bytes());
    }

    #[test]
    fn surrogate_pair_in_one_batch_combines() {
        // U+1F600 GRINNING FACE = high D83D, low DE00.
        assert_eq!(decode(&[0xD83D, 0xDE00]), "😀".as_bytes());
    }

    #[test]
    fn surrogate_pair_split_across_reads_combines() {
        let mut carry = SurrogateCarry::default();
        let mut first = Vec::new();
        carry.push(0xD83D, &mut first);
        assert!(first.is_empty(), "high surrogate alone emits nothing yet");

        let mut second = Vec::new();
        carry.push(0xDE00, &mut second);
        assert_eq!(second, "😀".as_bytes());
    }

    #[test]
    fn lone_high_surrogate_at_eof_flushes_replacement() {
        let mut carry = SurrogateCarry::default();
        let mut out = Vec::new();
        carry.push(0xD83D, &mut out);
        assert!(out.is_empty());
        carry.flush(&mut out);
        assert_eq!(out, "\u{FFFD}".as_bytes());
        // A second flush is a no-op: the carry was consumed.
        let mut again = Vec::new();
        carry.flush(&mut again);
        assert!(again.is_empty());
    }

    #[test]
    fn high_surrogate_followed_by_non_low_flushes_then_processes() {
        // High then an ASCII 'A': replacement for the orphan, then 'A'.
        assert_eq!(decode(&[0xD83D, u16::from(b'A')]), "\u{FFFD}A".as_bytes());
        // High then another high: replacement for the first, second is held (no more output).
        assert_eq!(decode(&[0xD83D, 0xD83D]), "\u{FFFD}".as_bytes());
    }

    #[test]
    fn unpaired_low_surrogate_is_replacement() {
        assert_eq!(decode(&[0xDE00]), "\u{FFFD}".as_bytes());
        assert_eq!(
            decode(&[u16::from(b'x'), 0xDC00, u16::from(b'y')]),
            "x\u{FFFD}y".as_bytes()
        );
    }

    #[test]
    fn interleaved_pairs_and_scalars() {
        // 'a', 😀 (split naturally by unit), 'b', unpaired low, 'c'.
        let units = [
            u16::from(b'a'),
            0xD83D,
            0xDE00,
            u16::from(b'b'),
            0xDC00,
            u16::from(b'c'),
        ];
        assert_eq!(decode(&units), "a😀b\u{FFFD}c".as_bytes());
    }

    #[test]
    fn raw_input_mode_clears_and_sets_the_right_bits() {
        // Start from a typical cooked input mode with insert and quick-edit also set.
        let cooked = ENABLE_PROCESSED_INPUT
            | ENABLE_LINE_INPUT
            | ENABLE_ECHO_INPUT
            | 0x0020 // ENABLE_INSERT_MODE, an unrelated bit that must be preserved.
            | 0x0040; // ENABLE_QUICK_EDIT_MODE, likewise preserved.
        let raw = raw_input_mode(cooked);

        // Cleared: line input, echo, processed input.
        assert_eq!(raw & RAW_INPUT_CLEAR, 0);
        // Set: VT input, window input, mouse input, extended flags.
        assert_eq!(raw & RAW_INPUT_SET, RAW_INPUT_SET);
        // Preserved: the unrelated bits carried over untouched.
        assert_eq!(raw & 0x0060, 0x0060);
    }

    #[test]
    fn raw_output_mode_is_additive_and_vt_readback_detects_the_bit() {
        let cooked = ENABLE_PROCESSED_OUTPUT | ENABLE_WRAP_AT_EOL_OUTPUT;
        let raw = raw_output_mode(cooked);
        assert_eq!(raw & RAW_OUTPUT_SET, RAW_OUTPUT_SET);
        assert!(output_has_vt(raw));
        // A host that dropped the VT bit on readback is detected.
        assert!(!output_has_vt(raw & !ENABLE_VIRTUAL_TERMINAL_PROCESSING));
    }

    #[test]
    fn mouse_left_press_and_release_diff_by_previous_state() {
        let mut out = Vec::new();
        let press = ConsoleMouse {
            button_state: BUTTON_LEFT,
            event_flags: 0,
            x: 9,
            y: 19,
        };
        let held = translate_mouse(press, 0, &mut out);
        assert_eq!(out, b"\x1b[<0;10;20M");
        assert_eq!(held, BUTTON_LEFT);

        out.clear();
        let release = ConsoleMouse {
            button_state: 0,
            event_flags: 0,
            x: 9,
            y: 19,
        };
        let held = translate_mouse(release, held, &mut out);
        assert_eq!(out, b"\x1b[<0;10;20m");
        assert_eq!(held, 0);
    }

    #[test]
    fn mouse_right_and_middle_button_numbers() {
        let mut out = Vec::new();
        let right = ConsoleMouse {
            button_state: BUTTON_RIGHT,
            event_flags: 0,
            x: 0,
            y: 0,
        };
        translate_mouse(right, 0, &mut out);
        assert_eq!(out, b"\x1b[<2;1;1M");

        out.clear();
        let middle = ConsoleMouse {
            button_state: BUTTON_MIDDLE,
            event_flags: 0,
            x: 0,
            y: 0,
        };
        translate_mouse(middle, 0, &mut out);
        assert_eq!(out, b"\x1b[<1;1;1M");
    }

    #[test]
    fn mouse_extra_buttons_use_high_sgr_numbers() {
        let mut out = Vec::new();
        let x1 = ConsoleMouse {
            button_state: BUTTON_X1,
            event_flags: 0,
            x: 4,
            y: 4,
        };
        translate_mouse(x1, 0, &mut out);
        assert_eq!(out, b"\x1b[<128;5;5M");

        out.clear();
        let x2 = ConsoleMouse {
            button_state: BUTTON_X2,
            event_flags: 0,
            x: 4,
            y: 4,
        };
        translate_mouse(x2, 0, &mut out);
        assert_eq!(out, b"\x1b[<129;5;5M");
    }

    #[test]
    fn mouse_drag_sets_motion_with_the_held_button() {
        let mut out = Vec::new();
        let drag = ConsoleMouse {
            button_state: BUTTON_LEFT,
            event_flags: MOUSE_MOVED,
            x: 2,
            y: 3,
        };
        let held = translate_mouse(drag, BUTTON_LEFT, &mut out);
        // Left (0) plus motion (32) = 32.
        assert_eq!(out, b"\x1b[<32;3;4M");
        assert_eq!(held, BUTTON_LEFT);
    }

    #[test]
    fn mouse_bare_motion_uses_the_no_button_code() {
        let mut out = Vec::new();
        let motion = ConsoleMouse {
            button_state: 0,
            event_flags: MOUSE_MOVED,
            x: 2,
            y: 3,
        };
        translate_mouse(motion, 0, &mut out);
        // No button (3) plus motion (32) = 35.
        assert_eq!(out, b"\x1b[<35;3;4M");
    }

    #[test]
    fn mouse_wheel_up_and_down() {
        let mut out = Vec::new();
        // Positive high word = wheel up (code 64).
        let up = ConsoleMouse {
            button_state: 0x0078_0000,
            event_flags: MOUSE_WHEELED,
            x: 0,
            y: 0,
        };
        translate_mouse(up, 0, &mut out);
        assert_eq!(out, b"\x1b[<64;1;1M");

        out.clear();
        // Negative high word = wheel down (code 65).
        let down = ConsoleMouse {
            button_state: 0xFF88_0000,
            event_flags: MOUSE_WHEELED,
            x: 0,
            y: 0,
        };
        translate_mouse(down, 0, &mut out);
        assert_eq!(out, b"\x1b[<65;1;1M");
    }

    #[test]
    fn mouse_horizontal_wheel_left_and_right() {
        let mut out = Vec::new();
        let right = ConsoleMouse {
            button_state: 0x0078_0000,
            event_flags: MOUSE_HWHEELED,
            x: 0,
            y: 0,
        };
        translate_mouse(right, 0, &mut out);
        assert_eq!(out, b"\x1b[<67;1;1M");

        out.clear();
        let left = ConsoleMouse {
            button_state: 0xFF88_0000,
            event_flags: MOUSE_HWHEELED,
            x: 0,
            y: 0,
        };
        translate_mouse(left, 0, &mut out);
        assert_eq!(out, b"\x1b[<66;1;1M");
    }

    #[test]
    fn mouse_double_click_emits_a_press() {
        let mut out = Vec::new();
        let double = ConsoleMouse {
            button_state: BUTTON_LEFT,
            event_flags: DOUBLE_CLICK,
            x: 0,
            y: 0,
        };
        // Even though the button was already held, a double-click reports another press.
        let held = translate_mouse(double, BUTTON_LEFT, &mut out);
        assert_eq!(out, b"\x1b[<0;1;1M");
        assert_eq!(held, BUTTON_LEFT);
    }

    #[test]
    fn mouse_event_with_no_button_change_emits_nothing() {
        let mut out = Vec::new();
        // Same button state as before, no flags: nothing to report.
        let noop = ConsoleMouse {
            button_state: BUTTON_LEFT,
            event_flags: 0,
            x: 0,
            y: 0,
        };
        let held = translate_mouse(noop, BUTTON_LEFT, &mut out);
        assert!(out.is_empty());
        assert_eq!(held, BUTTON_LEFT);
    }

    #[test]
    fn mouse_negative_coordinates_clamp_to_one() {
        let mut out = Vec::new();
        let press = ConsoleMouse {
            button_state: BUTTON_LEFT,
            event_flags: 0,
            x: -5,
            y: -1,
        };
        translate_mouse(press, 0, &mut out);
        assert_eq!(out, b"\x1b[<0;1;1M");
    }

    #[test]
    fn resize_report_is_cells_only_rows_then_cols() {
        let mut out = Vec::new();
        format_resize_report(24, 80, &mut out);
        assert_eq!(out, b"\x1b[48;24;80t");
    }

    #[test]
    fn window_extent_is_inclusive_and_flags_degenerate() {
        // Left=0,Top=0,Right=79,Bottom=23 → 80 columns, 24 rows.
        assert_eq!(window_extent(0, 0, 79, 23), (80, 24));
        assert!(!is_degenerate(80, 24));

        // A collapsed window (Right < Left) is degenerate, not a 0-wide size.
        let (cols, rows) = window_extent(0, 0, -1, 23);
        assert_eq!((cols, rows), (0, 24));
        assert!(is_degenerate(cols, rows));

        assert!(is_degenerate(80, 0));
    }

    /// Asserts the portable bit copies match the real `windows-sys` header values so they cannot
    /// drift. Windows-only because that is where `windows-sys` is compiled.
    #[cfg(windows)]
    #[test]
    fn local_bit_values_match_windows_sys() {
        use windows_sys::Win32::System::Console as c;

        assert_eq!(ENABLE_PROCESSED_INPUT, c::ENABLE_PROCESSED_INPUT);
        assert_eq!(ENABLE_LINE_INPUT, c::ENABLE_LINE_INPUT);
        assert_eq!(ENABLE_ECHO_INPUT, c::ENABLE_ECHO_INPUT);
        assert_eq!(ENABLE_WINDOW_INPUT, c::ENABLE_WINDOW_INPUT);
        assert_eq!(ENABLE_MOUSE_INPUT, c::ENABLE_MOUSE_INPUT);
        assert_eq!(ENABLE_EXTENDED_FLAGS, c::ENABLE_EXTENDED_FLAGS);
        assert_eq!(
            ENABLE_VIRTUAL_TERMINAL_INPUT,
            c::ENABLE_VIRTUAL_TERMINAL_INPUT
        );

        assert_eq!(ENABLE_PROCESSED_OUTPUT, c::ENABLE_PROCESSED_OUTPUT);
        assert_eq!(ENABLE_WRAP_AT_EOL_OUTPUT, c::ENABLE_WRAP_AT_EOL_OUTPUT);
        assert_eq!(
            ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            c::ENABLE_VIRTUAL_TERMINAL_PROCESSING
        );
        assert_eq!(DISABLE_NEWLINE_AUTO_RETURN, c::DISABLE_NEWLINE_AUTO_RETURN);

        assert_eq!(BUTTON_LEFT, c::FROM_LEFT_1ST_BUTTON_PRESSED);
        assert_eq!(BUTTON_RIGHT, c::RIGHTMOST_BUTTON_PRESSED);
        assert_eq!(BUTTON_MIDDLE, c::FROM_LEFT_2ND_BUTTON_PRESSED);
        assert_eq!(BUTTON_X1, c::FROM_LEFT_3RD_BUTTON_PRESSED);
        assert_eq!(BUTTON_X2, c::FROM_LEFT_4TH_BUTTON_PRESSED);

        assert_eq!(MOUSE_MOVED, c::MOUSE_MOVED);
        assert_eq!(DOUBLE_CLICK, c::DOUBLE_CLICK);
        assert_eq!(MOUSE_WHEELED, c::MOUSE_WHEELED);
        assert_eq!(MOUSE_HWHEELED, c::MOUSE_HWHEELED);
    }
}
