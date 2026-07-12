# ADR 0022: Windows Console Support Model

## Status

Accepted

## Context

ADR 0013 made live terminal ownership Unix-first, with Windows planned. The pre-freeze readiness
spike (2026-07-07) proved the public session API needs no signature change for Windows; what
remained open was the concrete support model: which Windows versions, which console input/output
mechanism, and how async I/O works on a platform with no pollable terminal descriptor.

The evidence base, gathered 2026-07-11:

- Console handles support neither overlapped I/O nor IOCP, by design — there is no kernel device
  behind them. `mio` has no console support and libuv uses threads for Windows console I/O. The
  console **input handle is a waitable object**: `WaitForMultipleObjects` signals when input
  records are pending, at which point `ReadConsoleInputW` returns immediately without blocking.
- With `ENABLE_VIRTUAL_TERMINAL_INPUT` set, the console host re-encodes keyboard input as
  xterm-style VT byte sequences — but there is **no VT resize sequence** on Windows: in-band resize
  (DEC mode 2048) is an open, unassigned Windows Terminal backlog item (microsoft/terminal#19618).
  Resize arrives only as `WINDOW_BUFFER_SIZE_EVENT` input records. A `ReadFile` byte-stream read
  discards those records; a `ReadConsoleInputW` record read yields both the VT bytes (packed in
  `KEY_EVENT_RECORD`s) and the resize events.
- Windows 11 is ~73% of the Windows installed base (2026-02, StatCounter) and the Windows 10
  remainder is effectively all 22H2. Build 10.0.17763 (1809) — the ConPTY floor Microsoft,
  helix/termina, and hex1b all chose — covers the entire supported population. Classic conhost on
  1809+ honors both VT mode flags once the application sets them, so requiring VT does not mean
  requiring Windows Terminal.
- win32-input-mode (`CSI ?9001h`) is all-or-nothing, silently disabled by RIS, and absent under
  VS Code/xterm.js; the kitty keyboard protocol is supported by Windows Terminal ≥ 1.25 (2026-03)
  and already has probe-by-readback support in qwertty.

## Decision

1. **Floor: Windows 10 build 10.0.17763 (1809), 64-bit.** No support claim below it; no runtime
   version sniffing beyond honest `SetConsoleMode` failure handling.
2. **VT-only public surface.** The device enables `ENABLE_VIRTUAL_TERMINAL_PROCESSING` (+
   `DISABLE_NEWLINE_AUTO_RETURN`) on output and `ENABLE_VIRTUAL_TERMINAL_INPUT` (+
   `ENABLE_WINDOW_INPUT`, `ENABLE_MOUSE_INPUT`, `ENABLE_EXTENDED_FLAGS`) on input, and fails
   `open()` with a typed error when output VT is unavailable. There is no legacy console rendering
   path (no `SetConsoleTextAttribute`), permanently — the divergence machine behind a third of
   crossterm's tracker is a rejected alternative, not deferred work.
3. **Hybrid record read.** Input is read with `ReadConsoleInputW`: `KEY_EVENT_RECORD` text feeds
   the platform-neutral VT decoder as a byte stream (UTF-16 units reassembled across surrogate
   halves, then encoded as UTF-8); `WINDOW_BUFFER_SIZE_EVENT` records surface as resize events;
   `MOUSE_EVENT` records are accepted for conhost, which does not translate mouse to SGR VT.
4. **Readiness = cancellable wait, not cancellable read.** The async driver runs one worker that
   waits on `[console input handle, waker event]` and only calls `ReadConsoleInputW` after the
   input handle signals, so the worker never parks inside a read. Teardown signals the waker and
   joins the worker within a bounded deadline. The public cancel-safety contract (state lives on
   the session struct; a dropped future abandons only its own await) is upheld by the channel
   between worker and session owning all in-flight bytes.
5. **Progressive input enhancement is opt-in.** The decoder understands win32-input-mode sequences
   unconditionally, but *enabling* `?9001h` is a policy-gated opt-in; the kitty keyboard protocol
   (already probed by readback) is the preferred enhancement where the host supports it.
6. **Output is UTF-8 via `WriteFile`** with the console output codepage set to 65001 for the
   session's lifetime; the codepage is console-global state, so it is captured at open and restored
   through the mode ledger like every other mode. Writes are batched (small console writes are
   disproportionately expensive on Windows).
7. **Job control does not exist on Windows.** `suspend()` returns the typed `Unsupported` error;
   `run_detached` (editor handoff) is supported; `signals()` surfaces console ctrl events
   (Ctrl+C/Ctrl+Break/close) as interrupt/terminate, never suspend/continue.
8. **ConPTY (`CreatePseudoConsole`) stays out of the device layer.** qwertty is a console *client*;
   ConPTY is the *hosting* API. It enters the tree only as a test/conformance harness that spawns
   qwertty programs headlessly — the same role tmux and betamax play on Unix.

## Consequences

- The platform-neutral decoder, correlator, command encoders, and capability model ship on Windows
  unchanged; the Windows-specific surface is the device, the readiness worker, and lifecycle glue.
- IME/CJK composition under VT input is undocumented upstream and must be verified empirically on
  a real console (the known reason helix retreated to crossterm on Windows). If VT input proves
  insufficient for IME, an `INPUT_RECORD`-assisted decode path can be added behind the existing
  device seam without changing this model — the hybrid loop already reads records.
- In-band resize (mode 2048) is probed like on Unix and simply starts working under any Windows
  host that implements it later; until then resize provenance is the record loop.
- Windows behavior tests run on CI's `windows-latest` job (real console via harness); local
  development from Unix relies on cross-compilation plus that CI loop, with a Windows VM reserved
  for interactive/IME verification.
