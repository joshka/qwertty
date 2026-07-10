# Why qwertty

qwertty is a Unix-first terminal library built around three bets that the existing libraries do not
all make together: it is **async-first**, its terminal queries are **race-free by construction**, and
it treats **what a terminal actually supports as measured evidence** rather than a guess. If those are
not properties you need, one of the mature alternatives below is very likely the better choice — this
page is about what qwertty adds, and, just as importantly, where it deliberately does less.

The landscape it is entering is a good one. [crossterm] is the de-facto default for Rust terminal
apps and the backbone of most [ratatui] programs; [termion] is a tiny, dependency-free Unix classic;
[termwiz] is the batteries-included engine behind [wezterm]; and [termina] is a modern, typed,
protocol-visible library that [helix] adopted. qwertty overlaps all of them and replaces none of them
wholesale.

## What qwertty adds

### One async-first core, not an async bolt-on

qwertty's encode, decode, and query-correlation logic is a single runtime-neutral, side-effect-free
core. That same core is driven either by the blocking [`TerminalSession`] or, under the `tokio`
feature, by the async `TokioTerminalSession` — the two share identical decode and query logic, so an
async application is not running a different, less-tested path. The alternatives do offer async or
non-blocking input in various forms — an optional event stream, a non-blocking reader on a background
thread — but as facilities layered onto a synchronous core; in qwertty the sans-io core is the
primary design, and the blocking and async sessions are two thin drivers over the same logic. See the
async-model reference for how that core is built.

### Queries that cannot mistake a keystroke for an answer

When an application asks the terminal a question — cursor position, device attributes, a capability
probe — the reply arrives on the same input stream as the user's typing. The common approach is to
issue the request and then read the stream until something reply-shaped appears, which can consume or
be corrupted by typeahead the user sent in the meantime.

qwertty instead runs a **correlator**: a request registers a typed, fully-discriminated expectation,
and only a reply that matches it is paired off; anything else — including query-shaped bytes that are
really input — flows through as a normal [`Event`]. Unanswered queries are resolved deterministically
by fencing the batch behind a device-attributes request rather than by a timeout guess. The result is
that live queries and user input coexist without either eating the other. This is qwertty's signature
property; the [report parsers](crate::report) turn the matched replies into typed values such as
[`CursorPositionReport`].

### Ownership you can trust to unwind

A [`TerminalSession`] enters raw mode through a **mode ledger** and undoes exactly what it enabled —
on `leave`, on drop, and from a panic hook via the Unix-only [`RestoreHandle`]. There is no
process-global raw-mode toggle to leave dangling, and sessions are re-entrant. A program that panics
mid-render still returns the user to a working shell with their scrollback intact, because the restore
path replays the ledger in reverse rather than hoping a fixed teardown string is enough.

### Lossless, typed decoding that never guesses

Input passes through a [`SyntaxParser`] that produces lossless [`SyntaxToken`] spans — it preserves
APC, PM, and SOS strings, bounds its own memory, and never guesses at an ambiguous escape — and then
a [`SemanticDecoder`] maps those to a frozen, typed [`Event`] vocabulary ([`KeyEvent`], mouse, focus,
paste, resize). Anything the semantic layer does not recognise passes through as [`Event::Syntax`]
rather than being dropped or turned into a fabricated event, so an application can always see the raw
truth beneath the typed view.

### Capabilities as evidence, and a canonical sequence database

qwertty does not return a bare boolean for "does this terminal support X." A [`Finding`] carries its
[`Evidence`] — whether the answer was **probed** from the live terminal, **inferred** from identity
and environment, or is genuinely **unknown** — so an application can tell a measured yes from a
hopeful one. Underneath sits a canonical, cited database of terminal sequences and a conformance
harness that captures real terminal behaviour; the long-term goal is a generated "caniuse + MDN for
terminals" reference. No other library in this list ships terminal-capability data as inspectable,
sourced evidence. See the [capability model](crate::docs::capabilities).

### Security as an explicit gate

Sequences that touch the world outside the screen — clipboard writes via OSC 52, for instance — are
mediated by an explicit [`Policy`] and [`PolicyGate`], and window titles and OSC payloads are
sanitised so a hostile byte stream cannot smuggle control sequences through them. The default policy
is restrictive; loosening it is a visible, deliberate choice in the code.

## How it compares

A rough orientation first (concurrency described as each library's primary shape, not its only
option — several offer non-blocking or async facilities as well):

