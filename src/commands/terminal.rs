//! Terminal-level command helpers.
//!
//! These helpers encode controls that ask about or affect the terminal as a whole instead of a
//! cursor, screen, or text attribute subdomain.

use crate::{Command, KittyKeyboardFlags, escape};

/// Which mouse tracking mode to enable, always paired with SGR extended coordinates (1006).
///
/// The three DEC private modes differ in *which* mouse events the terminal reports; SGR (1006) is
/// the coordinate encoding qwertty decodes, and every mouse enable pairs the chosen tracking mode
/// with it (design 02, R-IN-6). The enum is `#[non_exhaustive]`; pixel-coordinate mode (1016) is a
/// later (P2) addition.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum MouseMode {
    /// Normal tracking (DEC 1000): press and release only.
    Normal,
    /// Button-event tracking (DEC 1002): press, release, and motion **while a button is held**
    /// (drag).
    ButtonEvent,
    /// Any-event tracking (DEC 1003): press, release, and **all** pointer motion, even with no
    /// button held.
    AnyEvent,
}

impl MouseMode {
    /// Returns the DEC private-mode number for this tracking mode (1000, 1002, or 1003).
    #[must_use]
    const fn tracking_number(self) -> u16 {
        match self {
            Self::Normal => 1000,
            Self::ButtonEvent => 1002,
            Self::AnyEvent => 1003,
        }
    }
}

/// The SGR extended-coordinate mouse mode (DEC 1006), always paired with a tracking mode.
const SGR_MOUSE: u16 = 1006;
/// The focus-reporting DEC private mode (1004).
const FOCUS: u16 = 1004;
/// The bracketed-paste DEC private mode (2004).
const BRACKETED_PASTE: u16 = 2004;
/// The in-band resize DEC private mode (2048).
const IN_BAND_RESIZE: u16 = 2048;

/// Requests terminal status.
///
/// This encodes the ECMA-48 Device Status Report request `CSI 5 n`, emitted as `b"\x1b[5n"`.
/// Terminals commonly answer with `CSI 0 n` for ready or `CSI 3 n` for malfunction.
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a
/// response, route query responses, or filter unrelated input.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::request_status());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[5n");
/// ```
#[must_use]
pub fn request_status() -> Command {
    escape::csi("5", 'n')
}

/// Requests Primary Device Attributes (DA1).
///
/// This encodes the ECMA-48 request `CSI c`, emitted as `b"\x1b[c"`. Terminals answer with
/// `CSI ? … c`. In a capability probe DA1 is written **last**, as a fence: because a terminal
/// answers queries in order, DA1's reply arriving means every earlier reply that was coming has
/// arrived (design 03, FM-Q7). DA1 is a fence, not a feature oracle — its presence proves nothing
/// about features (FM-C4).
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a response,
/// route query responses, or filter unrelated input.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::request_primary_device_attributes());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[c");
/// ```
#[must_use]
pub fn request_primary_device_attributes() -> Command {
    escape::csi("", 'c')
}

/// Requests the terminal name and version (XTVERSION).
///
/// This encodes `CSI > q`, emitted as `b"\x1b[>q"`. Terminals answer with a DCS string
/// `DCS > | name version ST`, which qwertty parses into an
/// [`XtVersionReport`](crate::report::XtVersionReport).
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a response,
/// route query responses, or filter unrelated input.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::request_xtversion());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[>q");
/// ```
#[must_use]
pub fn request_xtversion() -> Command {
    escape::csi(">", 'q')
}

/// Requests the text-area size in pixels (XTWINOPS 14).
///
/// This encodes `CSI 14 t`, emitted as `b"\x1b[14t"` (`db/ecma48-csi.toml`'s
/// `csi.xtwinops.text_area_pixels`). Terminals answer with `CSI 4 ; height ; width t`, which
/// qwertty parses into a [`TextAreaPixelsReport`](crate::report::TextAreaPixelsReport). Some
/// terminal stacks answer with zero dimensions; the report type preserves the zeros and its
/// `pixel_size` accessor refuses to turn them into a fake geometry (FM-Z5).
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a
/// response, route query responses, or filter unrelated input.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::request_text_area_pixels());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[14t");
/// ```
#[must_use]
pub fn request_text_area_pixels() -> Command {
    escape::csi("14", 't')
}

