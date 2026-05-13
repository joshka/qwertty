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

## External References

The official ECMA-48 standard is published by Ecma International:
<https://ecma-international.org/publications-and-standards/standards/ecma-48/>.

The xterm control-sequence reference is useful for common terminal extensions:
<https://www.xfree86.org/current/ctlseqs.html>.
