//! Mouse event vocabulary and SGR (1006) mouse decode.
//!
//! A [`MouseEvent`] is the typed shape of one terminal mouse report: what happened
//! ([`MouseEventKind`]), which [`MouseButton`] it involved, where (1-based column and row), and the
//! active [`Modifiers`]. The decoder here reads the modern SGR encoding (`CSI < b ; x ; y M/m`,
//! DEC private mode 1006), the only mouse encoding this slice decodes to typed events; the legacy
//! X10 and urxvt forms are tolerated without decoding (see [the decode entry point](decode_sgr) and
//! design 02, FM-P13).
//!
//! # No scroll coalescing (FM-V6)
//!
//! Every mouse report becomes exactly one [`MouseEvent`], including scroll-wheel ticks: the decoder
//! never merges consecutive wheel events. Per-terminal physical-tick-to-event ratios vary 1:1 to
//! 9:1 with no protocol signal, so an application that wants a normalized scroll magnitude must be
//! able to see the raw event stream and build its own model (design 02, R-IN-6). Only resize events
//! coalesce, and that policy lives in the session, deliberately opposite to this one.

use crate::event::key::Modifiers;
use crate::syntax::ControlSequence;

/// What a [`MouseEvent`] reports happened.
///
/// The kind is derived from the SGR button-code bits: the motion bit (`32`) marks a move, the wheel
/// bits (`64`) mark a scroll, and otherwise the `M`/`m` final byte distinguishes a press from a
/// release. The enum is `#[non_exhaustive]` so future kinds add without churning consumers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum MouseEventKind {
    /// A button was pressed.
    Press,
    /// A button was released (the SGR `m` final byte).
    Release,
    /// The pointer moved. With a button held this is a drag; with no button it is a bare motion
    /// report (only sent under any-event mouse mode, DEC 1003).
    Moved,
    /// The scroll wheel moved in the given [`ScrollDirection`]. Never coalesced (FM-V6): each wheel
    /// tick the terminal sends is one event.
    Scroll(ScrollDirection),
}

/// The direction of a scroll-wheel [`MouseEventKind::Scroll`] event.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ScrollDirection {
    /// Wheel scrolled up / away from the user (button code `64`).
    Up,
    /// Wheel scrolled down / toward the user (button code `65`).
    Down,
    /// Wheel / trackpad scrolled left (button code `66`).
    Left,
    /// Wheel / trackpad scrolled right (button code `67`).
    Right,
}

/// Which mouse button a [`MouseEvent`] involved.
///
/// The three standard buttons come from the low two bits of the SGR button code. A scroll event
/// reports [`MouseButton::None`] because a wheel tick is not a button; a bare motion report with no
/// button held also reports [`MouseButton::None`]. Higher-numbered buttons (back/forward and
/// beyond) are preserved as [`MouseButton::Other`] rather than dropped. The enum is
/// `#[non_exhaustive]`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum MouseButton {
    /// The left (primary) button, SGR button number `0`.
    Left,
    /// The middle button, SGR button number `1`.
    Middle,
    /// The right (secondary) button, SGR button number `2`.
    Right,
    /// A higher-numbered button (for example back/forward), preserved by its SGR button number.
    Other(u8),
    /// No button: a scroll-wheel event or a bare motion report with nothing held.
    None,
}

/// A decoded mouse event: what happened, which button, where, and the active modifiers.
///
/// The position is 1-based (column, row) exactly as the terminal reports it — the top-left cell is
/// `(1, 1)`. This is the SGR wire convention preserved; the library does not rebase to 0. The
/// struct is `#[non_exhaustive]`.
///
/// # Example
///
/// ```
/// use qwertty::event::{MouseButton, MouseEventKind};
/// use qwertty::{Event, SemanticDecoder};
///
/// let mut decoder = SemanticDecoder::new();
/// // `CSI < 0 ; 10 ; 20 M` — left button pressed at column 10, row 20.
/// let events = decoder.feed(b"\x1b[<0;10;20M");
/// let mouse = events[0].mouse_event().expect("a mouse event");
///
/// assert_eq!(mouse.kind(), MouseEventKind::Press);
/// assert_eq!(mouse.button(), MouseButton::Left);
/// assert_eq!(mouse.column(), 10);
/// assert_eq!(mouse.row(), 20);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct MouseEvent {
    kind: MouseEventKind,
    button: MouseButton,
    column: u16,
    row: u16,
    modifiers: Modifiers,
}

