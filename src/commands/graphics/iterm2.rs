//! [iTerm2 inline image] command helpers.
//!
//! iTerm2 (and terminals that adopted its protocol, notably `WezTerm`) display an image inline with
//! an OSC 1337 `File` command of the shape `OSC 1337 ; File=<key=value>;… : <base64> ST`: a
//! `;`-separated list of `key=value` arguments, a `:`, then the base64 of the image. These helpers
//! build that byte string and nothing else — no image encoding (the caller hands over an already
//! encoded PNG/JPEG/GIF; qwertty takes no image dependency), no capability check, and no policy
//! (see the [module docs](super) for where those obligations live).
//!
//! Only the **inline** form — `inline=1`, with the caller supplying the bytes — is built. iTerm2's
//! OSC 1337 also carries many non-image subcommands (`StealFocus`, `SetProfile`, clipboard, …);
//! this module emits only the image command. There is no support query in the protocol, so whether
//! a terminal renders these bytes is an identity-keyed session-layer concern, not something these
//! helpers check.
//!
//! [iTerm2 inline image]: https://iterm2.com/documentation-images.html
//!
//! ```
//! use qwertty::CommandBuffer;
//! use qwertty::commands::graphics::iterm2;
//!
//! let bytes = CommandBuffer::new()
//!     .command(iterm2::inline_image(b"\x00\x00\x00"))
//!     .as_bytes()
//!     .to_vec();
//! assert_eq!(bytes, b"\x1b]1337;File=inline=1:AAAA\x1b\\");
//! ```

use crate::commands::osc::encode_base64;
use crate::{Command, escape};

/// A width or height for an inline image — iTerm2's `width=`/`height=` argument value.
///
/// A dimension is expressed in one of four units the protocol defines. `Cells` is character cells;
/// `Pixels` is device pixels; `Percent` is a percentage of the terminal's width or height; `Auto`
/// lets the terminal choose from the image's own size and the other dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Dimension {
    /// A number of character cells (`N`).
    Cells(u32),
    /// A number of device pixels (`Npx`).
    Pixels(u32),
    /// A percentage of the terminal dimension (`N%`).
    Percent(u32),
    /// Let the terminal choose (`auto`).
    Auto,
}

impl Dimension {
    /// Renders the dimension as the protocol's argument value (`10`, `64px`, `50%`, or `auto`).
    fn value(self) -> String {
        match self {
            Self::Cells(n) => n.to_string(),
            Self::Pixels(n) => format!("{n}px"),
            Self::Percent(n) => format!("{n}%"),
            Self::Auto => "auto".to_string(),
        }
    }
}

/// Displays an image inline at the cursor, at its natural size.
///
/// The `image` bytes are an already-encoded image file (PNG, JPEG, GIF, …); they are base64-encoded
/// into the payload. The terminal sizes the image from its own contents. Use [`inline_image_sized`]
/// to constrain the width and height.
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::graphics::iterm2;
///
/// let bytes = CommandBuffer::new()
///     .command(iterm2::inline_image(b"\x00\x00\x00"))
///     .as_bytes()
///     .to_vec();
/// assert_eq!(bytes, b"\x1b]1337;File=inline=1:AAAA\x1b\\");
/// ```
#[must_use]
pub fn inline_image(image: &[u8]) -> Command {
    escape::osc(format!("1337;File=inline=1:{}", encode_base64(image)))
}

/// Displays an image inline at the cursor, constrained to `width` by `height`.
///
/// The dimensions are the protocol's `width=`/`height=` arguments (see [`Dimension`]). The image's
/// aspect ratio is preserved by the terminal's default; a caller that needs to stretch to both
/// dimensions can build the raw command with a lower-level escape, which this narrow helper
/// deliberately does not expose.
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::graphics::iterm2::{self, Dimension};
///
/// let bytes = CommandBuffer::new()
///     .command(iterm2::inline_image_sized(
///         b"\x00\x00\x00",
///         Dimension::Cells(10),
///         Dimension::Percent(50),
///     ))
///     .as_bytes()
///     .to_vec();
/// assert_eq!(
///     bytes,
///     b"\x1b]1337;File=inline=1;width=10;height=50%:AAAA\x1b\\"
/// );
/// ```
#[must_use]
pub fn inline_image_sized(image: &[u8], width: Dimension, height: Dimension) -> Command {
    escape::osc(format!(
        "1337;File=inline=1;width={};height={}:{}",
        width.value(),
        height.value(),
        encode_base64(image)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CommandBuffer;

    /// Renders a command to its wire bytes the way a caller would.
    fn bytes(command: Command) -> Vec<u8> {
        CommandBuffer::new().command(command).as_bytes().to_vec()
    }

    #[test]
    fn inline_image_wraps_base64_in_the_file_command() {
        // OSC 1337 ; File=inline=1 : <base64> ST, per the cited iTerm2 Inline Images Protocol.
        // AAAA is base64 of three zero bytes.
        assert_eq!(
            bytes(inline_image(b"\x00\x00\x00")),
            b"\x1b]1337;File=inline=1:AAAA\x1b\\"
        );
    }

    #[test]
    fn sized_image_carries_width_and_height_arguments() {
        assert_eq!(
            bytes(inline_image_sized(
                b"\x00\x00\x00",
                Dimension::Cells(10),
                Dimension::Percent(50)
            )),
            b"\x1b]1337;File=inline=1;width=10;height=50%:AAAA\x1b\\"
        );
    }

    #[test]
    fn every_dimension_unit_renders() {
        let out = bytes(inline_image_sized(
            b"",
            Dimension::Pixels(64),
            Dimension::Auto,
        ));
        assert_eq!(
            out,
            b"\x1b]1337;File=inline=1;width=64px;height=auto:\x1b\\"
        );
    }

    #[test]
    fn large_payload_encodes_without_panicking() {
        let out = bytes(inline_image(&vec![0xABu8; 4096]));
        assert!(out.starts_with(b"\x1b]1337;File=inline=1:"));
        assert!(out.ends_with(b"\x1b\\"));
    }
}
