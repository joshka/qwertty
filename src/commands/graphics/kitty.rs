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
//! Payloads larger than the protocol's 4096-byte chunk bound are split automatically into a
//! chunked transmission (`m=1` continuations, `m=0` final chunk), as the spec requires; small
//! payloads stay a single escape.
//!
//! [kitty graphics protocol]: https://sw.kovidgoyal.net/kitty/graphics-protocol/
//!
//! # Image ids
//!
//! [`place`] and [`delete_image`] act on a client-assigned image id: the number a program chooses
//! when it transmits an image and then reuses to refer to it. qwertty does not allocate or track
//! ids — the application owns that id space (there is no hidden registry), exactly as the protocol
//! intends. Ids are "an arbitrary positive integer up to 4294967295, it must not be zero" (spec,
//! "Querying support"); the terminal answers every id-carrying command by echoing the id in an
//! `APC G i=<id> ; OK ST` acknowledgement (or an error), which decodes as a
//! [`KittyGraphicsReport`](crate::report::KittyGraphicsReport).
//!
//! # Resource-naming transmission is policy-gated
//!
//! [`transmit_file`], [`transmit_temp_file`], and [`transmit_shared_memory`] build the
//! transmission forms whose escape names a **resource the terminal itself opens** — a
//! local-file-read primitive. Like every command builder they only build bytes; the session-level
//! emits ([`TerminalSession::transmit_kitty_file`](crate::TerminalSession::transmit_kitty_file)
//! and siblings) apply the [`Policy`](crate::Policy) file-transfer gate and should be preferred.

use crate::commands::osc::encode_base64;
use crate::{Command, escape};

/// The maximum size of one chunk of base64 payload within a single APC escape.
///
/// The protocol requires the base64-encoded data to be "chunked up into chunks no larger than
/// 4096 bytes", every chunk but the last a multiple of 4 (spec, "Remote client"). 4096 is itself
/// a multiple of 4, so splitting at exactly this size satisfies both rules.
const CHUNK_LIMIT: usize = 4096;

/// The pixel format of an image payload — the kitty protocol's `f=` control key.
///
/// The three values the protocol defines for inline transmission. `Rgb`/`Rgba` are raw
/// little-endian pixel bytes (3 or 4 per pixel); `Png` is a complete PNG file, which the terminal
/// decodes itself, so it needs no separate width/height. When transmitting the raw formats, the
/// protocol requires the image dimensions in the control data — pass an [`ImageSize`] to
/// [`transmit`] and the file-naming helpers for those.
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

/// Pixel dimensions of raw image data — the protocol's `s=` (width) and `v=` (height) keys.
///
/// The raw formats ([`Format::Rgb`], [`Format::Rgba`]) carry no dimensions in their bytes, so the
/// protocol requires them in the control data; PNG carries its own and needs none. Pass
/// `Some(ImageSize { .. })` to [`transmit`] and the file-naming helpers for raw data, `None` for
/// PNG.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ImageSize {
    /// Image width in pixels (control key `s`).
    pub width: u32,
    /// Image height in pixels (control key `v`).
    pub height: u32,
}

/// Layout options for displaying an image with [`place_with`].
///
/// The default placement shows the whole image at the cursor, unscaled, at z-index 0, without a
/// placement id. Every option is additive:
///
/// ```
/// use qwertty::commands::graphics::kitty::Placement;
///
/// let placement = Placement::new().id(3).columns(20).rows(10).z_index(-1);
/// ```
///
/// A *placement* is one on-screen display of a transmitted image; the pair of image id and
/// placement id uniquely identifies it, and both id spaces are caller-owned. This struct is
/// non-exhaustive by construction (private fields, builder methods) so later protocol keys —
/// pixel offsets, source rectangles, cursor-movement policy — can join without breaking callers.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Placement {
    id: Option<u32>,
    columns: Option<u32>,
    rows: Option<u32>,
    z_index: Option<i32>,
}

