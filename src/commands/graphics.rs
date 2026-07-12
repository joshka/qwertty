//! Inline-image (graphics) command helpers.
//!
//! Terminals that support an image protocol let a program paint pixels into the character grid.
//! qwertty exposes each protocol as its own submodule of typed, encode-only helpers — one protocol
//! per submodule rather than a lowest-common-denominator abstraction, because the protocols differ
//! in real ways (placement, deletion, animation, and whether the terminal answers) that a unifying
//! type would erase. See `work/phase2/design/11-graphics.md` for the full surface, capability, and
//! policy design.
//!
//! Currently one protocol is implemented:
//!
//! - [`kitty`] — the [kitty graphics protocol]: the most capable and the only one that answers a
//!   support query, so it is the first target.
//!
//! [kitty graphics protocol]: https://sw.kovidgoyal.net/kitty/graphics-protocol/
//!
//! # These helpers do not gate anything
//!
//! Like every [`commands`](crate::commands) helper, a graphics helper returns a
//! [`Command`](crate::Command) of raw bytes built without a terminal, session, decoder, or
//! [`Policy`](crate::Policy). Two obligations live *above* this layer, in whatever session code
//! forwards the bytes to a real terminal:
//!
//! - **Capability gating.** Emit an image protocol only into a terminal that supports it — for
//!   kitty, one confirmed by the support probe (a session concern; sending image bytes to a
//!   terminal that cannot render them prints garbage). This module cannot and does not check.
//! - **Transmission policy.** These helpers only build the *inline* transmission form, where the
//!   caller supplies the image bytes and the escape opens no new resource. The kitty protocol also
//!   defines file, temp-file, and shared-memory transmission, where the escape names a path the
//!   terminal opens — a local-file-read / exfiltration surface that a session must gate behind
//!   [`Policy`](crate::Policy) (design 06's file-transfer gate). Those transmission modes are a
//!   later session-layer slice and are deliberately absent here.
//!
//! ```
//! use qwertty::CommandBuffer;
//! use qwertty::commands::graphics::kitty;
//!
//! // Transmit a (tiny) image and display it, then delete it by id.
//! let mut frame = CommandBuffer::new();
//! frame
//!     .command(kitty::transmit_and_display(
//!         kitty::Format::Png,
//!         b"\x00\x00\x00",
//!     ))
//!     .command(kitty::delete_all_images());
//!
//! assert_eq!(
//!     frame.as_bytes(),
//!     b"\x1b_Ga=T,f=100;AAAA\x1b\\\x1b_Ga=d;\x1b\\"
//! );
//! ```

pub mod kitty;
