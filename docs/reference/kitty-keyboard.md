# Kitty Keyboard Progressive Enhancement

The legacy terminal keyboard encoding cannot express many things a modern application wants: a bare
Escape is ambiguous with the start of an escape sequence, Ctrl-I is indistinguishable from Tab, key
releases are not reported, and there is no way to see the shifted or base-layout form of a key. The
**kitty keyboard protocol** fixes these by reporting keys as unambiguous `CSI u` sequences. It is a
*progressive enhancement*: the application asks for the behaviours it wants, and the terminal turns
on the subset it supports.

## Flags, not a switch

There is no single "enable kitty" toggle. The application pushes a set of
[`KittyKeyboardFlags`](crate::KittyKeyboardFlags), each bit turning on one reporting behaviour:

- disambiguate escape codes — Escape and colliding modified keys report an unambiguous `CSI u` form,
  which removes the bare-Escape timing guess;
- report event types — press, repeat, and release become distinct;
- report alternate keys — the shifted-key and base-layout-key of each press;
- report all keys as escape codes, and report associated text.

The terminal enables the subset it supports, so after pushing you cannot assume you got what you asked
for.

## Push, then verify

Because it is a progressive enhancement, qwertty separates the raw push from the verification:

- [`push_kitty_keyboard`](crate::TerminalSession::push_kitty_keyboard) is the narrow primitive: it
  writes `CSI > flags u` and records the matching pop for teardown, with no query. Use it when you
  want to drive the readback yourself, at your own timing, with
  [`commands::terminal::query_kitty_keyboard_flags`](crate::commands::terminal::query_kitty_keyboard_flags)
  and the reply parsers.
- On the Tokio session, `request_kitty_keyboard(flags, timeout)` is the verify-after-push convenience:
  it pushes, queries what the terminal actually granted, and records the **granted** set so teardown
  pops reality rather than intent. Its result is a
  [`KittyKeyboardGrant`](crate::KittyKeyboardGrant).

## Unknown is not unsupported

Over an old terminal, or a multiplexer that swallows the exchange, the query may get no answer. That
is **unknown**, not unsupported: the grant reports [`is_unknown`](crate::KittyKeyboardGrant::is_unknown),
nothing is recorded, and no enhancement is assumed. Degrading to plain legacy input is the safe
direction — worst case you lose an enhancement, but you never corrupt input by assuming a behaviour
the terminal never turned on. A granted set that is merely *smaller* than requested is different again:
the application must cope with the flags it did not get.
