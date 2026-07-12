# Windows Interactive Validation Runbook

CI proves the Windows backend compiles, that the async read path works through injected console
records, and that mode restoration round-trips (the `windows-latest` job). What it cannot prove is
real interactive behavior: how live keystrokes, an IME, the mouse, and resizing behave under an
actual terminal host with a human driving it. This runbook is the manual pass that closes that gap.
It tracks [issue #196](https://github.com/joshka/qwertty/issues/196) item 1.

Run it on a real Windows 10 (build 1809+) or Windows 11 machine — a VM is fine. It is not part of
CI; it is a release-confidence checklist a maintainer runs when touching the Windows backend.

## Setup

- Toolchain: `rustup` with the `x86_64-pc-windows-msvc` (or `aarch64-pc-windows-msvc` on ARM)
  target and the matching MSVC build tools.
- Install at least one IME: add a Japanese or Chinese input method under Settings → Time & Language
  → Language, so you can test composed input.
- The driver is the cross-platform example:

  ```sh
  cargo run --example input_event_viewer --features tokio
  ```

  It enters raw mode, enables mouse/focus/paste reporting, probes kitty keyboard, then prints every
  decoded `Event` until you press Ctrl-C. Watch its output while you exercise each item below.

## Terminal Matrix

Run the viewer under each host and record the results. "Requires VT" means the host must present a
VT-capable console; all of these do on 1809+.

| Host                        | Notes                                                                                     |
| --------------------------- | ----------------------------------------------------------------------------------------- |
| Windows Terminal (stable)   | The default console on Windows 11 22H2+; VT-native.                                       |
| Windows Terminal 1.25+      | Adds kitty keyboard — confirm the viewer's header reports it.                             |
| classic conhost             | Launch `conhost.exe`; the Windows 10 default. VT still works when the app sets the modes. |
| wezterm                     | Third-party; drives the app through ConPTY.                                               |
| VS Code integrated terminal | xterm.js over ConPTY; no win32-input-mode or kitty.                                       |

## Checklist (per host)

- [ ] **Startup / teardown.** The viewer enters raw mode cleanly and, on Ctrl-C, restores the
      console: the prompt echoes normally afterward, the cursor is visible, and the mouse is no
      longer reporting. (This is the FM-W4 anti-leak guarantee — no residual mode.)
- [ ] **ASCII keys and control keys.** Letters, digits, Enter, Tab, Backspace, Escape, and Ctrl-key
      combinations decode to the expected `Key`. Note the legacy collisions from the
      [keybinding portability reference](../reference/keybinding-portability.md) — Ctrl-I/Tab,
      Ctrl-M/Enter, Ctrl-[/Escape, Ctrl-H/Backspace — and confirm they behave as documented.
- [ ] **Special keys.** Arrows, Home/End, PageUp/PageDown, Insert/Delete, and function keys decode
      correctly (with and without modifiers where the host supports it).
- [ ] **IME / CJK (the priority risk, RR-5).** Switch to a CJK input method, compose a multi-glyph
      word, and confirm each committed character arrives as `Key::Char` with the expected `text`,
      with no dropped or duplicated code points and no mangled surrogate pairs. This is the failure
      that made helix re-add crossterm on Windows; if it is broken here, the fix is an
      `INPUT_RECORD`-assisted decode path behind the device trait (a decode switch, not a transport
      change) — file it against #196.
- [ ] **Paste.** Paste multi-line text containing non-ASCII; confirm it arrives as one `Paste` event
      (or bracketed segments) with bytes intact, not as a flood of individual keys.
- [ ] **Mouse.** Click, drag, and scroll; confirm `Mouse` events with sensible coordinates. Note
      that Windows Terminal delivers SGR mouse while classic conhost delivers record-based mouse —
      both should surface as `Mouse` events.
- [ ] **Focus.** Alt-tab away and back; confirm `Focus` gained/lost events (where the host reports
      them).
- [ ] **Resize.** Drag the window edge; confirm `Resize` events with the new geometry arrive in-band
      through `next_event` (there is no separate resize stream on Windows).
- [ ] **Kitty keyboard (WT 1.25+ only).** With the viewer reporting kitty support, confirm the
      normally-undecodable combinations (Ctrl-Shift-letter, Ctrl-Backspace) now decode distinctly.

## Recording Results

Note the host, its version, and any deviation. A clean pass across Windows Terminal and conhost is
the bar for calling the Windows tier interactively validated; per-host quirks (especially conhost vs
Windows Terminal mouse and `dwControlKeyState` differences) belong in the capability model and, where
they are defects, in follow-up issues.
