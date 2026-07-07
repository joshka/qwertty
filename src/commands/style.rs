//! SGR (Select Graphic Rendition) styling command helpers.
//!
//! These helpers encode ECMA-48 SGR text attributes and colors: foreground/background/underline
//! color in 16-color, 256-color, and truecolor forms, boolean attributes (bold, italic,
//! underline, and friends) with their individual resets, and the underline substyles (straight,
//! double, curly, dotted, dashed) that xterm, Kitty, iTerm2, and `WezTerm` commonly support
//! through SGR 4's colon subparameter.
//!
//! Each helper returns one granular [`Command`]: a single SGR parameter (or, for RGB/indexed
//! colors, the small parameter run a color needs) rather than a combined "set all these
//! attributes" call. A caller composes exactly the attributes that changed between frames onto a
//! [`CommandBuffer`](crate::CommandBuffer); qwertty does not track prior style state or diff
//! anything itself (R-OUT-2 "minimal-diff friendly" describes the shape callers get for building
//! their own diffing renderer, not a diff qwertty performs).
//!
//! ## Colon vs. semicolon SGR forms (FM-W6)
//!
//! SGR historically used only `;` to separate parameters. Later extensions (256-color,
//! truecolor, underline color) introduced a colon-subparameter form (`38:2:r:g:b`) to keep a
//! single color selector as one parameter with subparts. Real terminal support is uneven, and
//! the audited failure-mode survey (FM-W6) found colon-form 8-bit SGR does not work in
//! PowerShell/conhost, and non-default underline color has separately caused rendering bugs
//! (blinking) on Windows Terminal and hard errors on Windows 7 hosts — bad enough that crossterm
//! feature-gates underline color entirely. Given that evidence, every color helper in this module
//! emits the semicolon form, including underline color: it is the form with the widest observed
//! support, and using one separator convention throughout keeps output uniform. The colon form is
//! not emitted anywhere in this module.
//!
//! Underline *style* (straight, double, curly, dotted, dashed) is different: there is no
//! semicolon-form encoding for it anywhere in use. The colon subparameter on SGR 4 (`4:3` for
//! curly, and so on) is the only widely-implemented spelling, originating with Kitty/VTE and now
//! shared by xterm, iTerm2, and `WezTerm`, so [`underline_style`] emits that colon form because it
//! is the only form that exists, not as a form choice.
//!
//! ```
//! use qwertty::CommandBuffer;
//! use qwertty::commands::style::{self, Color};
//!
//! let mut frame = CommandBuffer::new();
//! frame
//!     .command(style::bold())
//!     .command(style::foreground(Color::Red))
//!     .text("error")
//!     .command(style::reset_all());
//!
//! assert_eq!(frame.as_bytes(), b"\x1b[1m\x1b[31merror\x1b[0m");
//! ```

use crate::{Command, escape};

/// A terminal color, usable as a foreground, background, or underline color.
///
/// The 16 named variants map to the classic ECMA-48/ANSI color numbers; [`Color::Indexed`]
/// selects one of a 256-color palette entry, and [`Color::Rgb`] selects a 24-bit truecolor value.
/// Each command that accepts a `Color` maps it to the SGR parameters appropriate for that
/// command's slot (foreground, background, or underline color); see [`foreground`],
/// [`background`], and [`underline_color`].
///
/// This enum is `#[non_exhaustive]`: terminals occasionally grow additional named colors or color
/// spaces, and qwertty does not want to promise this list is final.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Color {
    /// ANSI black (color 0).
    Black,
    /// ANSI red (color 1).
    Red,
    /// ANSI green (color 2).
    Green,
    /// ANSI yellow (color 3).
    Yellow,
    /// ANSI blue (color 4).
    Blue,
    /// ANSI magenta (color 5).
    Magenta,
    /// ANSI cyan (color 6).
    Cyan,
    /// ANSI white (color 7).
    White,
    /// Bright black / "bright" gray (color 8).
    BrightBlack,
    /// Bright red (color 9).
    BrightRed,
    /// Bright green (color 10).
    BrightGreen,
    /// Bright yellow (color 11).
    BrightYellow,
    /// Bright blue (color 12).
    BrightBlue,
    /// Bright magenta (color 13).
    BrightMagenta,
    /// Bright cyan (color 14).
    BrightCyan,
    /// Bright white (color 15).
    BrightWhite,
    /// A 256-color palette index.
    Indexed(u8),
    /// A 24-bit truecolor value.
    Rgb(u8, u8, u8),
}

