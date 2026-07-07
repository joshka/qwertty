//! Resize event vocabulary and in-band resize decode (DEC private mode 2048).
//!
//! When in-band resize is enabled (mode 2048), the terminal reports every size change as a CSI
//! sequence instead of relying on the out-of-band `SIGWINCH` signal. The wire form is
//! `CSI 48 ; height ; width ; height_px ; width_px t` — an XTWINOPS-shaped `t` final whose leading
//! `48` parameter is the discriminator that marks it a mode-2048 resize report rather than one of
//! the other window-operation `t` sequences (design 02, R-IN-8).
//!
//! This layer decodes that report into a [`ResizeEvent`] carrying the cell geometry
//! ([`TerminalSize`]) and, when the report's pixel fields are nonzero, the pixel geometry
//! ([`PixelSize`]). Every other `t` final — window ops qwertty does not decode — passes through as
//! lossless [`Event::Syntax`](crate::Event::Syntax) rather than becoming a fake resize (design 02's
//! forward-compatibility contract).
//!
//! # Coalescing lives elsewhere
//!
//! This decode is per-report: it never merges a burst of resize reports. Resize *coalescing* — one
//! `Resize` with the final geometry when several are pending — is a delivery policy that lives in
//! the async session's `next_event` queue (design 01 §resize, FM-G2), deliberately opposite to the
//! never-coalesce policy for mouse and scroll (FM-V6).

use crate::syntax::ControlSequence;
use crate::terminal::{PixelSize, TerminalSize};

/// The XTWINOPS window-operation code that marks an in-band resize report (`CSI 48 ; … t`).
const IN_BAND_RESIZE_OP: u32 = 48;

/// A decoded terminal resize event.
///
/// A `ResizeEvent` reports the terminal's new cell geometry and, when the source carried it, the
/// pixel geometry. It arrives from two sources with the same shape: an in-band resize report
/// (mode 2048) decoded here, and a `SIGWINCH`-driven `size()` read the async session synthesizes
/// (design 01). Applications treat both identically — the surface is one event.
///
/// The struct is `#[non_exhaustive]`.
///
/// # Example
///
/// ```
/// use qwertty::{Event, SemanticDecoder, TerminalSize};
///
/// let mut decoder = SemanticDecoder::new();
/// // `CSI 48 ; 24 ; 80 ; 0 ; 0 t` — 24 rows, 80 columns, no pixel geometry.
/// let events = decoder.feed(b"\x1b[48;24;80;0;0t");
/// let resize = events[0].resize_event().expect("a resize event");
///
/// assert_eq!(resize.cells(), TerminalSize::new(80, 24));
/// assert_eq!(resize.pixels(), None);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ResizeEvent {
    cells: TerminalSize,
    pixels: Option<PixelSize>,
}

impl ResizeEvent {
    /// Creates a resize event from its cell geometry and optional pixel geometry.
    ///
    /// This is the constructor the async session uses to synthesize a `SIGWINCH`-driven resize from
    /// a `size()` read, which has cell geometry only.
    #[must_use]
    pub const fn new(cells: TerminalSize, pixels: Option<PixelSize>) -> Self {
        Self { cells, pixels }
    }

    /// Returns the terminal's new size in cells.
    #[must_use]
    pub const fn cells(&self) -> TerminalSize {
        self.cells
    }

    /// Returns the terminal's new size in pixels, or `None` when the source carried no pixel
    /// geometry.
    ///
    /// An in-band report whose pixel fields are zero (or a `SIGWINCH`-synthesized event) reports
    /// `None`; a report with nonzero pixel fields reports `Some`.
    #[must_use]
    pub const fn pixels(&self) -> Option<PixelSize> {
        self.pixels
    }
}

/// Decodes an in-band resize report `CSI 48 ; h ; w ; hp ; wp t` into a [`ResizeEvent`], or `None`.
///
/// Returns `None` for any `t`-final CSI that is not a mode-2048 resize report: the leading
/// parameter must be exactly `48` (the discriminator), there must be no private markers or
/// intermediates, and the cell height and width must be present. The pixel fields are optional —
/// absent or zero pixel fields yield [`ResizeEvent::pixels`] `None`, nonzero fields yield `Some`.
///
/// Any other `t` final (the other XTWINOPS window operations) declines here and passes through as
/// lossless syntax rather than a fake resize (design 02).
pub(crate) fn decode(csi: &ControlSequence) -> Option<ResizeEvent> {
    let params = csi.params();
    if params.final_byte() != b't'
        || !params.private_markers().is_empty()
        || !params.intermediates().is_empty()
    {
        return None;
    }

    let mut values = params.params().iter();

    // The leading `48` window-operation code is the discriminator: without it this is some other
    // XTWINOPS `t` sequence, which must pass through untouched.
    if values.next()?.value()? != IN_BAND_RESIZE_OP {
        return None;
    }

    // Cell height then width are required (the report always carries them).
    let height = u16::try_from(values.next()?.value()?).ok()?;
    let width = u16::try_from(values.next()?.value()?).ok()?;
    let cells = TerminalSize::new(width, height);

    // Pixel height then width are optional; a zero field or a missing field means "no pixel
    // geometry". Both must be present and nonzero to report pixels.
    let height_px = values.next().and_then(|param| param.value());
    let width_px = values.next().and_then(|param| param.value());
    let pixels = decode_pixels(height_px, width_px);

    Some(ResizeEvent { cells, pixels })
}