| Library   | Platforms              | Concurrency model                              |
| --------- | ---------------------- | ---------------------------------------------- |
| crossterm | Unix + Windows         | sync core; optional async event stream         |
| termion   | Unix                   | sync; thread-backed non-blocking reader        |
| termwiz   | Unix + Windows         | sync; blocking or non-blocking input           |
| termina   | Unix + Windows         | synchronous (per its current docs)             |
| qwertty   | Unix (Windows planned) | async-first sans-io core; blocking session too |

**vs. [crossterm].** crossterm's reach is its strength: it runs on Windows as well as Unix, it has by
far the largest ecosystem, and it is the default backend for ratatui. qwertty is Unix-first today —
Windows is a goal, not a rejection — and spends its focus on the things above: an async-first core,
correlated queries, evidence-backed capabilities, and ledger-based restore. Choose crossterm for
portability and ecosystem today; reach for qwertty when you are building an async Unix application
whose correctness depends on reliable queries and clean teardown.

**vs. [termwiz].** termwiz is far more than an ownership layer: it carries a terminfo database, a
cell-and-surface model, a line editor, image protocols, and widgets — it is the toolkit behind a full
terminal emulator. qwertty is deliberately narrower. It owns the terminal and types the protocol in
both directions, and leaves rendering and widgets to layers above it. If you want a batteries-included
surface-and-widget stack, termwiz; if you want a focused, async ownership-and-protocol foundation to
build a renderer on, qwertty.

**vs. [termion].** termion is minimal and dependency-free, which is a real virtue for small Unix
programs. Its async story is a thread-backed non-blocking reader rather than correlated queries, it
does comparatively little to protect you from a botched teardown, and it reads query replies inline.
qwertty is a larger dependency that does more: async-first, panic-safe restore,
correlated queries, typed lossless decode. For a tiny filter or prompt, termion is lighter; for a
long-running interactive application, qwertty's guarantees start to pay for themselves.

**vs. [termina].** termina is the closest in spirit — it, too, keeps the terminal protocol visible,
letting you write typed CSI/OSC/DCS and read typed events instead of hand-decoding bytes, and it is
maintained and cross-platform. The differences are architectural rather than philosophical: termina
is cross-platform, whereas qwertty is async-first and Unix-first (Windows is a goal), and qwertty
builds the race-free query correlator and the evidence-backed capability model in as first-class
mechanisms rather than leaving reply-matching and capability inference to the application. If you want
a typed, portable, cross-platform library today, termina is an excellent choice; qwertty is the bet
for async Unix applications that lean hard on queries and capability data.

## What qwertty does not do

Being honest about the boundaries is part of the pitch:

- **Windows isn't here yet.** Live terminal ownership is Unix-only today, though the encode and decode
  layers already compile everywhere. Windows support is a goal rather than a rejected one — but until
  it lands, a Windows console app wants crossterm, termwiz, or termina. See
  [platform support](crate::docs::platform).
- **It is not a widget toolkit.** qwertty has no surfaces, cells, layout, or widgets. It is the layer
  a renderer sits on, not the renderer.
- **It is young.** qwertty is pre-1.0 with a small ecosystem; crossterm and termwiz have years of use
  behind them. The capability database and conformance tooling that back the evidence model are still
  growing.

If you are building an async, Unix-first terminal application — especially one that queries the
terminal, adapts to its real capabilities, and must always leave it clean — qwertty is built for
exactly that. Otherwise, the alternatives above are excellent, and this page has hopefully pointed you
at the right one.

[`TerminalSession`]: crate::TerminalSession
[`RestoreHandle`]: crate::RestoreHandle
[`SyntaxParser`]: crate::SyntaxParser
[`SyntaxToken`]: crate::SyntaxToken
[`SemanticDecoder`]: crate::SemanticDecoder
[`Event`]: crate::Event
[`Event::Syntax`]: crate::Event::Syntax
[`KeyEvent`]: crate::KeyEvent
[`CursorPositionReport`]: crate::CursorPositionReport
[`Finding`]: crate::Finding
[`Evidence`]: crate::Evidence
[`Policy`]: crate::Policy
[`PolicyGate`]: crate::PolicyGate
[crossterm]: https://crates.io/crates/crossterm
[termion]: https://crates.io/crates/termion
[termwiz]: https://crates.io/crates/termwiz
[termina]: https://crates.io/crates/termina
[ratatui]: https://crates.io/crates/ratatui
[wezterm]: https://wezfurlong.org/wezterm/
[helix]: https://helix-editor.com/
