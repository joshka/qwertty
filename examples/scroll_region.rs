//! Builds a synchronized-output frame that scrolls new lines into an inline viewport's scroll
//! region — the codex-shaped "history above a live viewport" pattern R-OUT-3/R-OUT-6 target.
//!
//! This mirrors `styled_text.rs`'s shape: pure byte-building against `CommandBuffer`, no terminal
//! opened. It shows the whole encode-only pair set this milestone adds:
//! `commands::screen::begin_synchronized_update`/`end_synchronized_update` (mode 2026, wrapping
//! the frame so a supporting terminal paints it atomically) and
//! `commands::screen::set_scroll_region`/`reset_scroll_region` (DECSTBM, confining
//! `commands::screen::scroll_up` and `commands::screen::insert_lines` to rows 2 through 10).
//!
//! **This example never writes these bytes to a real terminal, and that is deliberate.** Per
//! FM-V4, mode 2026 emission should be gated on probed support before it reaches a real terminal —
//! unconditional 2026 bytes leak raw onto terminals that do not understand them (codex#24543).
//! Per FM-V2, DECSTBM is not portable — xterm.js-based terminals (notably VS Code's integrated
//! terminal) drop scrollback permanently when a scroll region is set (codex#27644) — so R-OUT-6
//! says scroll-region emission should be gated on an `inline_insertion_safe` capability. Neither
//! gate exists at the encode layer: `commands::screen` only builds bytes. A later session/
//! capability slice owns probing for mode 2026 and `inline_insertion_safe` and applying these
//! gates before writing to a live device.

use qwertty::CommandBuffer;
use qwertty::commands::screen;

fn main() {
    let mut output = CommandBuffer::new();
    output
        // FM-V4 caller contract: only write this to a real terminal after probing mode 2026
        // support.
        .command(screen::begin_synchronized_update())
        // FM-V2 caller contract: only write this to a real terminal after confirming
        // inline_insertion_safe for the detected identity.
        .command(screen::set_scroll_region(2, 10))
        .command(screen::insert_lines(1))
        .command(screen::scroll_up(1))
        .command(screen::reset_scroll_region())
        .command(screen::end_synchronized_update());

    assert_eq!(
        output.as_bytes(),
        b"\x1b[?2026h\x1b[2;10r\x1b[1L\x1b[1S\x1b[r\x1b[?2026l"
    );

    print!("{}", String::from_utf8_lossy(output.as_bytes()));
    println!();
}