impl MouseEvent {
    /// Returns what the event reports happened.
    #[must_use]
    pub fn kind(&self) -> MouseEventKind {
        self.kind
    }

    /// Returns which button the event involved (or [`MouseButton::None`] for scroll and bare
    /// motion).
    #[must_use]
    pub fn button(&self) -> MouseButton {
        self.button
    }

    /// Returns the 1-based column (x) the terminal reported.
    #[must_use]
    pub fn column(&self) -> u16 {
        self.column
    }

    /// Returns the 1-based row (y) the terminal reported.
    #[must_use]
    pub fn row(&self) -> u16 {
        self.row
    }

    /// Returns the modifiers active during the event (Shift, Alt, Ctrl from the SGR button code).
    #[must_use]
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }
}

/// Decodes an SGR (DEC 1006) mouse report `CSI < b ; x ; y M/m` into a [`MouseEvent`], or `None`.
///
/// Returns `None` when the sequence is not an SGR mouse report: the `<` private marker and a final
/// byte of `M` (press/motion/scroll) or `m` (release) are required, along with the three numeric
/// parameters. A sequence this layer does not recognize passes through as lossless syntax rather
/// than becoming a fake event (design 02).
///
/// The button code's bits are decoded as: the low two bits pick the button (`0` left, `1` middle,
/// `2` right); bit `4` is Shift, bit `8` is Alt, bit `16` is Ctrl; bit `32` marks motion; and bit
/// `64` marks the wheel, whose four values `64`/`65`/`66`/`67` are scroll up/down/left/right. The
/// `M`/`m` final byte gives press versus release for a button event.
pub(crate) fn decode_sgr(csi: &ControlSequence) -> Option<MouseEvent> {
    let params = csi.params();
    if params.private_markers() != b"<" || !params.intermediates().is_empty() {
        return None;
    }
    let final_byte = params.final_byte();
    let is_release = match final_byte {
        b'M' => false,
        b'm' => true,
        _ => return None,
    };

    // Exactly three `;`-separated numeric parameters: button code, column, row.
    let mut values = params.params().iter();
    let button_code = u16::try_from(values.next()?.value()?).ok()?;
    let column = u16::try_from(values.next()?.value()?).ok()?;
    let row = u16::try_from(values.next()?.value()?).ok()?;
    if values.next().is_some() {
        // More than three parameters is not a well-formed SGR mouse report.
        return None;
    }

    let modifiers = decode_modifiers(button_code);
    let (kind, button) = decode_kind_and_button(button_code, is_release);

    Some(MouseEvent {
        kind,
        button,
        column,
        row,
        modifiers,
    })
}

/// The SGR button-code modifier bits.
const SHIFT_BIT: u16 = 0b0000_0100;
const ALT_BIT: u16 = 0b0000_1000;
const CTRL_BIT: u16 = 0b0001_0000;
/// The motion flag: the report is a pointer move (a drag when a button is held).
const MOTION_BIT: u16 = 0b0010_0000;
/// The wheel flag: the low bits select a scroll direction rather than a button.
const WHEEL_BIT: u16 = 0b0100_0000;

/// Decodes the Shift/Alt/Ctrl modifier bits of an SGR button code into [`Modifiers`].
fn decode_modifiers(button_code: u16) -> Modifiers {
    let mut modifiers = Modifiers::empty();
    if button_code & SHIFT_BIT != 0 {
        modifiers.insert(Modifiers::SHIFT);
    }
    if button_code & ALT_BIT != 0 {
        modifiers.insert(Modifiers::ALT);
    }
    if button_code & CTRL_BIT != 0 {
        modifiers.insert(Modifiers::CTRL);
    }
    modifiers
}