impl Placement {
    /// Creates the default placement: the whole image at the cursor, no placement id.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            id: None,
            columns: None,
            rows: None,
            z_index: None,
        }
    }

    /// Sets the placement id (`p=…`, nonzero), letting later commands address this placement.
    #[must_use]
    pub const fn id(mut self, id: u32) -> Self {
        self.id = Some(id);
        self
    }

    /// Sets the number of columns to display the image over (`c=…`); the terminal scales to fit.
    ///
    /// When only one of columns and rows is set, the terminal computes the other from the image's
    /// aspect ratio.
    #[must_use]
    pub const fn columns(mut self, columns: u32) -> Self {
        self.columns = Some(columns);
        self
    }

    /// Sets the number of rows to display the image over (`r=…`); the terminal scales to fit.
    #[must_use]
    pub const fn rows(mut self, rows: u32) -> Self {
        self.rows = Some(rows);
        self
    }

    /// Sets the z-index stacking order (`z=…`).
    ///
    /// Negative values draw the image under the text, letting text render on top of it.
    #[must_use]
    pub const fn z_index(mut self, z_index: i32) -> Self {
        self.z_index = Some(z_index);
        self
    }

    /// Appends this placement's control keys to `control`.
    fn push_keys(self, control: &mut String) {
        use core::fmt::Write;
        if let Some(id) = self.id {
            write!(control, ",p={id}").expect("writing to String");
        }
        if let Some(columns) = self.columns {
            write!(control, ",c={columns}").expect("writing to String");
        }
        if let Some(rows) = self.rows {
            write!(control, ",r={rows}").expect("writing to String");
        }
        if let Some(z_index) = self.z_index {
            write!(control, ",z={z_index}").expect("writing to String");
        }
    }
}

/// Builds one or more kitty graphics APC escapes from control keys and a (possibly empty) payload.
///
/// Every helper funnels through here so the `G` prefix, the `;` control/payload separator, and the
/// APC envelope are constructed one way. The separator is always emitted — a control-only command
/// carries an empty payload after it — matching the pinned `db/kitty-graphics` fixtures. A payload
/// that exceeds the protocol's per-escape bound becomes a chunked transmission: the first escape
/// carries the control keys plus `m=1`, continuations carry **only** the `m` key (the spec forbids
/// other keys on continuations), and the final chunk carries `m=0`.
fn command(control: &str, payload: &str) -> Command {
    if payload.len() <= CHUNK_LIMIT {
        return escape::apc(format!("G{control};{payload}"));
    }

    let mut bytes = Vec::new();
    let mut chunks = payload.as_bytes().chunks(CHUNK_LIMIT).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = chunks.peek().is_some();
        let m = if more { "m=1" } else { "m=0" };
        let mut body = String::with_capacity(1 + control.len() + 8 + chunk.len());
        body.push('G');
        if first {
            body.push_str(control);
            body.push(',');
            first = false;
        }
        body.push_str(m);
        body.push(';');
        body.push_str(std::str::from_utf8(chunk).expect("base64 payload is ASCII"));
        escape::apc(body).encode(&mut bytes);
    }
    Command::raw(bytes)
}

/// Transmits an image and displays it at the cursor in one command (`a=T`).
///
/// The `image` bytes are the already-encoded pixels or PNG for `format`; they are base64-encoded
/// into the payload. This is the fire-and-show path: the terminal assigns the image no client id,
/// so it cannot later be referred to by [`place`] or [`delete_image`], and sends no
/// acknowledgement. Use [`transmit`] plus [`place`] for the id-carrying, acknowledged flow.
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