impl Color {
    /// Returns the SGR base offset for the named 16-color slot (0-7 for standard colors, 8-15 for
    /// bright), or `None` for [`Color::Indexed`]/[`Color::Rgb`], which use the extended-color SGR
    /// parameters instead.
    const fn named_offset(self) -> Option<u8> {
        match self {
            Self::Black => Some(0),
            Self::Red => Some(1),
            Self::Green => Some(2),
            Self::Yellow => Some(3),
            Self::Blue => Some(4),
            Self::Magenta => Some(5),
            Self::Cyan => Some(6),
            Self::White => Some(7),
            Self::BrightBlack => Some(8),
            Self::BrightRed => Some(9),
            Self::BrightGreen => Some(10),
            Self::BrightYellow => Some(11),
            Self::BrightBlue => Some(12),
            Self::BrightMagenta => Some(13),
            Self::BrightCyan => Some(14),
            Self::BrightWhite => Some(15),
            Self::Indexed(_) | Self::Rgb(..) => None,
        }
    }

    /// Returns this color's 256-color palette index: a named color's ANSI number (0-15), or an
    /// [`Color::Indexed`] value's index directly.
    ///
    /// [`Color::Rgb`] has no palette index; callers that need to distinguish it should match on
    /// `Color::Rgb` before falling back to this method. It returns `0` for `Rgb` only so the
    /// method stays total; that branch is unreachable from every helper in this module because
    /// each one matches `Color::Rgb` first.
    const fn palette_index(self) -> u8 {
        match self.named_offset() {
            Some(offset) => offset,
            None => match self {
                Self::Indexed(index) => index,
                _ => 0,
            },
        }
    }

    /// Appends this color's SGR parameters for the given slot's base codes.
    ///
    /// `standard_base`/`bright_base` are the SGR base numbers for the 16 named colors (30/90 for
    /// foreground, 40/100 for background); `extended_selector` is the SGR parameter that
    /// introduces indexed/RGB colors for that slot (38 for foreground, 48 for background, 58 for
    /// underline color).
    fn push_params(
        self,
        out: &mut String,
        standard_base: u8,
        bright_base: u8,
        extended_selector: u8,
    ) {
        use std::fmt::Write as _;

        if let Some(offset) = self.named_offset() {
            let base = if offset < 8 {
                standard_base
            } else {
                bright_base
            };
            let code = base + (offset % 8);
            let _ = write!(out, "{code}");
            return;
        }

        match self {
            Self::Indexed(index) => {
                let _ = write!(out, "{extended_selector};5;{index}");
            }
            Self::Rgb(r, g, b) => {
                let _ = write!(out, "{extended_selector};2;{r};{g};{b}");
            }
            _ => unreachable!("named colors are handled by named_offset above"),
        }
    }
}