/// Requests the character-cell size in pixels (XTWINOPS 16).
///
/// This encodes `CSI 16 t`, emitted as `b"\x1b[16t"` (`db/ecma48-csi.toml`'s
/// `csi.xtwinops.cell_size`). Terminals answer with `CSI 6 ; height ; width t`, which qwertty
/// parses into a [`CellSizeReport`](crate::report::CellSizeReport) — the cells-to-pixels
/// conversion an application needs to size image placements. Fewer terminals implement this than
/// the text-area query; silence and zero answers both stay *unknown* (FM-Z5).
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a
/// response, route query responses, or filter unrelated input.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::request_cell_size());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[16t");
/// ```
#[must_use]
pub fn request_cell_size() -> Command {
    escape::csi("16", 't')
}

/// Requests the state of a DEC private mode (DECRQM).
///
/// This encodes the private-mode DECRQM request `CSI ? mode $ p`. For mode 2026 it emits
/// `b"\x1b[?2026$p"`. Terminals answer with a DEC private mode report `CSI ? mode ; value $ y`,
/// which qwertty parses into a [`DecPrivateModeReport`](crate::report::DecPrivateModeReport). The
/// reply carries the same mode number as the query, which is the discriminator that keeps two
/// concurrent DECRQM queries from cross-completing (FM-Q10).
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a response,
/// route query responses, or filter unrelated input.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::request_dec_private_mode(2026));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?2026$p");
/// ```
#[must_use]
pub fn request_dec_private_mode(mode: u16) -> Command {
    // `$` is the DECRQM intermediate byte, emitted between the parameters and the `p` final byte.
    escape::csi(format!("?{mode}$"), 'p')
}

/// Requests the terminal's current kitty keyboard flags (`CSI ? u`).
///
/// This is a probe-oriented alias for [`query_kitty_keyboard_flags`]: it encodes the same `CSI ? u`
/// query the terminal answers with `CSI ? flags u`, named consistently with the other
/// `request_*` probe helpers. The verify-after-push handshake uses
/// [`query_kitty_keyboard_flags`]; a capability probe that only reads the current flags without
/// pushing uses this name.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::request_kitty_keyboard_flags());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?u");
/// ```
#[must_use]
pub fn request_kitty_keyboard_flags() -> Command {
    query_kitty_keyboard_flags()
}

/// Pushes kitty keyboard progressive-enhancement flags onto the terminal's flags stack.
///
/// This encodes `CSI > flags u`, which asks the terminal to turn on the requested reporting
/// behaviours and remembers the previous set on a stack so it can be restored. A terminal enables
/// only the subset it supports, so a caller should follow this with [`query_kitty_keyboard_flags`]
/// to learn what was granted (verify-after-push, design 06). Pushing the empty set emits
/// `CSI > 0 u`.
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a
/// response, or record ledger state.
///
/// # Example
///
/// ```
/// use qwertty::commands::terminal;
/// use qwertty::{CommandBuffer, KittyKeyboardFlags};
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::push_kitty_keyboard_flags(
///     KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES,
/// ));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[>1u");
/// ```
#[must_use]
pub fn push_kitty_keyboard_flags(flags: KittyKeyboardFlags) -> Command {
    escape::csi(format!(">{}", flags.bits()), 'u')
}

/// Pops one entry off the terminal's kitty keyboard flags stack.
///
/// This encodes `CSI < 1 u`, restoring the flags in effect before the matching
/// [`push_kitty_keyboard_flags`]. It is the exact undo of a single push and is what the session
/// ledger replays on `leave`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::pop_kitty_keyboard_flags());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[<1u");
/// ```
#[must_use]
pub fn pop_kitty_keyboard_flags() -> Command {
    escape::csi("<1", 'u')
}

/// Queries the terminal's current kitty keyboard flags.
///
/// This encodes `CSI ? u`. The terminal answers with `CSI ? flags u`, the currently active
/// progressive-enhancement flags — the *granted* set after a push. This is the query half of
/// verify-after-push (design 06).
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::query_kitty_keyboard_flags());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?u");
/// ```
#[must_use]
pub fn query_kitty_keyboard_flags() -> Command {
    escape::csi("?", 'u')
}

/// Builds the bytes for a DEC private-mode set (`CSI ? N h`).
fn dec_set(number: u16) -> Vec<u8> {
    let mut bytes = Vec::new();
    escape::csi(format!("?{number}"), 'h').encode(&mut bytes);
    bytes
}