/// Builds the optional pixel geometry from the report's pixel height and width fields.
///
/// The fields are `Some(0)` when the terminal explicitly reports no pixel geometry, and `None` when
/// the report omitted them entirely (a shorter report). Either way the result is `None` unless both
/// fields are present and nonzero.
fn decode_pixels(height_px: Option<u32>, width_px: Option<u32>) -> Option<PixelSize> {
    let height_px = u16::try_from(height_px?).ok()?;
    let width_px = u16::try_from(width_px?).ok()?;
    if height_px == 0 || width_px == 0 {
        return None;
    }
    Some(PixelSize::new(width_px, height_px))
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

    #[test]
    fn cell_only_report_decodes() {
        // `CSI 48 ; 24 ; 80 ; 0 ; 0 t` — 24 rows, 80 columns, no pixel geometry.
        let event = decode(&csi(b"\x1b[48;24;80;0;0t")).expect("a resize event");
        assert_eq!(event.cells(), TerminalSize::new(80, 24));
        assert_eq!(event.pixels(), None);
    }

    #[test]
    fn cell_and_pixel_report_decodes() {
        // `CSI 48 ; 24 ; 80 ; 480 ; 800 t` — 24x80 cells, 800x480 pixels.
        let event = decode(&csi(b"\x1b[48;24;80;480;800t")).expect("a resize event");
        assert_eq!(event.cells(), TerminalSize::new(80, 24));
        assert_eq!(event.pixels(), Some(PixelSize::new(800, 480)));
        assert_eq!(event.pixels().map(PixelSize::width), Some(800));
        assert_eq!(event.pixels().map(PixelSize::height), Some(480));
    }

    #[test]
    fn four_parameter_report_has_no_pixels() {
        // A report that omits the pixel fields entirely still decodes its cell geometry.
        let event = decode(&csi(b"\x1b[48;30;120t")).expect("a resize event");
        assert_eq!(event.cells(), TerminalSize::new(120, 30));
        assert_eq!(event.pixels(), None);
    }

    #[test]
    fn one_zero_pixel_field_means_no_pixels() {
        // A partial pixel report (one field zero) is treated as no pixel geometry, not a 0-sized
        // one: both fields must be present and nonzero.
        assert_eq!(
            decode(&csi(b"\x1b[48;24;80;0;800t"))
                .expect("a resize event")
                .pixels(),
            None,
        );
        assert_eq!(
            decode(&csi(b"\x1b[48;24;80;480;0t"))
                .expect("a resize event")
                .pixels(),
            None,
        );
    }

    #[test]
    fn other_t_finals_are_declined() {
        // Other XTWINOPS `t` sequences (no leading 48) are not resize reports and pass through as
        // syntax. `CSI 18 t` reports the text-area size; `CSI 22 ; 0 t` pushes the window title.
        assert!(decode(&csi(b"\x1b[18t")).is_none());
        assert!(decode(&csi(b"\x1b[22;0t")).is_none());
        assert!(decode(&csi(b"\x1b[8;24;80t")).is_none());
    }

    #[test]
    fn non_t_final_is_declined() {
        // A leading 48 with a different final byte is not a resize report.
        assert!(decode(&csi(b"\x1b[48;24;80H")).is_none());
    }

    #[test]
    fn private_marker_or_intermediate_declines() {
        // A private marker or intermediate byte disqualifies the report.
        assert!(decode(&csi(b"\x1b[?48;24;80t")).is_none());
        assert!(decode(&csi(b"\x1b[48;24;80 t")).is_none());
    }

    #[test]
    fn missing_cell_dimensions_declines() {
        // `CSI 48 t` with no cell geometry is not a well-formed resize report.
        assert!(decode(&csi(b"\x1b[48t")).is_none());
        assert!(decode(&csi(b"\x1b[48;24t")).is_none());
    }
}