/// Transmits an image under a client-assigned id, without displaying it (`a=t`).
///
/// The terminal stores the data under `image_id` (nonzero) for later [`place`] / [`place_with`]
/// commands, and acknowledges the transmission with `APC G i=<id> ; OK ST` (or an error) — the
/// [`KittyGraphicsReport`](crate::report::KittyGraphicsReport) shape, echoing the id. Raw formats
/// need the image dimensions in `size` (the `s=`/`v=` keys); PNG carries its own, so pass `None`.
///
/// This encodes `db/kitty-graphics.toml`'s `kitty.graphics.transmit`.
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::graphics::kitty::{self, Format, ImageSize};
///
/// // One black RGB pixel as image 1.
/// let bytes = CommandBuffer::new()
///     .command(kitty::transmit(
///         1,
///         Format::Rgb,
///         Some(ImageSize {
///             width: 1,
///             height: 1,
///         }),
///         &[0, 0, 0],
///     ))
///     .as_bytes()
///     .to_vec();
/// assert_eq!(bytes, b"\x1b_Ga=t,i=1,f=24,s=1,v=1;AAAA\x1b\\");
/// ```
#[must_use]
pub fn transmit(image_id: u32, format: Format, size: Option<ImageSize>, image: &[u8]) -> Command {
    let control = transmit_control(image_id, format, size, None);
    command(&control, &encode_base64(image))
}

/// Places an already-transmitted image, by its client-assigned id, at the cursor (`a=p`).
///
/// `image_id` is the id the image was transmitted with. Placing an id that was never transmitted
/// is answered by the terminal with an `ENOENT:…` error (which is how an application discovers it
/// must re-transmit), not an error here — this layer only encodes bytes.
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

/// Places an already-transmitted image with explicit layout options (`a=p` plus display keys).
///
/// Like [`place`], with a [`Placement`] carrying the optional placement id, column/row scaling,
/// and z-index. One image can have any number of placements; a placement with an id can later be
/// deleted individually with [`delete_placement`].
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::graphics::kitty::{self, Placement};
///
/// let bytes = CommandBuffer::new()
///     .command(kitty::place_with(7, &Placement::new().id(3).z_index(-1)))
///     .as_bytes()
///     .to_vec();
/// assert_eq!(bytes, b"\x1b_Ga=p,i=7,p=3,z=-1;\x1b\\");
/// ```
#[must_use]
pub fn place_with(image_id: u32, placement: &Placement) -> Command {
    let mut control = format!("a=p,i={image_id}");
    placement.push_keys(&mut control);
    command(&control, "")
}

/// Deletes all images and placements the terminal is holding (`a=d`).
///
/// This is the placement-clearing form: the terminal may keep transmitted image data for
/// re-display. [`delete_all_images_and_data`] also frees the stored data.
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

/// Deletes all placements visible on screen and frees their stored image data (`a=d,d=A`).
///
/// The uppercase form of [`delete_all_images`]: the protocol's capital delete values also free
/// the transmitted data, so the images cannot be re-displayed without re-transmission.
#[must_use]
pub fn delete_all_images_and_data() -> Command {
    command("a=d,d=A", "")
}

/// Deletes a single image by its client-assigned id (`a=d,d=i,i=<id>`).
///
/// This is the placement-clearing form — the terminal may keep the transmitted data so the image
/// can be re-displayed without re-transmission. [`delete_image_and_data`] also frees the data.
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

/// Deletes a single image by id and frees its stored data (`a=d,d=I,i=<id>`).
///
/// The uppercase form of [`delete_image`] (spec's delete table): placements are removed *and* the
/// transmitted data is freed, provided the image is not still referenced elsewhere (for example
/// in the scrollback).
#[must_use]
pub fn delete_image_and_data(image_id: u32) -> Command {
    command(&format!("a=d,d=I,i={image_id}"), "")
}

/// Deletes one placement of one image, keeping the image data (`a=d,d=i,i=<id>,p=<pid>`).
///
/// The image id / placement id pair names exactly one placement (the ids given at [`transmit`]
/// and [`place_with`] time); other placements of the same image stay on screen.
#[must_use]
pub fn delete_placement(image_id: u32, placement_id: u32) -> Command {
    command(&format!("a=d,d=i,i={image_id},p={placement_id}"), "")
}

