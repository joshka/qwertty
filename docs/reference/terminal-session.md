# Terminal Session Reference

`TerminalSession` is the first application-facing owner above the low-level terminal device. It
opens or accepts a `Terminal`, enters raw mode, writes output bytes in method-call order, reads raw
input bytes, flushes explicitly, and restores cooked mode through an explicit `leave` path.

## Lifecycle

Use `TerminalSession::open` for the current controlling terminal, or `TerminalSession::from_terminal`
when embedding code or tests have already opened the terminal device.

Starting a session enters raw mode. Raw mode disables canonical input processing and local echo at
the operating-system terminal boundary. The first session slice does not enter the alternate screen,
hide the cursor, enable mouse tracking, enable bracketed paste, write graphics, touch the clipboard,
or change vendor-specific protocol state.

## Output Ordering

The session writes command bytes, raw bytes, and text bytes immediately in the order its methods are
called:

```rust,no_run
use qwertty::{ProtocolPosition, TerminalSession, commands};

# fn main() -> qwertty::Result<()> {
let mut session = TerminalSession::open()?;
session
    .command(commands::screen::clear())?
    .command(commands::cursor::move_to(ProtocolPosition::ORIGIN))?
    .text("Ready\r\n")?
    .flush()?;
session.leave()?;
# Ok(())
# }
```

The example writes these bytes before flushing:

```text
ESC [ 2 J ESC [ 1 ; 1 H R e a d y CR LF
```

In byte form:

```text
\x1b[2J\x1b[1;1HReady\r\n
```

`TerminalSession::text` writes UTF-8 bytes verbatim. It does not escape control characters, remove
escape sequences, or enforce a text policy. Renderers that accept user-controlled text should apply
their own escaping policy before writing to the session.

## Input Bytes

`TerminalSession::read_input` reads one chunk of raw terminal input bytes into a caller-provided
buffer and returns those bytes as `InputBytes`. It does not parse keys, UTF-8, Escape sequences,
query responses, paste, mouse input, or vendor protocols. See the
[terminal input reference](crate::docs) for the input byte contract.

## Flush And Leave

`TerminalSession::flush` reports output flushing errors. Call it when prior writes must be visible
before later application work continues.

