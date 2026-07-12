# Keybinding Portability

Some key combinations cannot be told apart from others in a plain terminal, and which ones depend on
the terminal, the platform, and the input protocol in effect. This page is the durable reference for
what qwertty can and cannot distinguish, so an application chooses keybindings that survive across
the terminals its users actually run.

The short version: prefer modified *special* keys (arrows, function keys, Home/End) over modified
*letters*, avoid `Ctrl`+`Shift`+letter, and treat the collisions below as unavailable unless a
progressive-enhancement protocol (kitty keyboard, or win32-input-mode) is negotiated and confirmed.

## The Legacy Collisions

In the traditional terminal encoding — the one every terminal falls back to, and the only one some
still speak — several distinct key presses arrive as the *same* bytes, because they were assigned the
same control code decades ago. qwertty decodes bytes faithfully; it cannot recover a distinction the
terminal never encoded. The unavoidable collisions:

| Key press             | Arrives as                     | Collides with               |
| --------------------- | ------------------------------ | --------------------------- |
| `Ctrl`+`I`            | `0x09`                         | `Tab`                       |
| `Ctrl`+`M`            | `0x0D`                         | `Enter` / `Return`          |
| `Ctrl`+`[`            | `0x1B`                         | `Escape`                    |
| `Ctrl`+`H`            | `0x08`                         | `Backspace`                 |
| `Ctrl`+`Shift`+letter | the same byte as `Ctrl`+letter | the unshifted `Ctrl`+letter |
| `Shift`+`Enter`       | `0x0D`                         | `Enter`                     |

Because of these, an application that binds, say, `Ctrl`+`I` to one action and `Tab` to another will
see both fire on either press in a legacy terminal. qwertty reports the *key* (`Tab`, `Enter`,
`Escape`, `Backspace`) for these bytes; it does not guess which physical combination produced them.

## The Enhancement Ladder

Two protocols recover the lost distinctions by having the terminal send richer sequences. Both are
opt-in and must be confirmed, never assumed:

- **Kitty keyboard protocol** — the portable, preferred path. Progressive-enhancement flags report
  modifiers, key events (press/repeat/release), and the shifted/base layout, so the collisions above
  become distinguishable. Supported by a growing set of terminals — including, on Windows, Windows
  Terminal 1.25 and later. qwertty pushes the flags and then reads back the granted subset rather
  than trusting the request (`request_kitty_keyboard`), so an application acts on what the terminal
  actually honored. See the [kitty keyboard reference](crate::docs::kitty_keyboard).

- **win32-input-mode** (`CSI ? 9001 h`) — a Windows-specific fallback that serializes full console
  key records (virtual key, scan code, Unicode char, key-down flag, control-key state, repeat count)
  as escape sequences. qwertty *decodes* these sequences unconditionally, so a terminal that sends
  them is understood. Enabling the mode, however, is a policy-gated opt-in, because:
  - it is **all-or-nothing** — turning it on reshapes *every* keystroke into a `CSI … _` sequence,
    with no per-key fallback;
  - it is **not universal** — several hosts (including VS Code's terminal) do not send it;
  - a hard terminal reset (`RIS`) silently disables it, so its state is not durable.

  It also cannot express a few concepts the frozen event vocabulary has no home for — left/right
  modifier position, the enhanced-key flag, Scroll Lock, and autorepeat-versus-fresh-press — which are
  dropped rather than approximated.

## Practical Guidance

- **Bind modified special keys, not modified letters.** `Ctrl`+arrow, `Shift`+`F5`, `Alt`+`Home` are
  distinguishable in the legacy encoding; `Ctrl`+`Shift`+`P` is not.
- **Do not rely on `Ctrl`+`Shift`+letter** unless kitty keyboard is confirmed for the session.
- **Treat `Ctrl`+`H`/`Ctrl`+`I`/`Ctrl`+`M`/`Ctrl`+`[` as their collided keys** (`Backspace`, `Tab`,
  `Enter`, `Escape`) unless an enhancement protocol is active — `Ctrl`+`Backspace`, in particular, is
  unavailable in the legacy encoding.
- **Probe, then adapt.** Use `request_kitty_keyboard` (and, on Windows, the win32-input policy) to
  learn what the current terminal supports, and offer richer bindings only where the terminal
  confirmed them.
- **See it live.** The `input_event_viewer` example prints the decoded event for every key, so you
  can check exactly which combinations a given terminal distinguishes before you bind them — run it
  with `cargo run --example input_event_viewer --features tokio`.

## Related References

- [Kitty keyboard](crate::docs::kitty_keyboard) — the progressive-enhancement flags and
  verify-after-push.
- [Terminal input](crate::docs::terminal_input) — the decoded key, mouse, focus, paste, and resize
  event vocabulary.
- [Platform support](crate::docs::platform) — the Unix/Windows boundary, including where
  win32-input-mode decode applies.