/// Sets the foreground (text) color.
///
/// Named colors emit the classic ECMA-48 SGR range `30`-`37` (standard) or the widely supported
/// xterm-derived `90`-`97` (bright), for example `CSI 31 m` for [`Color::Red`]. [`Color::Indexed`]
/// emits the semicolon 256-color form `CSI 38 ; 5 ; n m`; [`Color::Rgb`] emits the semicolon
/// truecolor form `CSI 38 ; 2 ; r ; g ; b m`. See the [module docs](self) for why the semicolon
/// form is used (FM-W6).
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style::{self, Color};
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::foreground(Color::Rgb(10, 20, 30)));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[38;2;10;20;30m");
/// ```
#[must_use]
pub fn foreground(color: Color) -> Command {
    let mut params = String::new();
    color.push_params(&mut params, 30, 90, 38);
    escape::csi(params, 'm')
}

/// Sets the background color.
///
/// Named colors emit ECMA-48 SGR range `40`-`47` (standard) or `100`-`107` (bright), for example
/// `CSI 41 m` for [`Color::Red`]. [`Color::Indexed`] emits `CSI 48 ; 5 ; n m`; [`Color::Rgb`]
/// emits `CSI 48 ; 2 ; r ; g ; b m`. See the [module docs](self) for why the semicolon form is
/// used (FM-W6).
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style::{self, Color};
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::background(Color::Indexed(214)));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[48;5;214m");
/// ```
#[must_use]
pub fn background(color: Color) -> Command {
    let mut params = String::new();
    color.push_params(&mut params, 40, 100, 48);
    escape::csi(params, 'm')
}

/// Sets the underline color, independent of the text foreground color.
///
/// This is SGR 58, a modern extension (Kitty, xterm, iTerm2, `WezTerm`) rather than an ECMA-48
/// control. Named [`Color`] variants are not defined for SGR 58 by any terminal (there is no
/// "named-color underline" convention), so this helper only emits the extended forms:
/// [`Color::Indexed`] as `CSI 58 ; 5 ; n m` and [`Color::Rgb`] as `CSI 58 ; 2 ; r ; g ; b m`. This
/// module deliberately emits the semicolon form, not the colon subparameter form (`58:2::r:g:b`)
/// that some terminals also accept: FM-W6 found non-default underline color has caused rendering
/// bugs (blinking) on Windows Terminal and hard failures on Windows 7 hosts serious enough that
/// crossterm feature-gates it, so this module favors the same widely-supported semicolon spelling
/// used everywhere else in this module over the newer colon form.
///
/// A named 16-color variant passed here still encodes as an indexed color (its ANSI color
/// number), since SGR 58 has no separate named-color range.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style::{self, Color};
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::underline_color(Color::Rgb(1, 2, 3)));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[58;2;1;2;3m");
/// ```
#[must_use]
pub fn underline_color(color: Color) -> Command {
    let params = match color {
        Color::Rgb(r, g, b) => format!("58;2;{r};{g};{b}"),
        other => format!("58;5;{}", other.palette_index()),
    };
    escape::csi(params, 'm')
}

/// Resets the foreground color to the terminal default.
///
/// This encodes SGR 39, emitted as `CSI 39 m`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reset_foreground());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[39m");
/// ```
#[must_use]
pub fn reset_foreground() -> Command {
    escape::csi("39", 'm')
}

/// Resets the background color to the terminal default.
///
/// This encodes SGR 49, emitted as `CSI 49 m`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reset_background());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[49m");
/// ```
#[must_use]
pub fn reset_background() -> Command {
    escape::csi("49", 'm')
}

/// Resets the underline color to the text foreground color.
///
/// This encodes SGR 59, emitted as `CSI 59 m`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reset_underline_color());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[59m");
/// ```
#[must_use]
pub fn reset_underline_color() -> Command {
    escape::csi("59", 'm')
}

/// Sets bold (increased intensity) text.
///
/// This encodes SGR 1, emitted as `CSI 1 m`. Pair with [`reset_bold_dim`], which resets both bold
/// and dim, since SGR only offers a combined reset for the two.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::bold());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[1m");
/// ```
#[must_use]
pub fn bold() -> Command {
    escape::csi("1", 'm')
}

/// Sets dim (decreased intensity) text.
///
/// This encodes SGR 2, emitted as `CSI 2 m`. Pair with [`reset_bold_dim`], which resets both bold
/// and dim, since SGR only offers a combined reset for the two.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::dim());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[2m");
/// ```
#[must_use]
pub fn dim() -> Command {
    escape::csi("2", 'm')
}

/// Sets italic text.
///
/// This encodes SGR 3, emitted as `CSI 3 m`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::italic());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[3m");
/// ```
#[must_use]
pub fn italic() -> Command {
    escape::csi("3", 'm')
}

/// Sets (single, straight) underlined text.
///
/// This encodes SGR 4, emitted as `CSI 4 m`. Use [`underline_style`] to select a specific
/// underline substyle (double, curly, dotted, dashed) instead of the plain default underline.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::underline());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[4m");
/// ```
#[must_use]
pub fn underline() -> Command {
    escape::csi("4", 'm')
}

/// Sets blinking text.
///
/// This encodes SGR 5, emitted as `CSI 5 m`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::blink());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[5m");
/// ```
#[must_use]
pub fn blink() -> Command {
    escape::csi("5", 'm')
}

/// Sets reverse (swap foreground and background) video.
///
/// This encodes SGR 7, emitted as `CSI 7 m`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reverse());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[7m");
/// ```
#[must_use]
pub fn reverse() -> Command {
    escape::csi("7", 'm')
}

/// Sets hidden (concealed) text.
///
/// This encodes SGR 8, emitted as `CSI 8 m`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::hidden());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[8m");
/// ```
#[must_use]
pub fn hidden() -> Command {
    escape::csi("8", 'm')
}

/// Sets strikethrough (crossed-out) text.
///
/// This encodes SGR 9, emitted as `CSI 9 m`.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::strikethrough());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[9m");
/// ```
#[must_use]
pub fn strikethrough() -> Command {
    escape::csi("9", 'm')
}

/// Resets both bold and dim (increased and decreased intensity).
///
/// This encodes SGR 22, emitted as `CSI 22 m`. SGR has no separate reset for bold versus dim; 22
/// always clears both, matching [`bold`] and [`dim`] as its undo.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reset_bold_dim());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[22m");
/// ```
#[must_use]
pub fn reset_bold_dim() -> Command {
    escape::csi("22", 'm')
}

/// Resets italic text.
///
/// This encodes SGR 23, emitted as `CSI 23 m`. Pairs with [`italic`].
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reset_italic());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[23m");
/// ```
#[must_use]
pub fn reset_italic() -> Command {
    escape::csi("23", 'm')
}

/// Resets underline, including any underline substyle set with [`underline_style`].
///
/// This encodes SGR 24, emitted as `CSI 24 m`. Pairs with [`underline`] and [`underline_style`].
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reset_underline());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[24m");
/// ```
#[must_use]
pub fn reset_underline() -> Command {
    escape::csi("24", 'm')
}

/// Resets blinking text.
///
/// This encodes SGR 25, emitted as `CSI 25 m`. Pairs with [`blink`].
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reset_blink());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[25m");
/// ```
#[must_use]
pub fn reset_blink() -> Command {
    escape::csi("25", 'm')
}

/// Resets reverse video.
///
/// This encodes SGR 27, emitted as `CSI 27 m`. Pairs with [`reverse`].
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reset_reverse());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[27m");
/// ```
#[must_use]
pub fn reset_reverse() -> Command {
    escape::csi("27", 'm')
}

/// Resets hidden (concealed) text.
///
/// This encodes SGR 28, emitted as `CSI 28 m`. Pairs with [`hidden`].
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reset_hidden());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[28m");
/// ```
#[must_use]
pub fn reset_hidden() -> Command {
    escape::csi("28", 'm')
}

/// Resets strikethrough text.
///
/// This encodes SGR 29, emitted as `CSI 29 m`. Pairs with [`strikethrough`].
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reset_strikethrough());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[29m");
/// ```
#[must_use]
pub fn reset_strikethrough() -> Command {
    escape::csi("29", 'm')
}

/// Resets all graphic rendition state to terminal defaults.
///
/// This encodes SGR 0, emitted as `CSI 0 m`. It clears every attribute and color set by this
/// module in one command, unlike the per-attribute resets above.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::reset_all());
///
/// assert_eq!(frame.as_bytes(), b"\x1b[0m");
/// ```
#[must_use]
pub fn reset_all() -> Command {
    escape::csi("0", 'm')
}

/// An SGR 4 underline substyle.
///
/// These map to SGR 4's colon subparameter (`4:0` through `4:5`), the only widely-implemented
/// encoding for underline substyles — there is no semicolon-form alternative in use. See the
/// [module docs](self) for the FM-W6 colon-vs-semicolon discussion.
///
/// This enum is `#[non_exhaustive]`: terminals occasionally add underline substyles beyond the
/// five Kitty/VTE-originated ones qwertty currently exposes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum UnderlineStyle {
    /// No underline (`4:0`).
    None,
    /// A plain, straight underline (`4:1`). Equivalent in appearance to plain SGR 4.
    Straight,
    /// A double underline (`4:2`).
    Double,
    /// A curly (wavy) underline (`4:3`), commonly used for spellcheck-style highlighting.
    Curly,
    /// A dotted underline (`4:4`).
    Dotted,
    /// A dashed underline (`4:5`).
    Dashed,
}

