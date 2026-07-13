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

## kitty graphics at a glance

The protocol carries images in Application Program Command sequences:

```text
ESC _ G <control-keys> ; <base64-payload> ESC \
```

The `G`-prefixed control keys are a comma-separated `key=value` list; the payload after `;` is the
base64 of the image (empty for a control-only command like a delete). Payloads past the protocol's
4096-byte chunk bound split automatically into `m=1`/`m=0` chunked transmissions. The helpers
cover:

- `query_support` → the spec's own `a=q` support probe (see below).
- `transmit` → `ESC _ Ga=t,i=<id>,f=…;<b64> ESC \` — send an image under a client-assigned id,
  without displaying it; the terminal acknowledges by echoing the id.
- `transmit_and_display` → `ESC _ Ga=T,f=…;<b64> ESC \` — send an image and show it at the cursor
  in one un-acknowledged shot (no id).
- `place` / `place_with` → `ESC _ Ga=p,i=<id>…; ESC \` — show an already-transmitted image by id,
  optionally with a placement id, column/row scaling, and z-index.
- `delete_image` / `delete_all_images` / `delete_placement` → drop placements, keeping the stored
  data for re-display; the `…_and_data` forms also free it.
- `transmit_file` / `transmit_temp_file` / `transmit_shared_memory` → the policy-gated
  resource-naming transmission forms (see below).

Image ids are client-assigned: the application chooses the number and reuses it to place or delete.
qwertty keeps no registry — the id space is the caller's. Every id-carrying command is acknowledged
with `APC G i=<id> ; OK ST` (or an ASCII error such as `ENOENT:…`), parsed by
[`KittyGraphicsReport`](crate::report::KittyGraphicsReport); the echoed id is how the internal
query correlator matches an acknowledgement to the command that provoked it, so two graphics
commands in flight can never complete each other's query.

## Capability: probe, never sniff

The kitty graphics query rides the same DA1-fenced bundle as every other capability probe (on the
Tokio session, `probe_capabilities` — the spec itself recommends exactly this query-then-DA1
pattern). The result lands in [`Capabilities::kitty_graphics`](crate::Capabilities::kitty_graphics)
with honest provenance:

| Terminal behaviour             | Finding value | Evidence                               |
| ------------------------------ | ------------- | -------------------------------------- |
| answers `OK`                   | `Some(true)`  | `Probed { via: "kitty graphics a=q" }` |
| answers an error               | `Some(false)` | `Probed { via: "kitty graphics a=q" }` |
| silent (or a mux swallowed it) | `None`        | `Unknown`                              |

Live conformance runs confirm the split — kitty, ghostty, and `WezTerm` answer `OK`; tmux and
alacritty stay silent (see [Conformance](crate::docs::conformance)). Unknown is not unsupported (a
multiplexer may have eaten the APC), and it is also not permission to emit: painting kitty
graphics bytes at a terminal that cannot render them prints garbage, so gate emission on a
known-`true` finding, the same rule that keeps mode-2026 wraps off terminals that never answered.

iTerm2 images have no support query at all, so a session gates their emission on the
identity-keyed [`Capabilities::iterm2_images`](crate::Capabilities::iterm2_images) finding rather
than a probe (see the iTerm2 section below).

## Pixel geometry: zeros are an admission, not a measurement

Sizing a placement needs the cells-to-pixels conversion. The probe asks two XTWINOPS questions —
[`request_text_area_pixels`](crate::commands::terminal::request_text_area_pixels) (`CSI 14 t`) and
[`request_cell_size`](crate::commands::terminal::request_cell_size) (`CSI 16 t`) — parsed by
[`TextAreaPixelsReport`](crate::report::TextAreaPixelsReport) and
[`CellSizeReport`](crate::report::CellSizeReport). Many terminal stacks answer these with zero
dimensions. The reports preserve the zeros verbatim, but their `pixel_size` accessors — and the
[`text_area_pixels`](crate::Capabilities::text_area_pixels) /
[`cell_size`](crate::Capabilities::cell_size) findings — refuse to turn a zero into a geometry: the
value stays unknown and the application chooses its own fallback. qwertty never fabricates a
default cell size.

## The policy split: who opens the resource?

Image *bytes* are not the security surface — *resource naming* is. The kitty protocol has four
transmission media, and they differ in exactly one way that matters:

| Transmission          | Who supplies the bytes               | Gating                              |
| --------------------- | ------------------------------------ | ----------------------------------- |
| direct (`t=d`)        | the application, inline              | capability only                     |
| file (`t=f`)          | **the terminal reads a path**        | capability + policy (file transfer) |
| temp file (`t=t`)     | **the terminal reads, then deletes** | capability + policy (file transfer) |
| shared memory (`t=s`) | **the terminal opens an IPC object** | capability + policy (file transfer) |

Direct transmission carries bytes the application already owns; it opens no new resource and needs
no policy. The other three make the escape stream name a resource **the terminal itself opens** —
attacker-influenced output could steer a supporting terminal into reading any readable file and
rendering it, a local-file-read and exfiltration primitive. Their session-level emits
([`transmit_kitty_file`](crate::TerminalSession::transmit_kitty_file) and siblings) therefore sit
behind the existing file-transfer policy gate: the default
[`Policy::restricted`](crate::Policy::restricted) denies them with a typed
[`PolicyDenied`](crate::Error::PolicyDenied), a missing capability finding refuses with
[`CapabilityUnverified`](crate::Error::CapabilityUnverified), and no bytes are written in either
case. The encode-only builders remain the raw escape hatch for callers that gate themselves.

## Lifecycle: images are content, not session state

A placed image is output content, exactly like emitted text. It does not enter the session's mode
ledger, is not replayed or undone by `leave` or drop, and is never auto-cleared — an
alternate-screen exit clears its screen anyway, and a primary-screen application owns its own
cleanup. The explicit surface is the delete family:
[`delete_image`](crate::commands::graphics::kitty::delete_image) and friends drop placements while
the terminal may keep the transmitted data for re-display, and the `…_and_data` forms free the
stored data too.

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

Unlike kitty, the protocol has no support query, so the capability finding is *identity-keyed*:
[`Capabilities::iterm2_images`](crate::Capabilities::iterm2_images) is a known `true` with
`Inferred { via: "terminal identity" }` evidence when the resolved identity is iTerm2 or `WezTerm`
— which speaks both this protocol and kitty graphics, so one `WezTerm` identity may enable two image
protocols at once — and honestly unknown for every other identity: an identity can fail to affirm
support, but it can never prove absence. An inferred finding is deliberately weaker provenance
than a probed one; the difference stays inspectable on the finding's `Evidence`, never papered
over. Because the finding is keyed on the *resolved* identity, an XTVERSION reply that improves on
the environment's story re-derives it — a multiplexer answering XTVERSION for itself downgrades an
env-inferred `true` back to unknown, because OSC 1337 written into an unaware mux is garbled no
matter what the outer terminal renders.

Emission is gated on that finding at the session layer, exactly like every other graphics emit
(R-CAP-4): [`inline_iterm2_image`](crate::TerminalSession::inline_iterm2_image) and
[`inline_iterm2_image_sized`](crate::TerminalSession::inline_iterm2_image_sized) — with async
analogues on the Tokio session — refuse with a typed
[`CapabilityUnverified`](crate::Error::CapabilityUnverified), writing nothing, unless the finding
is known-`true`. There is no policy gate: the image bytes are inline and the escape names no
resource the terminal would open, exactly like kitty direct transmission. A caller that verified
rendering out of band builds its own finding — the explicit escape hatch, same as the kitty gates.

See the `kitty_graphics.rs` example for the full probed flow (probe, transmit, place, decode the
acknowledgement, delete) and the `iterm2_inline_image.rs` example for the identity-gated flow. The
full surface, capability, and policy design is in `work/phase2/design/11-graphics.md`.
