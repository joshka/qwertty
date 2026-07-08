# Terminal Control Reference

qwertty builds byte sequences that terminal emulators interpret as commands. This page explains the
terms used by the first command helpers. It is intentionally small; each new protocol-facing API
should add the terms and behavior it needs.

## Command Anatomy

Terminal command documentation often uses compact names for bytes and sequence structure. This
section explains the names qwertty uses before describing specific commands.

### Escape Byte

Most terminal commands begin with the `ESC` byte, hexadecimal `0x1b`. Rust byte strings write this
as `\x1b`.

### Control Sequence Introducer

`CSI` means "Control Sequence Introducer". qwertty uses the common 7-bit spelling:

```text
ESC [
```

In Rust byte strings that is:

```rust
let csi = b"\x1b[";

assert_eq!(csi, &[0x1b, b'[']);
```

### Parameters And Final Bytes

CSI commands then include optional parameters and a final byte that names the operation. For
example, `CSI 3 ; 5 H` moves the cursor to row 3, column 5:

```rust
let command = b"\x1b[3;5H";

assert_eq!(command, &[0x1b, b'[', b'3', b';', b'5', b'H']);
```

CSI input uses the same syntax shape. qwertty's syntax layer preserves complete CSI input as a
`SyntaxToken::Csi` carrying its bytes plus structured parameters, intermediates, and the final byte
before the semantic and report layers interpret it.

```rust
use qwertty::{SyntaxParser, SyntaxToken};

let mut parser = SyntaxParser::new();
let mut tokens = parser.feed(b"\x1b[?25n");
tokens.extend(parser.finish());

let SyntaxToken::Csi(csi) = &tokens[0] else {
    panic!("expected a CSI token");
};

assert_eq!(csi.params().private_markers(), b"?");
assert_eq!(csi.params().param_bytes(), b"25");
assert_eq!(csi.params().final_byte(), b'n');
```

### ECMA-48

ECMA-48 is the control-function standard behind many terminal cursor, screen, and text controls.
qwertty names ECMA-48 when a helper emits one of those standard controls.

## Current Commands

This section covers the protocol commands exposed by the current encode-only helper surface.

### Cursor Position

`CUP` means "Cursor Position". It is an ECMA-48 command written as:

```text
CSI row ; column H
```

Upstream references:

