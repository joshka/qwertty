//! [kitty graphics protocol] command helpers.
//!
//! The kitty graphics protocol carries images in Application Program Command sequences of the
//! shape `ESC _ G <control-keys> ; <base64-payload> ESC \`: a `G`-prefixed comma-separated list of
//! `key=value` control keys, a `;`, then the base64 of the image (or an empty payload for a
//! control-only command). These helpers build that byte string and nothing else — no image
//! encoding (the caller hands over already-encoded PNG/RGB/RGBA bytes; qwertty takes no image
//! dependency), no capability check, and no policy (see the [module docs](super) for where those
//! obligations live).
//!
//! Only the **inline** transmission form (control key `t=d`, the default — the caller supplies the
//! bytes) is built here. The file, temp-file, and shared-memory transmission forms, where the
//! escape names a resource the terminal opens, are a policy-gated session-layer concern and are
//! deliberately not in this module.
//!
//! [kitty graphics protocol]: https://sw.kovidgoyal.net/kitty/graphics-protocol/
//!
//! # Image ids
//!
//! [`place`] and [`delete_image`] act on a client-assigned image id: the number a program chooses
//! when it transmits an image and then reuses to refer to it. qwertty does not allocate or track
//! ids — the application owns that id space (there is no hidden registry), exactly as the protocol
//! intends.

use crate::commands::osc::encode_base64;
use crate::{Command, escape};

/// The pixel format of an image payload — the kitty protocol's `f=` control key.
///
/// The three values the protocol defines for inline transmission. `Rgb`/`Rgba` are raw
/// little-endian pixel bytes (3 or 4 per pixel); `Png` is a complete PNG file, which the terminal
/// decodes itself, so it needs no separate width/height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Format {
    /// Raw 24-bit RGB pixels (`f=24`).
    Rgb,
    /// Raw 32-bit RGBA pixels (`f=32`).
    Rgba,
    /// A complete PNG image (`f=100`).
    Png,
}

impl Format {
    /// The numeric `f=` control value the protocol assigns this format.
    fn code(self) -> u16 {
        match self {
            Self::Rgb => 24,
            Self::Rgba => 32,
            Self::Png => 100,
        }
    }
}

/// Builds one kitty graphics APC command from its control keys and (possibly empty) payload.
///
/// Every helper funnels through here so the `G` prefix, the `;` control/payload separator, and the
/// APC envelope are constructed one way. The separator is always emitted — a control-only command
/// carries an empty payload after it — matching the pinned `db/kitty-graphics` fixtures.
fn command(control: &str, payload: &str) -> Command {
    escape::apc(format!("G{control};{payload}"))
}

/// Transmits an image and displays it at the cursor in one command (`a=T`).
///
/// The `image` bytes are the already-encoded pixels or PNG for `format`; they are base64-encoded
/// into the payload. This is the fire-and-show path: the terminal assigns the image no client id,
/// so it cannot later be referred to by [`place`] or [`delete_image`].
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::graphics::kitty::{self, Format};
///
/// let bytes = CommandBuffer::new()
///     .command(kitty::transmit_and_display(Format::Png, b"\x00\x00\x00"))
///     .as_bytes()
///     .to_vec();
/// assert_eq!(bytes, b"\x1b_Ga=T,f=100;AAAA\x1b\\");
/// ```
#[must_use]
pub fn transmit_and_display(format: Format, image: &[u8]) -> Command {
    command(&format!("a=T,f={}", format.code()), &encode_base64(image))
}

/// Places an already-transmitted image, by its client-assigned id, at the cursor (`a=p`).
///
/// `image_id` is the id the image was transmitted with. Placing an id that was never transmitted
/// is a no-op at the terminal, not an error here — this layer only encodes bytes.
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::graphics::kitty;
///
/// let bytes = CommandBuffer::new()
///     .command(kitty::place(7))
///     .as_bytes()
///     .to_vec();
/// assert_eq!(bytes, b"\x1b_Ga=p,i=7;\x1b\\");
/// ```
#[must_use]
pub fn place(image_id: u32) -> Command {
    command(&format!("a=p,i={image_id}"), "")
}

/// Deletes all images and placements the terminal is holding (`a=d`).
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::graphics::kitty;
///
/// let bytes = CommandBuffer::new()
///     .command(kitty::delete_all_images())
///     .as_bytes()
///     .to_vec();
/// assert_eq!(bytes, b"\x1b_Ga=d;\x1b\\");
/// ```
#[must_use]
pub fn delete_all_images() -> Command {
    command("a=d", "")
}

/// Deletes a single image by its client-assigned id (`a=d,d=i,i=<id>`).
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::graphics::kitty;
///
/// let bytes = CommandBuffer::new()
///     .command(kitty::delete_image(7))
///     .as_bytes()
///     .to_vec();
/// assert_eq!(bytes, b"\x1b_Ga=d,d=i,i=7;\x1b\\");
/// ```
#[must_use]
pub fn delete_image(image_id: u32) -> Command {
    command(&format!("a=d,d=i,i={image_id}"), "")
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
    fn transmit_and_display_reproduces_the_audited_fixture() {
        // fixtures/kitty/graphics_transmit_display.seq (origin=prototype:audited-2026-07-06):
        // `ESC _ G a=T,f=100 ; AAAA ESC \`, where AAAA is base64 of three zero bytes.
        assert_eq!(
            bytes(transmit_and_display(Format::Png, b"\x00\x00\x00")),
            b"\x1b_Ga=T,f=100;AAAA\x1b\\"
        );
    }

    #[test]
    fn place_reproduces_the_audited_fixture() {
        // fixtures/kitty/graphics_place.seq (origin=prototype:audited-2026-07-06).
        assert_eq!(bytes(place(7)), b"\x1b_Ga=p,i=7;\x1b\\");
    }

    #[test]
    fn format_codes_match_the_spec() {
        assert_eq!(
            bytes(transmit_and_display(Format::Rgb, &[])),
            b"\x1b_Ga=T,f=24;\x1b\\"
        );
        assert_eq!(
            bytes(transmit_and_display(Format::Rgba, &[])),
            b"\x1b_Ga=T,f=32;\x1b\\"
        );
    }

    #[test]
    fn delete_forms_match_the_spec() {
        assert_eq!(bytes(delete_all_images()), b"\x1b_Ga=d;\x1b\\");
        assert_eq!(bytes(delete_image(7)), b"\x1b_Ga=d,d=i,i=7;\x1b\\");
    }

    #[test]
    fn large_ids_and_payloads_encode_without_panicking() {
        let big = vec![0xABu8; 4096];
        let out = bytes(transmit_and_display(Format::Rgba, &big));
        assert!(out.starts_with(b"\x1b_Ga=T,f=32;"));
        assert!(out.ends_with(b"\x1b\\"));
        assert_eq!(
            bytes(delete_image(u32::MAX)),
            b"\x1b_Ga=d,d=i,i=4294967295;\x1b\\"
        );
    }
}