/// Queries whether the terminal speaks the graphics protocol (`a=q`).
///
/// This emits the probe the protocol spec itself recommends: a query-action transmission of a
/// single black RGB pixel, `APC G i=<id>,s=1,v=1,a=q,t=d,f=24 ; AAAA ST` (the spec's own example,
/// verbatim but for the caller's id). A terminal that supports the protocol must reply
/// immediately with `APC G i=<id> ; OK ST` (or an error); the query action stores nothing and
/// replaces nothing, so any nonzero id is safe to use. A terminal that does not support the
/// protocol ignores the APC entirely — which is why the spec pairs this query with a trailing
/// Primary Device Attributes request as a fence, exactly the pattern the Tokio session's
/// `probe_capabilities` applies: a DA1 answer without a graphics answer means silence, recorded
/// as *unknown*, never as proof of absence.
///
/// This encodes `db/kitty-graphics.toml`'s `kitty.graphics.query_support`.
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::graphics::kitty;
///
/// let bytes = CommandBuffer::new()
///     .command(kitty::query_support(31))
///     .as_bytes()
///     .to_vec();
/// assert_eq!(bytes, b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\");
/// ```
#[must_use]
pub fn query_support(image_id: u32) -> Command {
    // The spec's canonical probe: key order follows the spec example so the emitted bytes match
    // the cited fixture exactly ("AAAA" is base64 for three zero bytes — one black RGB pixel).
    command(&format!("i={image_id},s=1,v=1,a=q,t=d,f=24"), "AAAA")
}

/// Transmits an image by naming a **file the terminal opens and reads** (`t=f`).
///
/// The escape's payload is the base64-encoded file path; the terminal — not the application —
/// opens that path and reads the pixel data from it. `path` is passed as raw bytes because the
/// path is interpreted by the *terminal's* host, which over ssh is not the application's host.
/// Raw formats need `size` exactly as in [`transmit`]; PNG files carry their own dimensions.
///
/// # Security (FM-X4): this escape is a local-file-read primitive
///
/// Any output that reaches the terminal can carry this escape, and the file it names is opened
/// with the terminal's privileges on the terminal's machine: a malicious payload can steer a
/// supporting terminal into reading `/etc/passwd`, a key file, or any other readable path and
/// rendering it — with an id, the acknowledgement even confirms whether the read succeeded. This
/// is exactly the resource-naming exfiltration class the [`Policy`](crate::Policy) file-transfer
/// gate exists for, so the session-level emit
/// ([`TerminalSession::transmit_kitty_file`](crate::TerminalSession::transmit_kitty_file))
/// consults that gate and refuses with a typed [`PolicyDenied`](crate::Error::PolicyDenied) under
/// the default [`Policy::restricted`](crate::Policy::restricted). This builder, like every
/// command builder, only builds bytes; prefer the session method so the gate is applied.
///
/// This encodes `db/kitty-graphics.toml`'s `kitty.graphics.transmit_file`.
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::graphics::kitty::{self, Format};
///
/// let bytes = CommandBuffer::new()
///     .command(kitty::transmit_file(
///         5,
///         Format::Png,
///         None,
///         b"/tmp/image.png",
///     ))
///     .as_bytes()
///     .to_vec();
/// assert_eq!(bytes, b"\x1b_Ga=t,i=5,f=100,t=f;L3RtcC9pbWFnZS5wbmc=\x1b\\");
/// ```
#[must_use]
pub fn transmit_file(
    image_id: u32,
    format: Format,
    size: Option<ImageSize>,
    path: &[u8],
) -> Command {
    let control = transmit_control(image_id, format, size, Some('f'));
    command(&control, &encode_base64(path))
}

