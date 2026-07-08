# The Async Model

qwertty is async-first, and its terminal queries are race-free by construction. Terminal I/O makes
that harder than it sounds: a reply to a question you asked the terminal — the cursor position, the
device attributes — arrives on the same input stream as the user's keystrokes, and a keystroke can
look exactly like a reply. Libraries that read input and match replies in separate places, or that
bolt an async layer over a blocking core, can drop a reply, mistake typeahead for an answer, or hang
waiting for a terminal that will never respond. This page explains how qwertty avoids all of that.

## A sans-io core, driven by a runtime

The parts of qwertty that interpret bytes do no I/O, keep no clock, and start no threads. Encoding
turns intent into [`Command`](crate::Command) bytes; the [`SyntaxParser`](crate::SyntaxParser) and
[`SemanticDecoder`](crate::SemanticDecoder) turn input bytes into [`Event`](crate::Event) values; and
a query correlator matches replies to the questions that asked for them. Each is a pure state machine
you feed bytes and events.

That purity is the design's foundation. A *driver* owns the actual reading, writing, and timing and
feeds the core. Because the core holds no I/O, the same decode and correlation logic can be tested
in memory and driven by any runtime — and qwertty ships two drivers over one core:

- [`TokioTerminalSession`](crate::TokioTerminalSession), the asynchronous owner, driven by Tokio
  readiness.
- [`TerminalSession`](crate::TerminalSession), the synchronous owner, which drives the same core with
  a blocking poll loop and needs no async runtime at all.

Only *who feeds bytes and time* differs between them. A query answered by the async session and the
same query answered by the synchronous one run through the identical correlator.

## The correlator: matching replies without racing keystrokes

When the session asks the terminal a question, it registers a typed *expectation* with the correlator
and writes the request. Decoded input then flows through the correlator, which completes the
expectation only when a matching reply arrives. Three properties make this race-free:

- **Typed, fully-discriminated matching.** An expectation carries enough of the reply's shape that an
  unrelated report cannot satisfy it: a mode report for one mode never completes a query about a
  different mode, and a colour report never completes a cursor-position query. A keystroke that
  merely resembles a control sequence is decoded as input, not swallowed as an answer.
- **Typeahead survival.** Input the user typed while the query was in flight is not lost. Bytes read
  before the matching reply that are not the reply are preserved in arrival order and delivered by
  the next read — through [`next_event`](crate::TokioTerminalSession::next_event) on the async
  session, or the next `read_input` on the synchronous one.
- **A bounded, honest "no reply".** Not every terminal answers every query. Rather than hang, a probe
  ends its write with a Primary Device Attributes request, which essentially every terminal answers.
  When that answer arrives, any expectation still pending is resolved as *no reply* — but only after
  the current batch of decoded input has fully drained, so a real reply sitting just before the
  device-attributes answer in the same read is never discarded. "No reply" becomes a definite,
  bounded outcome, distinct from an error and from a hang.

## Driving with Tokio readiness

The async session registers a duplicate of the terminal's file descriptor with Tokio's `AsyncFd`.
The duplicate shares the same open file description as the device the session owns, so readiness
observed on either applies to both, and the session can keep ownership of the device while Tokio
watches the descriptor. This dance is also what makes the session work on macOS, where a freshly
opened controlling-terminal descriptor is rejected by the kernel's poller while the inherited one is
accepted; [`acquisition`](crate::TokioTerminalSession::acquisition) reports which path reached the
terminal. Reads and writes happen on the registered descriptor, and the session restores the
descriptor's original blocking flags on exit so the parent shell is unaffected.

## Cancellation safety

Every `async fn` on [`TokioTerminalSession`](crate::TokioTerminalSession) is cancel-safe. All the
state a call touches — the decoder, the correlator, the queue of decoded-but-undelivered events, and
the id of any in-flight query — lives on the session, not in the future's stack. Dropping a future
mid-await (a lost `select!` branch, a timeout, a cancelled task) therefore loses nothing: a later
call resumes from the same state, with a pending query still pending and every buffered event still
available.

## Event delivery, coalescing, and the lone Escape

[`next_event`](crate::TokioTerminalSession::next_event) delivers decoded [`Event`](crate::Event)
values from the queue, reading more input only when the queue is empty. Two timing behaviours live
here because they are the driver's job, not the decoder's:

- **Resize coalescing.** A burst of resize events collapses to a single event carrying the final
  geometry, so a window drag does not flood the application with intermediate sizes; interleaved
  keystrokes keep their order.
- **Lone-Escape flush.** A bare `Esc` byte could begin an escape sequence, so the decoder holds it.
  When only a lone `Esc` is pending and no further input arrives within a bounded window
  ([`set_esc_flush_timeout`](crate::TokioTerminalSession::set_esc_flush_timeout), 25 ms by default),
  it is flushed as `Key::Escape` — so Escape-to-cancel feels immediate. When the kitty keyboard
  protocol is active, Escape arrives unambiguously and the timeout never engages.

## Handing the terminal back

An interactive program does not own the terminal forever. The async session can give it back and
reclaim it cleanly:

- [`suspend`](crate::TokioTerminalSession::suspend) / [`resume`](crate::TokioTerminalSession::resume)
  restore the terminal and stop the process for Ctrl-Z job control, then re-enter raw mode,
  re-assert readiness, and queue a resize on return.
- [`run_detached`](crate::TokioTerminalSession::run_detached) hands the terminal to a synchronous
  child — an `$EDITOR`, a pager, a subshell — releasing the readiness registration and restoring
  blocking mode for the child, then taking a fresh registration and resyncing the terminal
  afterward, never trusting what the child left behind.
- [`signals`](crate::TokioTerminalSession::signals) surfaces `SIGTSTP`/`SIGCONT`/`SIGTERM`/`SIGINT`
  as a stream the application drives its own response to; qwertty installs no handlers itself.

## See also

- [Tokio input ownership](crate::docs::tokio_input_ownership) — the usage guide for owning reads and
  running queries with the async session.
- [Terminal session reference](crate::docs::terminal_session) — the session lifecycle and its methods.
- [Capabilities](crate::docs::capabilities) — how the probe bundle turns query replies into a typed
  capability snapshot.