impl UnderlineStyle {
    /// Returns the SGR 4 colon subparameter value for this style.
    const fn subparam(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Straight => 1,
            Self::Double => 2,
            Self::Curly => 3,
            Self::Dotted => 4,
            Self::Dashed => 5,
        }
    }
}

/// Sets a specific underline substyle.
///
/// This encodes SGR 4 with a colon subparameter, `CSI 4 : n m`, for example `CSI 4 : 3 m` for
/// [`UnderlineStyle::Curly`]. The colon form is the only widely-implemented spelling for
/// underline substyles (see the [module docs](self)), unlike this module's colors, which use the
/// semicolon form. [`UnderlineStyle::None`] turns the underline off the same way
/// [`reset_underline`] does, but as the SGR 4 substyle form rather than SGR 24.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::style::{self, UnderlineStyle};
///
/// let mut frame = CommandBuffer::new();
/// frame.command(style::underline_style(UnderlineStyle::Curly));
///
/// assert_eq!(frame.as_bytes(), b"\x1b[4:3m");
/// ```
#[must_use]
pub fn underline_style(style: UnderlineStyle) -> Command {
    escape::csi(format!("4:{}", style.subparam()), 'm')
}

#[cfg(test)]
mod tests {
    use super::{Color, UnderlineStyle, background, foreground, underline_color, underline_style};
    use crate::{Command, SyntaxParser, SyntaxToken};