/// Transmits an image by naming a **temporary file the terminal reads and deletes** (`t=t`).
///
/// Like [`transmit_file`], but the terminal deletes the file after reading it — provided the file
/// sits in a known temporary directory and its full path contains the string
/// `tty-graphics-protocol` (the spec's consent rule for deletion); put both in place when
/// creating the file.
///
/// # Security (FM-X4)
///
/// Same resource-naming read primitive as [`transmit_file`], with deletion on top. The
/// session-level emit
/// ([`TerminalSession::transmit_kitty_temp_file`](crate::TerminalSession::transmit_kitty_temp_file))
/// sits behind the [`Policy`](crate::Policy) file-transfer gate.
///
/// This encodes `db/kitty-graphics.toml`'s `kitty.graphics.transmit_temp_file`.
#[must_use]
pub fn transmit_temp_file(
    image_id: u32,
    format: Format,
    size: Option<ImageSize>,
    path: &[u8],
) -> Command {
    let control = transmit_control(image_id, format, size, Some('t'));
    command(&control, &encode_base64(path))
}

/// Transmits an image by naming a **shared-memory object the terminal opens** (`t=s`).
///
/// The payload is the base64-encoded name of a POSIX shared-memory object (`shm_open` name); the
/// terminal reads the pixel data from it, then unlinks and closes it. Raw formats need `size`
/// exactly as in [`transmit`].
///
/// # Security (FM-X4)
///
/// Naming an IPC object the terminal opens is the same resource-naming class as
/// [`transmit_file`]; the session-level emit
/// ([`TerminalSession::transmit_kitty_shared_memory`](crate::TerminalSession::transmit_kitty_shared_memory))
/// sits behind the [`Policy`](crate::Policy) file-transfer gate.
///
/// This encodes `db/kitty-graphics.toml`'s `kitty.graphics.transmit_shared_memory`.
#[must_use]
pub fn transmit_shared_memory(
    image_id: u32,
    format: Format,
    size: Option<ImageSize>,
    name: &[u8],
) -> Command {
    let control = transmit_control(image_id, format, size, Some('s'));
    command(&control, &encode_base64(name))
}