- [ECMA-48](https://ecma-international.org/publications-and-standards/standards/ecma-48/)
- [xterm `CSI Ps ; Ps H`](https://www.xfree86.org/current/ctlseqs.html#:~:text=CSI%20P%20s%20%3B%20P%20s%20H)

`commands::cursor::move_to(ProtocolPosition::new(3, 5))` emits:

```rust
use qwertty::commands::cursor;
use qwertty::{CommandBuffer, ProtocolPosition};

let mut frame = CommandBuffer::new();
frame.command(cursor::move_to(ProtocolPosition::new(3, 5)));

assert_eq!(frame.as_bytes(), b"\x1b[3;5H");
```

Terminal protocol coordinates are one-based. Row 1, column 1 is the top-left cell.

### Cursor Position Query And Report

`commands::cursor::request_position` emits the ECMA-48 Device Status Report request for the
current cursor position:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::cursor;

let mut frame = CommandBuffer::new();
frame.command(cursor::request_position());

assert_eq!(frame.as_bytes(), b"\x1b[6n");
```

Terminals commonly answer with `CSI row ; column R`. qwertty parses that report from the CSI syntax
token:

```rust
use qwertty::{CursorPositionReport, ProtocolPosition, SyntaxParser, SyntaxToken};

let mut parser = SyntaxParser::new();
let mut tokens = parser.feed(b"\x1b[12;34R");
tokens.extend(parser.finish());
let SyntaxToken::Csi(csi) = &tokens[0] else {
    panic!("expected a CSI token");
};

let report = CursorPositionReport::from_control_sequence(csi).unwrap();

assert_eq!(report.position(), ProtocolPosition::new(12, 34));
```

With the optional `tokio` feature on Unix, `TokioTerminalSession::request_cursor_position` writes
the request, flushes output, waits for the matching report, and applies a caller-provided timeout.
See [Live Query Helpers (Tokio)](crate::docs#live-query-helpers-tokio) for the runnable example,
included with the `tokio` feature.

Unrelated decoded events that arrive before the matching report remain available through
`TokioTerminalSession::next_event`. This is still not a general query router: qwertty does not yet
support multiple simultaneous live queries, capability probing, or query registration.

Future live query helpers should keep this boundary: command helpers describe emitted bytes,
response parsers describe interpreted input, and `TokioTerminalSession` owns the live routing
state that connects a request to a response without hiding unrelated input.

### Terminal Status Query And Report

`commands::terminal::request_status` emits the ECMA-48 Device Status Report request for terminal
status:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::terminal;

let mut frame = CommandBuffer::new();
frame.command(terminal::request_status());

assert_eq!(frame.as_bytes(), b"\x1b[5n");
```

Terminals commonly answer with `CSI 0 n` for ready or `CSI 3 n` for malfunction. qwertty parses
those reports from the CSI syntax token:

```rust
use qwertty::{SyntaxParser, SyntaxToken, TerminalStatus, TerminalStatusReport};

let mut parser = SyntaxParser::new();
let mut tokens = parser.feed(b"\x1b[0n");
tokens.extend(parser.finish());
let SyntaxToken::Csi(csi) = &tokens[0] else {
    panic!("expected a CSI token");
};

let report = TerminalStatusReport::from_control_sequence(csi).unwrap();

assert_eq!(report.status(), TerminalStatus::Ready);
```

With the optional `tokio` feature on Unix, `TokioTerminalSession::request_terminal_status` writes
the request, flushes output, waits for the matching report, and applies a caller-provided timeout.
See [Live Query Helpers (Tokio)](crate::docs#live-query-helpers-tokio) for the runnable example,
included with the `tokio` feature.

Unrelated decoded events that arrive before the matching report remain available through
`TokioTerminalSession::next_event`. This is still not a general query router: qwertty does not yet
support multiple simultaneous live queries, capability probing, or query registration.

### Input Enablement Commands

`commands::terminal` also builds the DEC private-mode sequences that turn on the input reporting
modes `SemanticDecoder` decodes, plus the probe requests a capability check needs. Every enable has
a matching disable that emits the exact reverse bytes, which is what `TerminalSession`'s mode ledger
replays on `leave`.

**Mouse (DEC 1000/1002/1003, paired with SGR 1006).** `enable_mouse(MouseMode)` picks *which*
events the terminal reports — [`MouseMode::Normal`](crate::commands::terminal::MouseMode::Normal)
(press/release only), [`MouseMode::ButtonEvent`](crate::commands::terminal::MouseMode::ButtonEvent)
(press/release/drag), or [`MouseMode::AnyEvent`](crate::commands::terminal::MouseMode::AnyEvent)
(all motion) — and always pairs it with mode 1006, the SGR extended-coordinate encoding qwertty
decodes to `MouseEvent`:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::terminal::{self, MouseMode};

let mut frame = CommandBuffer::new();
frame.command(terminal::enable_mouse(MouseMode::ButtonEvent));
assert_eq!(frame.as_bytes(), b"\x1b[?1002h\x1b[?1006h");

frame.command(terminal::disable_mouse(MouseMode::ButtonEvent));
assert_eq!(
    frame.as_bytes(),
    b"\x1b[?1002h\x1b[?1006h\x1b[?1006l\x1b[?1002l"
);
```

**Focus (DEC 1004).** `enable_focus_events()`/`disable_focus_events()` turn `CSI I` (gain) and
`CSI O` (loss) reporting on and off; qwertty decodes these to `FocusEvent`:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::terminal;

let mut frame = CommandBuffer::new();
frame.command(terminal::enable_focus_events());
assert_eq!(frame.as_bytes(), b"\x1b[?1004h");
```

**Bracketed paste (DEC 2004).** `enable_bracketed_paste()`/`disable_bracketed_paste()` turn on the
`ESC [ 200 ~ … ESC [ 201 ~` wrapping qwertty decodes to `PasteEvent` segments, so pasted text is
delivered as data instead of being mistaken for typed keys:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::terminal;

let mut frame = CommandBuffer::new();
frame.command(terminal::enable_bracketed_paste());
assert_eq!(frame.as_bytes(), b"\x1b[?2004h");
```

**In-band resize (DEC 2048).** `enable_in_band_resize()`/`disable_in_band_resize()` turn on
`CSI 48 ; height ; width ; height_px ; width_px t` size reports, decoded to `ResizeEvent`, as an
alternative to the out-of-band `SIGWINCH` signal:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::terminal;

let mut frame = CommandBuffer::new();
frame.command(terminal::enable_in_band_resize());
assert_eq!(frame.as_bytes(), b"\x1b[?2048h");
```

**Probe requests: DA1, XTVERSION, DECRQM.** These build query bytes only; they do not write,
flush, wait, or route a reply.

- `request_primary_device_attributes()` emits `CSI c` (DA1). In a capability probe this is written
  **last** as a fence: because a terminal answers in order, DA1's reply arriving means every
  earlier reply that was coming has arrived.
- `request_xtversion()` emits `CSI > q`. The terminal answers with a DCS string qwertty parses into
  [`XtVersionReport`](crate::report::XtVersionReport) (see [Typed Reports](#typed-reports) below).
- `request_dec_private_mode(mode)` emits the DECRQM request `CSI ? mode $ p` for the given DEC
  private-mode number (`request_dec_private_mode(2026)` emits `b"\x1b[?2026$p"`). The terminal
  answers `CSI ? mode ; value $ y`, parsed into
  [`DecPrivateModeReport`](crate::report::DecPrivateModeReport).

```rust
use qwertty::CommandBuffer;
use qwertty::commands::terminal;

let mut frame = CommandBuffer::new();
frame
    .command(terminal::request_xtversion())
    .command(terminal::request_dec_private_mode(2026))
    .command(terminal::request_primary_device_attributes());

assert_eq!(frame.as_bytes(), b"\x1b[>q\x1b[?2026$p\x1b[c");
```

**Kitty keyboard push/pop/query.** `push_kitty_keyboard_flags(flags)` emits `CSI > flags u`,
turning on the requested progressive-enhancement reporting and pushing the previous set onto the
terminal's flags stack; `pop_kitty_keyboard_flags()` emits `CSI < 1 u`, the exact undo of one push.
`query_kitty_keyboard_flags()` emits `CSI ? u`, asking for the currently active (granted) flags —
the read half of the verify-after-push handshake (design 06), since a terminal may grant only a
subset of what was pushed:

```rust
use qwertty::commands::terminal;
use qwertty::{CommandBuffer, KittyKeyboardFlags};

let mut frame = CommandBuffer::new();
frame
    .command(terminal::push_kitty_keyboard_flags(
        KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES,
    ))
    .command(terminal::query_kitty_keyboard_flags())
    .command(terminal::pop_kitty_keyboard_flags());

assert_eq!(frame.as_bytes(), b"\x1b[>1u\x1b[?u\x1b[<1u");
```

See [Input Modes](crate::docs#input-modes) in the session reference for how `TerminalSession`
records these as reversible ledger entries instead of raw one-off writes.

### Typed Reports

Three more `report::` types round out the query-response surface beyond cursor position and
terminal status:

- [`DecPrivateModeReport`](crate::report::DecPrivateModeReport) and
  [`DecPrivateModeState`](crate::report::DecPrivateModeState) — the DECRPM answer to
  `request_dec_private_mode`, `CSI ? mode ; value $ y`. `DecPrivateModeReport::mode()` and
  `::state()` expose the queried mode number and its five-way state (`NotRecognized`, `Set`,
  `Reset`, `PermanentlySet`, `PermanentlyReset`); `::is_enabled()` collapses that to
  `Option<bool>` (`None` for `NotRecognized`, matching the unknown-not-unsupported rule in the
  capability model reference).
- [`XtVersionReport`](crate::report::XtVersionReport) — the answer to `request_xtversion`, a DCS
  string `DCS > | text ST`. `::version()` returns the terminal's self-reported identification text
  verbatim, unparsed.
- [`OscColorReport`](crate::report::OscColorReport) and
  [`OscColorKind`](crate::report::OscColorKind) — a parsed OSC 10/11 default-colour reply.
  `OscColorKind` discriminates `Foreground` (OSC 10) from `Background` (OSC 11) — the two share
  the `rgb:…` payload shape and differ only in the OSC selector — and
  `OscColorReport::kind()`/`::rgb()` expose which colour it was and its normalized
  [`Rgb`](crate::Rgb) value.

```rust
use qwertty::report::{OscColorKind, OscColorReport};
use qwertty::Rgb;

let report = OscColorReport::from_osc_payload(b"11;rgb:1a1a/2b2b/3c3c").expect("colour report");
assert_eq!(report.kind(), OscColorKind::Background);
assert_eq!(report.rgb(), Rgb::new(0x1a, 0x2b, 0x3c));
```

### Erase In Display

`ED` means "Erase in Display". qwertty's first screen clear helper uses mode `2`, which erases the
complete active display:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::screen;

let mut frame = CommandBuffer::new();
frame.command(screen::clear());

assert_eq!(frame.as_bytes(), b"\x1b[2J");
```

The command erases display cells but does not move the cursor.

Upstream references:

- [ECMA-48](https://ecma-international.org/publications-and-standards/standards/ecma-48/)
- [xterm `CSI Ps J`](https://www.xfree86.org/current/ctlseqs.html#:~:text=CSI%20P%20s%20J)

### Erase In Line

`EL` means "Erase in Line". qwertty's first line erase helper uses mode `2`, which erases the
complete active line:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::screen;

let mut frame = CommandBuffer::new();
frame.command(screen::erase_line());

assert_eq!(frame.as_bytes(), b"\x1b[2K");
```

The command erases line cells but does not move the cursor.

Upstream references:

- [ECMA-48](https://ecma-international.org/publications-and-standards/standards/ecma-48/)
- [xterm `CSI Ps K`](https://www.xfree86.org/current/ctlseqs.html#:~:text=CSI%20P%20s%20K)

### Alternate Screen

`commands::screen::enter_alternate_screen` and `commands::screen::leave_alternate_screen` use the
widely supported xterm private mode 1049, which switches to a separate screen buffer and saves the
cursor position:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::screen;

let mut frame = CommandBuffer::new();
frame
    .command(screen::enter_alternate_screen())
    .command(screen::leave_alternate_screen());

assert_eq!(frame.as_bytes(), b"\x1b[?1049h\x1b[?1049l");
```

These helpers only build the enter and leave bytes. `TerminalSession::enter_alternate_screen`
(R-OUT-3) pairs the enter bytes with an **explicit clear** (`commands::screen::clear`,
`CSI 2 J`) immediately after entry, and tracks the pair in the session's mode ledger so `leave`
restores the primary screen automatically. The explicit clear exists because mode 1049 does not
clear the alternate buffer implicitly on every host: mosh does not, and helix works around exactly
this by clearing right after entering, so qwertty follows that evidence rather than trusting the
terminal's own 1049 behavior. See [Screen And Cursor
Lifecycle](crate::docs#screen-and-cursor-lifecycle) for the session-level API.

Upstream references:

- [xterm alternate screen buffer `CSI ? 1049 h` / `CSI ? 1049 l`](https://www.xfree86.org/current/ctlseqs.html#:~:text=P%20s%20%3D%201%200%204%209)

### Synchronized Output

`commands::screen::begin_synchronized_update` and `commands::screen::end_synchronized_update` use
the xterm/Contour private mode 2026, which asks a supporting terminal to buffer output written
between the pair and paint it as one atomic update instead of redrawing progressively:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::screen;

let mut frame = CommandBuffer::new();
frame
    .command(screen::begin_synchronized_update())
    .text("frame contents")
    .command(screen::end_synchronized_update());

assert_eq!(frame.as_bytes(), b"\x1b[?2026hframe contents\x1b[?2026l");
```

**This pair must be detection-gated (FM-V4).** `commands::screen` only builds bytes; it does not
probe whether a terminal understands mode 2026. codex found mode-2026-adjacent sequences leaking
raw onto consoles that do not support them because it emitted unconditionally (codex#24543) — a
caller should probe for mode 2026 support before writing these bytes to a real terminal. Wrap
exactly one full frame per `begin`/`end` pair.

#### Capability-Gated Synchronized Output

The encode-only `begin`/`end` pair above must be detection-gated by the caller. With the optional
`tokio` feature on Unix, `TokioTerminalSession::synchronized` performs that gate — it wraps a frame
in mode 2026 **only when the terminal probed the capability as supported** (R-CAP-4), degrading to an
un-batched frame otherwise, never emitting the 2026 bytes into a terminal that did not answer
(FM-V4). See [Live Query Helpers (Tokio)](crate::docs#live-query-helpers-tokio) for the gated helper
and its example.

Upstream references:

- [Contour synchronized output](https://github.com/contour-terminal/contour/blob/master/docs/vt-extensions/synchronized-output.md)

### Scroll Regions

`commands::screen::set_scroll_region` (DECSTBM) confines subsequent scrolling to a band of rows;
`commands::screen::reset_scroll_region` restores the full viewport. `commands::screen::scroll_up`
and `commands::screen::scroll_down` (SU/SD) scroll within whatever region is active;
`commands::screen::insert_lines` and `commands::screen::delete_lines` (IL/DL) shift lines at the
cursor within the active scrolling area:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::screen;

let mut frame = CommandBuffer::new();
frame
    .command(screen::set_scroll_region(2, 10))
    .command(screen::insert_lines(1))
    .command(screen::scroll_up(1))
    .command(screen::reset_scroll_region());

assert_eq!(frame.as_bytes(), b"\x1b[2;10r\x1b[1L\x1b[1S\x1b[r");
```

SU, SD, IL, and DL all write their count parameter explicitly even at the ECMA-48 default of 1 —
`scroll_up(1)` emits `b"\x1b[1S"`, not the parameter-omitted form.

**DECSTBM is not portable (FM-V2).** It is the core primitive ratatui-shaped `insert_before`
inline-viewport consumers need (R-OUT-6) — the scroll-region-plus-reverse-index shape codex uses to
insert history above a live viewport — but it is *known* to misbehave on some hosts: codex's tui2
postmortem found scroll-region history insertion could drop or duplicate content depending on the
terminal, and xterm.js-based terminals (notably VS Code's integrated terminal) permanently drop
scrollback when a scroll region is set (codex#27644). `commands::screen` builds the bytes only; it
has no capability model and cannot refuse to emit on a host known to be unsafe. Per R-OUT-6,
callers should gate scroll-region emission on an `inline_insertion_safe` capability — backed by the
conformance matrix's per-terminal scroll-region/clear semantics — that a later session/capability
slice adds, rather than assuming DECSTBM is safe everywhere it parses.

Upstream references:

- [xterm `CSI Ps ; Ps r` (DECSTBM)](https://www.xfree86.org/current/ctlseqs.html#:~:text=CSI%20P%20t%20%3B%20P%20b%20r)
- [ECMA-48 SU/SD/IL/DL](https://ecma-international.org/publications-and-standards/standards/ecma-48/)

### Cursor Visibility

`commands::cursor::hide` and `commands::cursor::show` use the widely supported xterm/DEC private
mode for cursor visibility:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::cursor;

let mut frame = CommandBuffer::new();
frame.command(cursor::hide()).command(cursor::show());

assert_eq!(frame.as_bytes(), b"\x1b[?25l\x1b[?25h");
```

Hiding the cursor changes terminal state. Code that writes the hide command to a real terminal
should arrange cleanup that shows the cursor again.

Upstream references:

- [xterm show cursor `DECTCEM`](https://www.xfree86.org/current/ctlseqs.html#:~:text=P%20s%20%3D%202%205%20%E2%86%92%20Show%20Cursor%20(DECTCEM))
- [xterm hide cursor `DECTCEM`](https://www.xfree86.org/current/ctlseqs.html#:~:text=P%20s%20%3D%202%205%20%E2%86%92%20Hide%20Cursor%20(DECTCEM))

### Cursor Save And Restore

`commands::cursor::save` and `commands::cursor::restore` use DEC save and restore cursor controls:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::cursor;

let mut frame = CommandBuffer::new();
frame.command(cursor::save()).command(cursor::restore());

assert_eq!(frame.as_bytes(), b"\x1b7\x1b8");
```

Save and restore use terminal state. Prefer using the pair within a narrow output frame.

Upstream references:

- [xterm `ESC 7`](https://www.xfree86.org/current/ctlseqs.html#:~:text=ESC%207)
- [xterm `ESC 8`](https://www.xfree86.org/current/ctlseqs.html#:~:text=ESC%208)

### Cursor Shape

`commands::cursor::set_shape` encodes DEC's "Set Cursor Style" control, DECSCUSR: `CSI Ps SP q`,
where `Ps` selects a `CursorShape` — `Default`, `BlinkingBlock`, `SteadyBlock`,
`BlinkingUnderline`, `SteadyUnderline`, `BlinkingBar`, or `SteadyBar`, mapping to `Ps` 0 through 6:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::cursor::{self, CursorShape};

let mut frame = CommandBuffer::new();
frame.command(cursor::set_shape(CursorShape::SteadyBar));

assert_eq!(frame.as_bytes(), b"\x1b[6 q");
```

`commands::cursor::reset_shape` emits the same bytes as `set_shape(CursorShape::Default)`
(`CSI 0 SP q`) under a name that documents restore intent at call sites.

Cursor shape is a plain command, not a session-tracked mode. Per FM-L3 (helix#10089, open;
libvaxis#10, #98), no single DECSCUSR value is a universal reset: `Ps` = 0 asks for "the terminal
profile's own default," which is not guaranteed to match whatever shape was active before an
application changed it — helix builds its own restore recipe from terminfo `Se` plus `cnorm` plus
`CSI 0 SP q` rather than trusting one value. qwertty does not pretend otherwise: an application
that changes the cursor shape and needs to restore the exact prior shape should track and restore
it explicitly. See [Screen And Cursor Lifecycle](crate::docs#screen-and-cursor-lifecycle) for how
this differs from the ledger-tracked alternate screen and cursor visibility.

Upstream references:

- [xterm `CSI Ps SP q` (DECSCUSR)](https://www.xfree86.org/current/ctlseqs.html#:~:text=CSI%20P%20s%20SP%20q)

## Styling

`SGR` means "Select Graphic Rendition". `commands::style` builds SGR command bytes for colors and
text attributes. Every helper returns one granular command — a single SGR parameter, or the small
parameter run one color needs — rather than a combined "set everything" call, so a caller composes
exactly the attributes that changed between frames onto a `CommandBuffer`. qwertty does not track
prior style state or diff anything itself.

### Colors

`commands::style::foreground`, `commands::style::background`, and
`commands::style::underline_color` accept a `Color`: the 16 classic named colors, a 256-color
`Color::Indexed`, or a 24-bit `Color::Rgb`:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::style::{self, Color};

let mut frame = CommandBuffer::new();
frame
    .command(style::foreground(Color::Red))
    .command(style::background(Color::Indexed(214)))
    .command(style::underline_color(Color::Rgb(10, 20, 30)));

assert_eq!(
    frame.as_bytes(),
    b"\x1b[31m\x1b[48;5;214m\x1b[58;2;10;20;30m"
);
```

Named colors emit the classic ECMA-48 range (`30`-`37` foreground, `40`-`47` background) or the
widely supported xterm-derived bright range (`90`-`97` foreground, `100`-`107` background).
Indexed and RGB colors always use the semicolon form (`38;5;n`, `38;2;r;g;b`, and the background
and underline-color equivalents), never the colon-subparameter form. The audited failure-mode
survey (FM-W6) found colon-form 8-bit SGR fails in PowerShell/conhost, and non-default underline
color specifically has caused rendering bugs on Windows Terminal and hard failures on Windows 7
hosts serious enough that crossterm feature-gates it — so every color helper here, including
underline color, uses the one widely-supported spelling.

`commands::style::reset_foreground`, `reset_background`, and `reset_underline_color` (SGR 39, 49,
59) restore the terminal default for each slot independently.

### Text Attributes

Boolean attributes each have a setter and, except bold/dim which share one reset, an individual
resetter:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::style;

let mut frame = CommandBuffer::new();
frame.command(style::bold()).text("important").command(style::reset_bold_dim());

assert_eq!(frame.as_bytes(), b"\x1b[1mimportant\x1b[22m");
```

`bold`, `dim`, `italic`, `underline`, `blink`, `reverse`, `hidden`, and `strikethrough` map to SGR
1, 2, 3, 4, 5, 7, 8, and 9. `reset_bold_dim` (SGR 22) clears both bold and dim, since SGR has no
separate reset for either alone; `reset_italic`, `reset_underline`, `reset_blink`,
`reset_reverse`, `reset_hidden`, and `reset_strikethrough` (SGR 23, 24, 25, 27, 28, 29) clear one
attribute each. `commands::style::reset_all` (SGR 0) clears every attribute and color in one
command.

### Underline Styles

`commands::style::underline_style` selects a specific underline substyle with `UnderlineStyle`:
`None`, `Straight`, `Double`, `Curly`, `Dotted`, or `Dashed`, mapping to SGR 4's colon subparameter
form `4:0` through `4:5`:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::style::{self, UnderlineStyle};

let mut frame = CommandBuffer::new();
frame.command(style::underline_style(UnderlineStyle::Curly));

assert_eq!(frame.as_bytes(), b"\x1b[4:3m");
```

Unlike this module's colors, underline substyles have no semicolon-form alternative anywhere in
use: the colon subparameter on SGR 4 is the only widely-implemented spelling, originating with
Kitty/VTE and now shared by xterm, iTerm2, and `WezTerm`.

Upstream references:

- [ECMA-48](https://ecma-international.org/publications-and-standards/standards/ecma-48/)
- [xterm `CSI Pm m` (SGR)](https://www.xfree86.org/current/ctlseqs.html#:~:text=CSI%20P%20m%20m)

## OSC Commands

`OSC` means "Operating System Command", written `OSC Ps ; Pt ST`: `Ps` selects the command, `Pt`
(when present) carries its text, and `ST` (String Terminator) ends it. `commands::osc` builds OSC
command bytes for window titles, hyperlinks, clipboard writes, and shell semantic-prompt marks.
Every helper always emits the 7-bit `ESC \` spelling of `ST`, never the legacy BEL (`0x07`)
terminator some producers use.

### Window Title

`commands::osc::set_title` (OSC 2) sets the window title; `commands::osc::set_icon_and_title`
(OSC 0) sets both the icon name and the window title:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::osc;

let mut frame = CommandBuffer::new();
frame.command(osc::set_title("qwertty"));

assert_eq!(frame.as_bytes(), b"\x1b]2;qwertty\x1b\\");
```

Both helpers sanitize their argument first. Terminal title-report echo has been the exact shape of
several CVEs (`ConEmu` CVE-2022-46387 and its bypass CVE-2023-39150; Windows Terminal's OSC 9;9
working-directory injection, CVE-2022-44702) — a terminal that echoes a title back onto a
controlling program's stdin turns attacker-influenced title text into attacker-controlled input.
`commands::osc::sanitize_title` strips every C0/C1 control character plus a documented
bidi/invisible-formatting blocklist (the "Trojan Source" set: explicit bidi embedding/override and
isolate controls, zero-width and directional marks, `U+2028`/`U+2029` line separators, and the
byte-order-mark code point), and caps the result at 240 characters.

### Hyperlinks

`commands::osc::hyperlink` (OSC 8) opens a hyperlink; text written afterward is a clickable link
until `commands::osc::close_hyperlink` closes it:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::osc;

let mut frame = CommandBuffer::new();
frame
    .command(osc::hyperlink("https://example.com", Some("docs")))
    .text("docs")
    .command(osc::close_hyperlink());

assert_eq!(
    frame.as_bytes(),
    b"\x1b]8;id=docs;https://example.com\x1b\\docs\x1b]8;;\x1b\\"
);
```

The optional `id` groups multiple text spans (for example, a link wrapped across lines) as one
hyperlink. Both the URI and `id` have control bytes stripped before encoding, so neither can
terminate the OSC sequence early.

### Clipboard Write

`commands::osc::set_clipboard` (OSC 52) writes `ClipboardSelection::Clipboard` (target `c`) or
`ClipboardSelection::Primary` (target `p`), base64-encoding the payload:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::osc::{self, ClipboardSelection};

let mut frame = CommandBuffer::new();
frame.command(osc::set_clipboard(ClipboardSelection::Clipboard, b"Hello"));

assert_eq!(frame.as_bytes(), b"\x1b]52;c;SGVsbG8=\x1b\\");
```

**This command is an exfiltration surface, not merely a formatting choice.** OSC 52 lets any text
a terminal displays reach the local clipboard; MITRE ATT&CK catalogs this as T1115, and terminals
increasingly prompt or drop these writes themselves (kitty#9428). `commands::osc` only builds
bytes — it has no policy and cannot prompt — so **a session or application must apply its own
policy gate (opt-in, allowlist, size/rate limiting) before writing this command's bytes to a real
terminal.** Payloads over 100,000 raw bytes are dropped: `set_clipboard` returns an empty command
rather than truncate silently.

### Semantic Prompt Marks

`commands::osc::prompt_start`, `prompt_end` (alias: `command_start`), `command_executed`, and
`command_finished` encode OSC 133 (`FinalTerm`) shell-integration marks, letting a terminal or
multiplexer navigate by prompt/command boundary instead of by raw line:

```rust
use qwertty::CommandBuffer;
use qwertty::commands::osc;

let mut frame = CommandBuffer::new();
frame
    .command(osc::prompt_start())
    .text("$ ")
    .command(osc::prompt_end())
    .text("ls")
    .command(osc::command_executed())
    .text("file.txt\n")
    .command(osc::command_finished(Some(0)));

assert_eq!(
    frame.as_bytes(),
    b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\ls\x1b]133;C\x1b\\file.txt\n\x1b]133;D;0\x1b\\"
);
```

`command_finished` takes an optional exit code: `Some(code)` emits `OSC 133 ; D ; code ST`, `None`
emits `OSC 133 ; D ST`.

Upstream references:

- [xterm Control Sequences (OSC 0/2, OSC 8, OSC 52)](https://www.invisible-island.net/xterm/ctlseqs/ctlseqs.html)
- [terminal-wg specifications, issue 14 (OSC 133 / FinalTerm)](https://gitlab.freedesktop.org/terminal-wg/specifications/-/issues/14)

## External References

The official ECMA-48 standard is published by Ecma International:
<https://ecma-international.org/publications-and-standards/standards/ecma-48/>.

The xterm control-sequence reference is useful for common terminal extensions:
<https://www.xfree86.org/current/ctlseqs.html>.