    /// Encodes `command` and asserts the bytes match exactly.
    fn assert_bytes(command: &Command, expected: &[u8]) {
        let mut bytes = Vec::new();
        command.encode(&mut bytes);
        assert_eq!(bytes, expected);
    }

    /// Asserts that `command`'s bytes parse back through `SyntaxParser` as exactly one CSI token
    /// with final byte `m`, proving the emitted bytes are well-formed SGR.
    fn assert_round_trips_as_sgr(command: &Command) {
        let mut bytes = Vec::new();
        command.encode(&mut bytes);

        let mut parser = SyntaxParser::new();
        let mut tokens = parser.feed(&bytes);
        tokens.extend(parser.finish());

        assert_eq!(tokens.len(), 1, "expected exactly one token from {bytes:?}");
        let SyntaxToken::Csi(csi) = &tokens[0] else {
            panic!("expected a CSI token from {bytes:?}, got {:?}", tokens[0]);
        };
        assert_eq!(csi.params().final_byte(), b'm');
        assert_eq!(csi.as_bytes(), bytes.as_slice());
    }

    #[test]
    fn foreground_rgb_bytes() {
        let command = foreground(Color::Rgb(10, 20, 30));
        assert_bytes(&command, b"\x1b[38;2;10;20;30m");
        assert_round_trips_as_sgr(&command);
    }

    #[test]
    fn foreground_indexed_bytes() {
        let command = foreground(Color::Indexed(214));
        assert_bytes(&command, b"\x1b[38;5;214m");
        assert_round_trips_as_sgr(&command);
    }

    #[test]
    fn background_rgb_bytes() {
        let command = background(Color::Rgb(1, 2, 3));
        assert_bytes(&command, b"\x1b[48;2;1;2;3m");
        assert_round_trips_as_sgr(&command);
    }

    #[test]
    fn background_indexed_bytes() {
        let command = background(Color::Indexed(21));
        assert_bytes(&command, b"\x1b[48;5;21m");
        assert_round_trips_as_sgr(&command);
    }

    #[test]
    fn underline_color_rgb_bytes() {
        let command = underline_color(Color::Rgb(1, 2, 3));
        assert_bytes(&command, b"\x1b[58;2;1;2;3m");
        assert_round_trips_as_sgr(&command);
    }

