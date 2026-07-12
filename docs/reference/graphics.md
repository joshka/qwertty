# Inline images (graphics)

Some terminals can paint pixels into the character grid through an *image protocol*. qwertty builds
the bytes for these protocols with typed, encode-only helpers under
[`commands::graphics`](crate::commands::graphics) — one submodule per protocol, because the
protocols differ in ways (placement, deletion, whether the terminal answers a support query) that a
single "draw an image" abstraction would hide.

Two protocols are implemented:

- the [kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/), in
  [`commands::graphics::kitty`](crate::commands::graphics::kitty) — the most capable, and the only
  one that can be *probed* for support;
- [iTerm2 inline images](https://iterm2.com/documentation-images.html), in
  [`commands::graphics::iterm2`](crate::commands::graphics::iterm2) — a simpler one-shot form (also
  spoken by `WezTerm`) with no support query.

## What these helpers do and do not do

A graphics helper returns a `Command` of raw bytes, built without a terminal, session, decoder, or
policy — exactly like every other [`commands`](crate::commands) helper. Two obligations therefore
live above this layer, wherever session code forwards the bytes to a real terminal:

- **Capability gating.** Send an image protocol only to a terminal that supports it. Painting kitty
  graphics bytes at a terminal that cannot render them prints garbage. Support is a session-level
  capability finding (for kitty, confirmed by a support probe); the encode helpers cannot and do
  not check.
- **Transmission policy.** The helpers build only the *inline* transmission form, where the caller
  supplies the image bytes and the escape opens no new resource. The kitty protocol also defines
  file, temp-file, and shared-memory transmission, where the escape names a path or object the
  terminal opens on the caller's behalf — a local-file-read and exfiltration surface. Those forms
  belong behind a `Policy` gate at the session layer and are intentionally not built here.

## kitty graphics at a glance

The protocol carries images in Application Program Command sequences:

```text
ESC _ G <control-keys> ; <base64-payload> ESC \
```

The `G`-prefixed control keys are a comma-separated `key=value` list; the payload after `;` is the
base64 of the image (empty for a control-only command like a delete). The helpers cover:

- `transmit_and_display` → `ESC _ Ga=T,f=…;<b64> ESC \` — send an image and show it at the cursor.
- `place` → `ESC _ Ga=p,i=<id>; ESC \` — show an already-transmitted image by id.
- `delete_all_images` → `ESC _ Ga=d; ESC \` — drop every image and placement.
- `delete_image` → `ESC _ Ga=d,d=i,i=<id>; ESC \` — drop one image by id.

Image ids are client-assigned: the application chooses the number and reuses it to place or delete.
qwertty keeps no registry — the id space is the caller's.

```rust
use qwertty::CommandBuffer;
use qwertty::commands::graphics::kitty::{self, Format};

// Transmit a PNG and display it, then clear it later.
let show = CommandBuffer::new()
    .command(kitty::transmit_and_display(Format::Png, /* png bytes */ b"\x00\x00\x00"))
    .as_bytes()
    .to_vec();
assert_eq!(show, b"\x1b_Ga=T,f=100;AAAA\x1b\\");
```

## iTerm2 inline images at a glance

iTerm2 (and `WezTerm`) display an image with an OSC 1337 `File` command:

```text
ESC ] 1337 ; File=<key=value>;… : <base64-payload> ESC \
```

Only the inline form (`inline=1`, the caller's own bytes) is built — the escape names no file, so
it opens no resource. The helpers cover:

- `inline_image` → `ESC ]1337;File=inline=1:<b64> ESC \` — show an image at its natural size.
- `inline_image_sized` → adds `;width=<w>;height=<h>` — constrain to a `Dimension` (cells, pixels,
  percent, or auto).

Unlike kitty, the protocol has no support query, so a session gates emission on a terminal-identity
finding rather than a probe.

## Not yet built

The support probe and capability findings (kitty's readback query and iTerm2's identity keying), the
policy-gated file/temp/shared-memory transmission forms, and image-id-carrying kitty transmission are
planned follow-ups. The full surface, capability, and policy design is in
`work/phase2/design/11-graphics.md`.
