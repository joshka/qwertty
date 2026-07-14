# Terminal I/O Failure Modes

Terminal applications talk to the terminal over one byte stream in each direction, with no
framing, no message ids, and decades of accumulated protocol. Most of the bugs people hit — in
every language and library — are instances of a small set of failure modes that follow directly
from that shape. This page names them, so you can recognise one when it bites and know which
mechanism addresses it. It is the short, public version of the failure catalog qwertty was
designed against.

## A reply and a keystroke share one wire

The classic. An application writes a query — cursor position, a kitty graphics command expecting
an acknowledgement, a device-attributes request — and reads the input stream for the reply.
But the user is typing, so the bytes that arrive are `j`, half a reply, another `j`, the rest of
the reply. Read "until something reply-shaped appears" and one of two things happens: the reply
parser chokes on the interleaved keystroke, or the keystrokes are silently consumed by the query
path and the application loses input. Holding a key during a query makes it near-certain.

The fix is architectural, not a parsing tweak: every byte goes through one decoder, and a
**correlator** pairs replies with the specific outstanding request while everything else —
including the keystrokes that arrived mid-reply — flows through as ordinary [`Event`]s. Nothing
is consumed on suspicion. This is qwertty's core design; both sessions drive the same correlator,
so the blocking and async paths cannot disagree.

## The wrong reply completes the wrong question

Ask two questions and match answers loosely, and answer B completes question A. A `DECRQM` reply
for mode 2026 must not satisfy a pending mode-2027 query; worse, some replies are *shaped like
input* — an unmodified `CSI 1;1R` cursor report is byte-identical to a modified F3 key in some
terminals. qwertty's expectations are typed and fully discriminated (a mode reply must carry the
asked-for mode number to match), and the genuinely ambiguous forms are refused rather than
guessed: they pass through as input instead of being claimed as answers.

## Silence is ambiguous

Some terminals answer a query; some ignore it; a multiplexer in between may swallow it. Waiting
"a bit longer" cannot distinguish *unsupported* from *slow*, and a fixed timeout is wrong on both
sides of its value. The trick qwertty uses: send the optional queries, then a **fence** — a
primary device-attributes request, which effectively every terminal answers. When the fence's
reply arrives, everything asked before it that has not answered is resolved as *no reply* — a
deterministic outcome, not a timeout guess. Silence is then recorded honestly: a [`Finding`] of
unknown [`Evidence`], never a fabricated "unsupported".

## The late reply arrives after you gave up

Time a query out, move on, and the reply lands in a later read — where naive code treats those
bytes as keystrokes (escape-sequence garbage on screen or phantom key events). The correlator
keeps expired expectations resolvable: a late reply is still recognised as reply-shaped for the
question that was actually asked, instead of leaking into the input stream.

## Escape is a prefix of everything

A lone `ESC` byte is the Escape key — and also the first byte of every escape sequence. On a
byte stream you cannot know which until more bytes arrive (or don't). Deliver instantly and
you split real sequences apart when they arrive across network/scheduler boundaries; wait forever
and the Escape key is dead. This needs an explicit, bounded flush policy — qwertty's async session
holds an ambiguous lone `ESC` for a short configurable window
(`set_esc_flush_timeout`) — and the kitty keyboard protocol removes the ambiguity entirely
where the terminal grants it (see [kitty keyboard](crate::docs::kitty_keyboard)).

## Sequences split across reads

`read()` returns whatever bytes are available; a mouse report or paste chunk can be cut anywhere,
including mid-sequence. Parsers that assume "one read = whole sequences" break under load,
over ssh, or in a small buffer. The decoder must be incremental and total: qwertty's
[`SyntaxParser`] carries partial state across feeds, is fuzz-verified to reconstruct its input
byte-exactly under *every* split, and never guesses at an incomplete sequence (see
[terminal input](crate::docs::terminal_input)).

## Paste looks like typing — and can look like anything else

Pasted bytes arrive exactly as if typed: a pasted newline "submits", and pasted content can even
contain reply-shaped or escape-shaped bytes. With bracketed paste enabled, qwertty captures the
paste opaquely at the syntax layer — embedded escapes are data, never re-interpreted — and
delivers it as [`PasteEvent`] segments distinct from key events, with a control-byte inspection
hook for paste hygiene (see [bracketed paste](crate::docs::bracketed_paste)).

## Raw mode outlives your process

Enable raw mode, alternate screen, mouse reporting — then panic. The shell your user returns to
is unusable, mouse escapes spray the screen, and scrollback is gone. Restoration must be owned
by construction: a [`TerminalSession`] records every mode it enables in a ledger and undoes
exactly that set on `leave`, on drop, and from a panic hook via the pre-armed [`RestoreHandle`]
— including from the middle of a panic, where allocating or re-negotiating is off the table.

## Async bolted onto a blocking stream

Waking a future and *then* reading a blocking fd opens races a synchronous loop never sees: two
tasks polling one stream steal bytes from each other; a cancelled read drops bytes already pulled
into a buffer; readiness registered on one file description while another is read never wakes.
An async terminal layer has to be designed for cancellation safety and single ownership of the
read path — qwertty's Tokio session drives the same sans-io core as the blocking one over a
single owned reader, so cancellation cannot lose decoded state. With the `tokio` feature enabled,
the async-model reference page documents this machinery in depth.

## Detection by guesswork

`TERM` lies, `COLORTERM` is a convention, device-attribute replies overclaim (a terminal
advertising sixel support in DA1 frequently doesn't render it), and a multiplexer answers
identity queries for itself. Any capability system built on one signal fabricates certainty.
qwertty's [`Capabilities`] carry per-finding [`Evidence`] — probed, inferred, or unknown — and
the conformance database behind the [support summary](crate::docs::conformance) records what real terminals
*measurably* do, silence included. Dumb terminals (`TERM=dumb`, the Linux console) are detected
before a single probe byte is written, because a terminal that does not parse escapes echoes
your probe as garbage.

---

None of these are exotic: every one was harvested from real bugs across the ecosystem's terminal
libraries and applications, and several routinely co-occur (a held key during a graphics
acknowledgement is the first two at once). If you are debugging interleaved-input corruption in
any stack, start by asking which of these your event path assumes away.

[`Event`]: crate::Event
[`Finding`]: crate::Finding
[`Evidence`]: crate::Evidence
[`Capabilities`]: crate::Capabilities
[`SyntaxParser`]: crate::SyntaxParser
[`PasteEvent`]: crate::PasteEvent
[`TerminalSession`]: crate::TerminalSession
[`RestoreHandle`]: crate::RestoreHandle
