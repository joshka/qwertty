# The Alternate Screen

Terminals keep two screen buffers. The **primary** buffer is the normal scrollback — the shell
prompt, command output, everything the user scrolls through. The **alternate** buffer is a separate,
non-scrolling, full-screen surface. A full-screen application (an editor, a pager, a TUI) switches to
the alternate buffer so it can paint the whole screen without disturbing scrollback, then switches
back on exit and the user's scrollback is exactly as they left it.

## The sequence

qwertty enters the alternate screen with `CSI ? 1049 h` and leaves with `CSI ? 1049 l`. Mode 1049
both switches buffers and saves/restores the cursor position, which is what a full-screen app wants.
[`commands::screen::enter_alternate_screen`](crate::commands::screen::enter_alternate_screen) and
[`leave_alternate_screen`](crate::commands::screen::leave_alternate_screen) build these bytes.

There is one subtlety. Some terminals and multiplexers do not clear the alternate buffer on entry, so
a fresh switch can show stale content for a frame. qwertty therefore writes an explicit erase
(`CSI 2 J`) right after entering, so the alternate screen always starts blank.

## Through a session

Prefer the session method over emitting the bytes yourself. A
[`TerminalSession`](crate::TerminalSession) records the alternate-screen state in its mode ledger, so
leaving the alternate buffer is automatic on `leave`, on drop, and from the panic-safe restore path.
A crashing full-screen app still returns the user to their shell and scrollback rather than stranding
them in a blank alternate buffer. Enabling the mode directly with the [`commands`](crate::commands)
helpers puts the cleanup back on you.

The alternate screen carries no scrollback of its own. An application that wants scrollback keeps its
content in the primary buffer instead; see the inline-rendering approach rather than a full-screen
takeover for that case.