/// Builds the bytes for a DEC private-mode reset (`CSI ? N l`).
fn dec_reset(number: u16) -> Vec<u8> {
    let mut bytes = Vec::new();
    escape::csi(format!("?{number}"), 'l').encode(&mut bytes);
    bytes
}

/// Enables mouse reporting: the chosen tracking mode plus SGR extended coordinates (1006).
///
/// This encodes `CSI ? N h CSI ? 1006 h`, where `N` is the tracking mode's number (1000, 1002, or
/// 1003). The tracking mode picks *which* events the terminal reports; 1006 selects the SGR
/// coordinate encoding qwertty decodes to [`MouseEvent`](crate::MouseEvent). The two are always
/// paired (design 02, R-IN-6). The session pairs this with [`disable_mouse`] in its mode ledger so
/// teardown resets both.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal::{self, MouseMode};
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::enable_mouse(MouseMode::ButtonEvent));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?1002h\x1b[?1006h");
/// ```
#[must_use]
pub fn enable_mouse(mode: MouseMode) -> Command {
    let mut bytes = dec_set(mode.tracking_number());
    bytes.extend(dec_set(SGR_MOUSE));
    Command::raw(bytes)
}

/// Disables mouse reporting: resets SGR coordinates (1006) and the tracking mode.
///
/// This encodes `CSI ? 1006 l CSI ? N l`, the exact reverse of [`enable_mouse`]. The session's mode
/// ledger uses this as the mouse undo, so orderly leave and the emergency blob both reset mouse
/// reporting.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal::{self, MouseMode};
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::disable_mouse(MouseMode::ButtonEvent));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?1006l\x1b[?1002l");
/// ```
#[must_use]
pub fn disable_mouse(mode: MouseMode) -> Command {
    let mut bytes = dec_reset(SGR_MOUSE);
    bytes.extend(dec_reset(mode.tracking_number()));
    Command::raw(bytes)
}

/// Enables focus reporting (DEC 1004): `CSI ? 1004 h`.
///
/// With focus reporting on, the terminal sends `CSI I` on focus gain and `CSI O` on focus loss,
/// which qwertty decodes to [`FocusEvent`](crate::FocusEvent) (R-IN-9).
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::enable_focus_events());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?1004h");
/// ```
#[must_use]
pub fn enable_focus_events() -> Command {
    Command::raw(dec_set(FOCUS))
}

/// Disables focus reporting (DEC 1004): `CSI ? 1004 l`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::disable_focus_events());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?1004l");
/// ```
#[must_use]
pub fn disable_focus_events() -> Command {
    Command::raw(dec_reset(FOCUS))
}

/// Enables bracketed paste (DEC 2004): `CSI ? 2004 h`.
///
/// With bracketed paste on, the terminal wraps pasted text in `ESC [ 200 ~ … ESC [ 201 ~`, which
/// qwertty decodes to [`PasteEvent`](crate::PasteEvent) segments — so pasted text is delivered as
/// data, never mistaken for typed keys or keybindings (R-IN-7, FM-P12).
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::enable_bracketed_paste());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?2004h");
/// ```
#[must_use]
pub fn enable_bracketed_paste() -> Command {
    Command::raw(dec_set(BRACKETED_PASTE))
}

/// Disables bracketed paste (DEC 2004): `CSI ? 2004 l`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::disable_bracketed_paste());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?2004l");
/// ```
#[must_use]
pub fn disable_bracketed_paste() -> Command {
    Command::raw(dec_reset(BRACKETED_PASTE))
}

/// Enables in-band resize reporting (DEC 2048): `CSI ? 2048 h`.
///
/// With it on, the terminal reports every size change in band as `CSI 48 ; height ; width ;
/// height_px ; width_px t`, which qwertty decodes to [`ResizeEvent`](crate::ResizeEvent). This is
/// the preferred resize source where available, letting an application avoid the out-of-band
/// `SIGWINCH` signal entirely (R-IN-8, design 01).
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::enable_in_band_resize());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?2048h");
/// ```
#[must_use]
pub fn enable_in_band_resize() -> Command {
    Command::raw(dec_set(IN_BAND_RESIZE))
}

/// Disables in-band resize reporting (DEC 2048): `CSI ? 2048 l`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::terminal;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(terminal::disable_in_band_resize());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[?2048l");
/// ```
#[must_use]
pub fn disable_in_band_resize() -> Command {
    Command::raw(dec_reset(IN_BAND_RESIZE))
}
