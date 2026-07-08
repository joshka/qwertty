# Mouse Modes

Terminal mouse reporting is two independent choices: **what** movements the terminal reports (the
tracking mode), and **how** it encodes the coordinates (the encoding). qwertty pairs them for you and
decodes the result into [`MouseEvent`](crate::MouseEvent) values.

## Tracking modes: what gets reported

Each tracking mode is a DEC private mode that widens what the terminal sends. They are a ladder —
higher modes report strictly more:

- **Mode 1000** — button press and release only.
- **Mode 1002** — press/release plus drag: motion reported while a button is held.
- **Mode 1003** — press/release plus all motion, even with no button held.

Choose the least you need. Mode 1003 reports every cursor move, which is a lot of input for an
application that only cares about clicks; 1002 is the common choice for click-and-drag interfaces.
Scroll-wheel events arrive under all three.

## Encoding: how coordinates are sent

The original mouse encoding packs each coordinate into a single byte, which cannot express a column
or row past 223 — a real limit on today's wide terminals. The **SGR encoding** (mode 1006) reports
coordinates as decimal parameters (`CSI < button ; column ; row M` for press/drag, `m` for release),
so there is no coordinate ceiling and press and release are unambiguous. qwertty decodes the SGR form
only; it is the encoding every current terminal supports.

## Enabling through a session

[`enable_mouse`](crate::TerminalSession::enable_mouse) takes the tracking mode you want and **always
pairs it with SGR (1006)**, so you never have to enable the encoding separately or worry about the
223-column limit:

```rust,no_run
use qwertty::{TerminalSession, commands::terminal::MouseMode};

# fn main() -> qwertty::Result<()> {
let mut session = TerminalSession::open()?;
session.enable_mouse(MouseMode::ButtonEvent)?; // 1002 + 1006
# session.leave()
# }
```

The mode is recorded in the session's ledger, so it is turned back off on `leave`, on drop, and from
the panic-safe restore path. Re-recording a different [`MouseMode`](crate::MouseMode) replaces the
entry in place, so switching tracking modes never leaves a stale one enabled.

## The decoded events

Each report decodes to one [`MouseEvent`](crate::MouseEvent) carrying the
[`MouseEventKind`](crate::MouseEventKind) (press, release, drag, motion, or scroll), the
[`MouseButton`](crate::MouseButton), and one-based column and row. Scroll events are **not** coalesced
— every wheel notch is its own event, so an application can react to each. Reports the decoder does
not recognize pass through losslessly as [`Event::Syntax`](crate::Event) rather than becoming a fake
click.
