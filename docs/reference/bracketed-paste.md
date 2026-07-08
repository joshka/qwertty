# Bracketed Paste

When a user pastes text into a terminal, the bytes arrive on the input stream exactly as if they were
typed. That is a problem: an application cannot tell a pasted newline (which should be inserted as
text) from a typed Enter (which should submit), and a shell that runs pasted content line by line can
execute something the user only meant to review. **Bracketed paste** (DEC private mode 2004) fixes
this. With it enabled, the terminal wraps pasted content in `ESC [ 200 ~` … `ESC [ 201 ~`, so the
application knows the bytes between the markers are pasted, not typed.

## Decoded as segments

qwertty decodes bracketed paste into [`PasteEvent`](crate::PasteEvent) values. A paste is delivered
as one or more segments rather than a single event, because a paste can be large — many megabytes —
and buffering all of it before delivering anything would stall the event loop. Each segment reports
where it sits in the paste:

- `is_first` / `is_final` mark the boundary segments, so an application that wants the whole paste can
  accumulate from first to final.
- `terminated` says whether the paste closed cleanly (`ESC [ 201 ~` seen) or the stream ended
  mid-paste; the final segment of an unterminated paste carries `terminated == false`.

The content is preserved byte-exactly, including any escape sequences embedded in the pasted text —
those are never re-interpreted as terminal control. Carriage returns are normalized to newlines, and
[`contains_control`](crate::PasteEvent::contains_control) lets an application inspect a segment for
control bytes before acting on it (paste hygiene).

## Enabling

[`enable_bracketed_paste`](crate::TerminalSession::enable_bracketed_paste) turns on mode 2004 and
records it in the session ledger, so it is turned back off on `leave`, on drop, and from a panic. Once
on, pasted text arrives as [`Event::Paste`](crate::Event) segments and typed keys keep arriving as
[`Event::Key`](crate::Event), so the two are never confused.