    #[test]
    fn underline_color_indexed_bytes() {
        let command = underline_color(Color::Indexed(9));
        assert_bytes(&command, b"\x1b[58;5;9m");
        assert_round_trips_as_sgr(&command);
    }

    #[test]
    fn underline_color_named_uses_semicolon_indexed_form() {
        // SGR 58 has no named-color range; a named Color still encodes as its ANSI number through
        // the indexed form (FM-W6: semicolon form throughout, no colon subparameter anywhere).
        let command = underline_color(Color::Red);
        assert_bytes(&command, b"\x1b[58;5;1m");
        assert_round_trips_as_sgr(&command);
    }

    #[test]
    fn attribute_commands_byte_exact() {
        type AttributeCase = (fn() -> Command, &'static [u8]);
        let cases: &[AttributeCase] = &[
            (super::bold, b"\x1b[1m"),
            (super::dim, b"\x1b[2m"),
            (super::italic, b"\x1b[3m"),
            (super::underline, b"\x1b[4m"),
            (super::blink, b"\x1b[5m"),
            (super::reverse, b"\x1b[7m"),
            (super::hidden, b"\x1b[8m"),
            (super::strikethrough, b"\x1b[9m"),
            (super::reset_bold_dim, b"\x1b[22m"),
            (super::reset_italic, b"\x1b[23m"),
            (super::reset_underline, b"\x1b[24m"),
            (super::reset_blink, b"\x1b[25m"),
            (super::reset_reverse, b"\x1b[27m"),
            (super::reset_hidden, b"\x1b[28m"),
            (super::reset_strikethrough, b"\x1b[29m"),
            (super::reset_all, b"\x1b[0m"),
            (super::reset_foreground, b"\x1b[39m"),
            (super::reset_background, b"\x1b[49m"),
            (super::reset_underline_color, b"\x1b[59m"),
        ];

        for (make, expected) in cases {
            let command = make();
            assert_bytes(&command, expected);
            assert_round_trips_as_sgr(&command);
        }
    }

    #[test]
    fn all_sixteen_named_colors_foreground_and_background() {
        let cases: &[(Color, u8)] = &[
            (Color::Black, 0),
            (Color::Red, 1),
            (Color::Green, 2),
            (Color::Yellow, 3),
            (Color::Blue, 4),
            (Color::Magenta, 5),
            (Color::Cyan, 6),
            (Color::White, 7),
            (Color::BrightBlack, 8),
            (Color::BrightRed, 9),
            (Color::BrightGreen, 10),
            (Color::BrightYellow, 11),
            (Color::BrightBlue, 12),
            (Color::BrightMagenta, 13),
            (Color::BrightCyan, 14),
            (Color::BrightWhite, 15),
        ];

        for &(color, offset) in cases {
            let (fg_base, bg_base) = if offset < 8 { (30, 40) } else { (90, 100) };
            let fg_code = fg_base + (offset % 8);
            let bg_code = bg_base + (offset % 8);

            let fg = foreground(color);
            assert_bytes(&fg, format!("\x1b[{fg_code}m").as_bytes());
            assert_round_trips_as_sgr(&fg);

            let bg = background(color);
            assert_bytes(&bg, format!("\x1b[{bg_code}m").as_bytes());
            assert_round_trips_as_sgr(&bg);
        }
    }

    #[test]
    fn underline_style_bytes() {
        let cases: &[(UnderlineStyle, &[u8])] = &[
            (UnderlineStyle::None, b"\x1b[4:0m"),
            (UnderlineStyle::Straight, b"\x1b[4:1m"),
            (UnderlineStyle::Double, b"\x1b[4:2m"),
            (UnderlineStyle::Curly, b"\x1b[4:3m"),
            (UnderlineStyle::Dotted, b"\x1b[4:4m"),
            (UnderlineStyle::Dashed, b"\x1b[4:5m"),
        ];

        for &(style, expected) in cases {
            let command = underline_style(style);
            assert_bytes(&command, expected);
            assert_round_trips_as_sgr(&command);
        }
    }
}