`TerminalSession::leave` replays the session's mode ledger: every reversible state change the
session made is undone in reverse enablement order, every step is attempted even after a failure,
and the first error is reported. The replay ends with a flush so restoration bytes never sit in a
buffer. The ledger holds raw-mode restoration and the input-mode enables described in [Input
Modes](#input-modes); alternate screen, cursor visibility, and vendor protocol cleanup join it in
later slices.

## Input Modes

`enable_mouse(MouseMode)`, `enable_focus_events()`, and `enable_bracketed_paste()` turn on the
terminal reporting modes whose events the decoder produces (`Event::Mouse`, `Event::Focus`,
`Event::Paste`). Each writes its DEC private-mode set (`CSI ? N h`) to the terminal now and records
a byte-based ledger entry whose apply re-emits the set on a later `enter` and whose undo emits the
reset (`CSI ? N l`). Because these are byte-based entries, `leave` writes the resets in reverse
enablement order, and the reset bytes flow into the panic-safe emergency blob automatically — a
panic teardown turns the modes back off, not just an orderly `leave`. `enable_mouse` always pairs
the chosen tracking mode (1000/1002/1003) with SGR extended coordinates (1006), and re-recording a
different `MouseMode` replaces the entry in place so switching tracking modes never leaves a stale
one enabled. The same methods are available on the Tokio session (`TokioTerminalSession`), which
writes the enable bytes through its readiness path before recording the ledger entry.

The lifecycle is re-entrant: `leave` does not consume the session, and
`TerminalSession::enter` re-applies the recorded state afterwards. A line-editor-shaped caller
cycles the pair once per prompt over one long-lived session; each transition replays mode actions
only and never reopens the device, so cycling stays as cheap as the mode changes themselves.
Sessions also run headless over any `TerminalDevice` through `TerminalSession::from_device` — the
`session_cycles.rs` example drives the full lifecycle against a `FakeDevice`.

## Security Policy

Some terminal features do more than paint the grid: they reach the system clipboard, pull data back
from the terminal, transfer files, raise desktop notifications, or wrap sequences for a multiplexer
to pass through. Those are exactly the operations an attacker who controls a program's *output*
wants to reach — a log line that quietly writes the clipboard (FM-X4) is an exfiltration primitive,
not a formatting choice. The session carries a `Policy` value that gates them (R-SEC-1).

A new session starts at `Policy::restricted()`, the safe default. Read it with
`TerminalSession::policy` (which returns a `Copy` value) and change it with
`TerminalSession::set_policy` (chains, returns `&mut Self`) or the builder-style
`TerminalSession::with_policy` (returns the session by value). Every field is public, so an app can
also build a policy by hand.

The presets form a ladder from safe-by-default to fully trusted:

| Preset          | Clipboard write | Clipboard read | Notifications | File transfer | Mux passthrough |
| --------------- | --------------- | -------------- | ------------- | ------------- | --------------- |
| `restricted()`  | on              | off            | off           | off           | off             |
| `interactive()` | on              | off            | on            | off           | on              |
| `trusted()`     | on              | on             | on            | on            | on              |

`Default` returns `restricted()`. Clipboard **write** is on even in `restricted` because the
terminal itself gates the sensitive paste-back direction (FM-X4, kitty#9428), so a write here cannot
silently reach the user. The surfaces that *read* or *exfiltrate* — clipboard read, file transfer —
open only at `trusted`; `interactive` widens `restricted` with notifications and mux passthrough for
a locally-trusted interactive app, but not those reads.

Gated session methods consult the policy through `Policy::allows(PolicyGate)` before emitting. When
a gate is off, the method returns `Error::PolicyDenied { gate }` — a teachable error naming the gate
(OQ-4) — **without writing anything**. The generic `command`, `bytes`, and `text` methods stay
ungated: they write exactly what the caller encoded.

`set_clipboard(selection, data)` is the first wired gate. It checks `PolicyGate::ClipboardWrite`,
and on success emits `commands::osc::set_clipboard(selection, data)` (OSC 52) through the same
immediate-write path as `command`, returning `Ok(self)` so it chains:

```rust,no_run
use qwertty::commands::osc::ClipboardSelection;
use qwertty::{Error, Policy, TerminalSession};

# fn main() -> qwertty::Result<()> {
let mut session = TerminalSession::open()?.with_policy(Policy::trusted());

// Allowed under a trusted (or the default restricted) policy.
session
    .set_clipboard(ClipboardSelection::Clipboard, b"copied")?
    .flush()?;

// A policy with clipboard write off denies the call before any bytes are written.
session.set_policy(Policy {
    clipboard_write: false,
    ..Policy::restricted()
});
match session.set_clipboard(ClipboardSelection::Clipboard, b"secret") {
    Err(Error::PolicyDenied { gate }) => eprintln!("denied by policy: {gate}"),
    other => {
        other?;
    }
}

session.leave()
# }
```

The `clipboard_policy.rs` example runs both paths headless over a `FakeDevice`. Clipboard read, file
transfer, notifications, and mux passthrough are policy fields today; their session methods join as
those features land.

## Screen And Cursor Lifecycle

`enter_alternate_screen()` switches to the alternate screen buffer for full-screen applications.
It writes `CSI ? 1049 h` **followed by an explicit `CSI 2 J` clear** and records both as one
ledger entry's apply action (`ModeKind::AlternateScreen`), so a later `enter` replays the pair.
The undo action is the plain leave sequence, `CSI ? 1049 l`, written on `leave`/drop/emergency —
never a matching clear, since the primary screen was never touched while alternate.

The explicit clear is deliberate, not decorative (R-OUT-3, design 01 evidence): mode 1049 usually
clears the alternate buffer implicitly, but mosh does not, and helix works around exactly this by
emitting its own clear right after entering. Without it, a host that skips the implicit clear can
show stale content through the new alternate buffer until the application's first frame overwrites
every cell. qwertty follows that evidence rather than trusting the terminal's own 1049 behavior.

```rust,no_run
use qwertty::TerminalSession;

# fn main() -> qwertty::Result<()> {
let mut session = TerminalSession::open()?;
session.enter_alternate_screen()?;
session.text("full-screen frame\r\n")?.flush()?;
session.leave()
# }
```

`hide_cursor()` writes `CSI ? 25 l` and records a ledger entry (`ModeKind::CursorVisibility`)
whose undo shows the cursor again (`CSI ? 25 h`) on `leave`/drop/emergency. Hiding is the tracked
state (FM-L3): a session that hides the cursor is guaranteed to show it again on every exit path,
whether or not the application calls `show_cursor` itself. `show_cursor()` writes `CSI ? 25 h`
immediately and is **not** itself ledger-tracked — the visible cursor is the safe default state,
so there is nothing to undo on leave; calling it after `hide_cursor()` makes the cursor visible
right away, and the still-present hide entry writes one more redundant, harmless show on the next
`leave`.

Cursor shape (DECSCUSR, `commands::cursor::set_shape`) is a plain command, not a ledger-tracked
mode: a shape change has no single universal reset (FM-L3 — helix#10089, libvaxis#10/#98; no one
DECSCUSR value restores every terminal profile's prior shape). `commands::cursor::reset_shape()`
emits `CSI 0 SP q`, the terminal-profile-default request, but an application that changes the
cursor shape and cares about restoring the exact prior shape should track and restore it
explicitly rather than rely on `reset_shape` alone. See [Cursor
Shape](crate::docs#cursor-shape).

Both are available on the Tokio session (`TokioTerminalSession`) with the same names and the same
ledger semantics, writing through its readiness path before recording the ledger entry. See the
`alternate_screen.rs` example for the full enter/hide/write/leave cycle.

Restoration runs at most once per entered period. Whichever of `leave`, drop, or the panic-safe
restore handle runs first performs it; the others skip. Dropping an entered session still
restores the terminal, but drop-time failures cannot be reported.

Flush explicitly before `leave` when the visibility ordering of your own output matters.

## Panic-Safe Restore

On Unix, `TerminalSession::restore_handle` returns a `RestoreHandle`: a cheap, cloneable handle
that restores the terminal without borrowing the session, built for panic hooks.

```no_run
use qwertty::TerminalSession;

fn main() -> qwertty::Result<()> {
    let mut session = TerminalSession::open()?;

    let restore = session.restore_handle();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        _ = restore.restore();
        previous(info);
    }));

    session.leave()
}
```

The emergency path is precomposed: the session keeps the handle's teardown bytes current as its
state changes, so the hook only writes bytes and restores the captured terminal mode. Writes are
bounded, so a stalled terminal cannot hang the hook. A panic hook covers unwinding panics on any
thread; it does not run on `abort` or fatal signals. See the `panic_safe_restore.rs` example.

## Async Boundary And Live Queries

The runtime-neutral session above is the whole default-feature surface. The async boundary —
`TokioTerminalSession`, the event loop, and the live cursor-position, terminal-status, and
capability-probe query helpers — lives in [Terminal Session: Async Boundary And Live
Queries](crate::docs#terminal-session-async-boundary-and-live-queries), included with the
optional `tokio` feature. See also [Tokio Input Ownership And Query Handoff](
crate::docs#tokio-input-ownership-and-query-handoff) for the single-owner model and handoff
pattern.

## Platform Support

The live terminal implementation currently supports Unix. Unsupported platforms expose the same
public types where possible and return `Error::Unsupported` for live terminal operations.

See [Platform Support](crate::docs#platform-support) for the current Unix-first support boundary
and the documented unsupported behavior on other platforms.
