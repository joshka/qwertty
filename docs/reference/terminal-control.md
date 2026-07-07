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
the request, flushes output, waits for the matching report, and applies a caller-provided timeout:

```rust,no_run
use std::time::Duration;

use qwertty::TokioTerminalSession;

# async fn run() -> qwertty::Result<()> {
let mut session = TokioTerminalSession::open()?;
let report = session.request_cursor_position(Duration::from_secs(1)).await?;

assert!(report.row() > 0);
assert!(report.column() > 0);

session.leave().await
# }
```

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
the request, flushes output, waits for the matching report, and applies a caller-provided timeout:

```rust,no_run
use std::time::Duration;

use qwertty::TokioTerminalSession;
use qwertty::report::TerminalStatus;

# async fn run() -> qwertty::Result<()> {
let mut session = TokioTerminalSession::open()?;
let report = session.request_terminal_status(Duration::from_secs(1)).await?;

assert_eq!(report.status(), TerminalStatus::Ready);

session.leave().await
# }
```

Unrelated decoded events that arrive before the matching report remain available through
`TokioTerminalSession::next_event`. This is still not a general query router: qwertty does not yet
support multiple simultaneous live queries, capability probing, or query registration.

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

## External References

The official ECMA-48 standard is published by Ecma International:
<https://ecma-international.org/publications-and-standards/standards/ecma-48/>.

The xterm control-sequence reference is useful for common terminal extensions:
<https://www.xfree86.org/current/ctlseqs.html>.