/// Builds the control keys shared by every `a=t` transmission: `a=t,i=<id>,f=<code>` plus the
/// optional raw-format dimensions and the transmission-medium key (`t=` is omitted for direct
/// transmission, the protocol default).
fn transmit_control(
    image_id: u32,
    format: Format,
    size: Option<ImageSize>,
    medium: Option<char>,
) -> String {
    use core::fmt::Write;
    let mut control = format!("a=t,i={image_id},f={}", format.code());
    if let Some(ImageSize { width, height }) = size {
        write!(control, ",s={width},v={height}").expect("writing to String");
    }
    if let Some(medium) = medium {
        write!(control, ",t={medium}").expect("writing to String");
    }
    control
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
        assert_eq!(bytes(delete_all_images_and_data()), b"\x1b_Ga=d,d=A;\x1b\\");
        assert_eq!(bytes(delete_image_and_data(7)), b"\x1b_Ga=d,d=I,i=7;\x1b\\");
        assert_eq!(
            bytes(delete_placement(10, 7)),
            b"\x1b_Ga=d,d=i,i=10,p=7;\x1b\\"
        );
    }

    #[test]
    fn query_support_matches_spec_example() {
        // The spec's own probe example, id 31: `<ESC>_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA<ESC>\`.
        assert_eq!(
            bytes(query_support(31)),
            b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\"
        );
    }

    #[test]
    fn transmit_encodes_id_format_and_dimensions() {
        assert_eq!(
            bytes(transmit(
                1,
                Format::Rgb,
                Some(ImageSize {
                    width: 1,
                    height: 1,
                }),
                &[0, 0, 0],
            )),
            b"\x1b_Ga=t,i=1,f=24,s=1,v=1;AAAA\x1b\\"
        );
        // PNG needs no dimensions.
        assert_eq!(
            bytes(transmit(2, Format::Png, None, b"x")),
            b"\x1b_Ga=t,i=2,f=100;eA==\x1b\\"
        );
    }

    #[test]
    fn place_with_encodes_all_placement_keys() {
        assert_eq!(
            bytes(place_with(
                7,
                &Placement::new().id(3).columns(20).rows(10).z_index(-1),
            )),
            b"\x1b_Ga=p,i=7,p=3,c=20,r=10,z=-1;\x1b\\"
        );
        // A default placement is exactly the `place` form.
        assert_eq!(bytes(place_with(7, &Placement::new())), bytes(place(7)));
    }

    #[test]
    fn resource_transmissions_encode_their_medium() {
        assert_eq!(
            bytes(transmit_file(5, Format::Png, None, b"/tmp/image.png")),
            b"\x1b_Ga=t,i=5,f=100,t=f;L3RtcC9pbWFnZS5wbmc=\x1b\\"
        );
        assert_eq!(
            bytes(transmit_temp_file(5, Format::Png, None, b"/tmp/x")),
            b"\x1b_Ga=t,i=5,f=100,t=t;L3RtcC94\x1b\\"
        );
        assert_eq!(
            bytes(transmit_shared_memory(
                5,
                Format::Rgb,
                Some(ImageSize {
                    width: 10,
                    height: 2,
                }),
                b"/shm-name",
            )),
            b"\x1b_Ga=t,i=5,f=24,s=10,v=2,t=s;L3NobS1uYW1l\x1b\\"
        );
    }

    #[test]
    fn large_payloads_chunk_per_the_protocol() {
        // 3 * 4096 raw bytes encode to exactly 4 * 4096 base64 bytes: four full chunks.
        let data = vec![0u8; 3 * 4096];
        let out = bytes(transmit(1, Format::Png, None, &data));

        let text = String::from_utf8(out).expect("ASCII escapes");
        let escapes: Vec<&str> = text
            .split("\x1b\\")
            .filter(|part| !part.is_empty())
            .collect();
        assert_eq!(escapes.len(), 4, "4 * 4096 base64 bytes -> four chunks");

        // First chunk: full control data plus m=1; continuations carry only the m key.
        assert!(escapes[0].starts_with("\x1b_Ga=t,i=1,f=100,m=1;"));
        assert!(escapes[1].starts_with("\x1b_Gm=1;"));
        assert!(escapes[2].starts_with("\x1b_Gm=1;"));
        assert!(escapes[3].starts_with("\x1b_Gm=0;"));

        // Every chunk's payload is within the protocol bound, non-final chunks a multiple of 4,
        // and reassembly reproduces the whole base64 string.
        let mut reassembled = String::new();
        for (index, escape_text) in escapes.iter().enumerate() {
            let payload = escape_text
                .split_once(';')
                .expect("chunk has a payload separator")
                .1;
            assert!(payload.len() <= CHUNK_LIMIT, "chunk {index} within bound");
            if index + 1 != escapes.len() {
                assert_eq!(
                    payload.len() % 4,
                    0,
                    "non-final chunk {index} multiple of 4"
                );
            }
            reassembled.push_str(payload);
        }
        assert_eq!(reassembled, encode_base64(&data));
    }

    #[test]
    fn large_ids_and_payloads_encode_without_panicking() {
        // A payload past the chunk bound splits rather than emitting one oversized escape.
        let big = vec![0xABu8; 4096];
        let out = bytes(transmit_and_display(Format::Rgba, &big));
        assert!(out.starts_with(b"\x1b_Ga=T,f=32,m=1;"));
        assert!(out.ends_with(b"\x1b\\"));
        assert_eq!(
            bytes(delete_image(u32::MAX)),
            b"\x1b_Ga=d,d=i,i=4294967295;\x1b\\"
        );
    }
}
