//! Inline-image (graphics) command helpers.
//!
//! Terminals that support an image protocol let a program paint pixels into the character grid.
//! qwertty exposes each protocol as its own submodule of typed, encode-only helpers — one protocol
//! per submodule rather than a lowest-common-denominator abstraction, because the protocols differ
//! in real ways (placement, deletion, animation, and whether the terminal answers) that a unifying
//! type would erase. See `work/phase2/design/11-graphics.md` for the full surface, capability, and
//! policy design, and [`docs::graphics`](crate::docs::graphics) for the concept page.
//!
//! Two protocols are implemented:
//!
//! - [`kitty`] — the [kitty graphics protocol]: the most capable and the only one that answers a
//!   support query, so it is the primary target.
//! - [`iterm2`] — [iTerm2 inline images]: a simpler one-shot form (also spoken by `WezTerm`) with
//!   no support query, so support is identity-keyed.
//!
//! [kitty graphics protocol]: https://sw.kovidgoyal.net/kitty/graphics-protocol/
//! [iTerm2 inline images]: https://iterm2.com/documentation-images.html
//!
//! # These helpers do not gate anything
//!
//! Like every [`commands`](crate::commands) helper, a graphics helper returns a
//! [`Command`](crate::Command) of raw bytes built without a terminal, session, decoder, or
//! [`Policy`](crate::Policy). Two obligations live *above* this layer, in whatever session code
//! forwards the bytes to a real terminal:
//!
//! - **Capability gating.** Emit an image protocol only into a terminal that supports it — for
//!   kitty, one confirmed by the support probe (the `a=q` query rides the DA1-fenced capability
//!   bundle and lands in [`Capabilities::kitty_graphics`](crate::Capabilities::kitty_graphics);
//!   sending image bytes to a terminal that cannot render them prints garbage). This module cannot
//!   and does not check.
//! - **Transmission policy.** Inline transmission — the caller supplies the image bytes, the escape
//!   opens no new resource — needs no policy. The kitty file, temp-file, and shared-memory
//!   transmission forms ([`kitty::transmit_file`] and siblings) instead name a path or object **the
//!   terminal itself opens** — a local-file-read / exfiltration surface — so their session-level
//!   emits ([`TerminalSession::transmit_kitty_file`](crate::TerminalSession::transmit_kitty_file)
//!   and siblings) sit behind the [`Policy`](crate::Policy) file-transfer gate (design 06), denied
//!   under the default [`Policy::restricted`](crate::Policy::restricted).
//!
//! # Ownership and lifecycle
//!
//! Placed images are **application-owned content**, like emitted text: they are not tracked in
//! the session mode ledger, not restored or replayed by session teardown, and never auto-cleared
//! by qwertty. The protocol delete commands (for example [`kitty::delete_image`]) are the
//! explicit cleanup surface; image and placement ids are caller-chosen and caller-owned.
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

pub mod iterm2;
pub mod kitty;