/// Decodes the kind and button from an SGR button code, given the press/release final byte.
///
/// Precedence follows the encoding: the wheel flag (`64`) makes it a scroll (which is never a
/// release — wheels report on `M`), the motion flag (`32`) makes it a move, and otherwise the low
/// two bits name the button pressed or released.
fn decode_kind_and_button(button_code: u16, is_release: bool) -> (MouseEventKind, MouseButton) {
    if button_code & WHEEL_BIT != 0 {
        // The low two bits pick the wheel direction: 0 up, 1 down, 2 left, 3 right (codes 64-67).
        let direction = match button_code & 0b11 {
            0 => ScrollDirection::Up,
            1 => ScrollDirection::Down,
            2 => ScrollDirection::Left,
            _ => ScrollDirection::Right,
        };
        return (MouseEventKind::Scroll(direction), MouseButton::None);
    }

    let button = decode_button(button_code);

    if button_code & MOTION_BIT != 0 {
        return (MouseEventKind::Moved, button);
    }

    let kind = if is_release {
        MouseEventKind::Release
    } else {
        MouseEventKind::Press
    };
    (kind, button)
}

/// Maps the low button bits of a (non-wheel) SGR button code to a [`MouseButton`].
///
/// The low two bits are `0` left, `1` middle, `2` right. Higher-numbered buttons set bit `128`
/// (codes 128+) for buttons 8-11 (back/forward and beyond); those are preserved as
/// [`MouseButton::Other`] by their reconstructed button number. A bare motion report with no button
/// held encodes button bits `3` (the "no button" placeholder), which maps to [`MouseButton::None`].
fn decode_button(button_code: u16) -> MouseButton {
    // Buttons 8-11 (back/forward, etc.) set bit 7 (128) with the low two bits selecting among them.
    if button_code & 0b1000_0000 != 0 {
        let number = 8 + u8::try_from(button_code & 0b11).unwrap_or(0);
        return MouseButton::Other(number);
    }
    match button_code & 0b11 {
        0 => MouseButton::Left,
        1 => MouseButton::Middle,
        2 => MouseButton::Right,
        // Button bits `3` is the "no button" code, used by a bare motion report.
        _ => MouseButton::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{SyntaxParser, SyntaxToken};

    /// Parses `bytes` into the single CSI control sequence they encode.
    fn csi(bytes: &[u8]) -> ControlSequence {
        let mut parser = SyntaxParser::new();
        let mut tokens = parser.feed(bytes);
        tokens.extend(parser.finish());
        assert_eq!(tokens.len(), 1, "expected one token from {bytes:?}");
        match tokens.into_iter().next().expect("one token") {
            SyntaxToken::Csi(csi) => csi,
            other => panic!("expected a CSI token, got {other:?}"),
        }
    }

    fn mouse(bytes: &[u8]) -> MouseEvent {
        decode_sgr(&csi(bytes))
            .unwrap_or_else(|| panic!("{bytes:?} did not decode to a mouse event"))
    }

    #[test]
    fn left_press_at_position() {
        let event = mouse(b"\x1b[<0;10;20M");
        assert_eq!(event.kind(), MouseEventKind::Press);
        assert_eq!(event.button(), MouseButton::Left);
        assert_eq!(event.column(), 10);
        assert_eq!(event.row(), 20);
        assert_eq!(event.modifiers(), Modifiers::empty());
    }

    #[test]
    fn middle_and_right_buttons() {
        assert_eq!(mouse(b"\x1b[<1;5;5M").button(), MouseButton::Middle);
        assert_eq!(mouse(b"\x1b[<2;5;5M").button(), MouseButton::Right);
    }

    #[test]
    fn release_final_byte_is_release_kind() {
        let event = mouse(b"\x1b[<0;10;20m");
        assert_eq!(event.kind(), MouseEventKind::Release);
        assert_eq!(event.button(), MouseButton::Left);
    }

    #[test]
    fn modifier_bits_decode() {
        // Shift (4), Alt (8), Ctrl (16) on a left press: 0 + 4 + 8 + 16 = 28.
        let event = mouse(b"\x1b[<28;1;1M");
        assert_eq!(
            event.modifiers(),
            Modifiers::SHIFT
                .union(Modifiers::ALT)
                .union(Modifiers::CTRL)
        );
        assert_eq!(event.button(), MouseButton::Left);
    }

    #[test]
    fn each_modifier_alone() {
        assert_eq!(mouse(b"\x1b[<4;1;1M").modifiers(), Modifiers::SHIFT);
        assert_eq!(mouse(b"\x1b[<8;1;1M").modifiers(), Modifiers::ALT);
        assert_eq!(mouse(b"\x1b[<16;1;1M").modifiers(), Modifiers::CTRL);
    }

    #[test]
    fn motion_with_button_is_a_drag() {
        // Motion flag (32) with left button (0): 32. A held-button move.
        let event = mouse(b"\x1b[<32;3;4M");
        assert_eq!(event.kind(), MouseEventKind::Moved);
        assert_eq!(event.button(), MouseButton::Left);
    }

    #[test]
    fn bare_motion_has_no_button() {
        // Motion flag (32) with the no-button code (3): 35. Sent under any-event mode.
        let event = mouse(b"\x1b[<35;7;8M");
        assert_eq!(event.kind(), MouseEventKind::Moved);
        assert_eq!(event.button(), MouseButton::None);
    }

    #[test]
    fn wheel_up_and_down() {
        let up = mouse(b"\x1b[<64;5;5M");
        assert_eq!(up.kind(), MouseEventKind::Scroll(ScrollDirection::Up));
        assert_eq!(up.button(), MouseButton::None);

        let down = mouse(b"\x1b[<65;5;5M");
        assert_eq!(down.kind(), MouseEventKind::Scroll(ScrollDirection::Down));
    }

    #[test]
    fn horizontal_wheel() {
        assert_eq!(
            mouse(b"\x1b[<66;5;5M").kind(),
            MouseEventKind::Scroll(ScrollDirection::Left)
        );
        assert_eq!(
            mouse(b"\x1b[<67;5;5M").kind(),
            MouseEventKind::Scroll(ScrollDirection::Right)
        );
    }

    #[test]
    fn wheel_carries_modifiers() {
        // Ctrl (16) + wheel up (64) = 80. Ctrl+scroll is a common zoom gesture.
        let event = mouse(b"\x1b[<80;5;5M");
        assert_eq!(event.kind(), MouseEventKind::Scroll(ScrollDirection::Up));
        assert_eq!(event.modifiers(), Modifiers::CTRL);
    }

    #[test]
    fn extended_buttons_are_preserved() {
        // Bit 128 with low bits 0 = button 8 (back). Preserved, not dropped.
        let event = mouse(b"\x1b[<128;1;1M");
        assert_eq!(event.button(), MouseButton::Other(8));
        assert_eq!(mouse(b"\x1b[<129;1;1M").button(), MouseButton::Other(9));
    }

    #[test]
    fn max_coordinates_decode() {
        // SGR removes the 223-cell ceiling of legacy encodings; large coordinates decode intact.
        let event = mouse(b"\x1b[<0;1000;2000M");
        assert_eq!(event.column(), 1000);
        assert_eq!(event.row(), 2000);
    }

    #[test]
    fn non_sgr_shapes_are_declined() {
        // No `<` marker, wrong final byte, or wrong parameter count are not SGR mouse reports.
        assert!(decode_sgr(&csi(b"\x1b[0;10;20M")).is_none());
        assert!(decode_sgr(&csi(b"\x1b[<0;10;20R")).is_none());
        assert!(decode_sgr(&csi(b"\x1b[<0;10M")).is_none());
        assert!(decode_sgr(&csi(b"\x1b[<0;10;20;30M")).is_none());
    }
}
