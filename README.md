# qwertty

qwertty is a Rust library for building terminal applications that need explicit terminal ownership,
ordered output, input handling, and policy-aware terminal features.

The library is being developed in small public slices. The first slices establish project quality
standards, then add command encoding, terminal device access, session lifecycle management, input,
queries, and capability policy.

## Status

qwertty has an encode-only command foundation. It can build terminal output bytes, but it does not
open a terminal, manage raw mode, read input, or own a terminal session yet.

## Small Example

```rust
use qwertty::{CommandBuffer, ProtocolPosition, commands};

let mut output = CommandBuffer::new();
output
    .command(commands::screen::clear())
    .command(commands::cursor::move_to(ProtocolPosition::new(3, 5)))
    .text("Ready");

assert_eq!(output.as_bytes(), b"\x1b[2J\x1b[3;5HReady");
```

## Project Shape

- User-facing APIs should be practical before they are broad.
- Examples should show realistic terminal workflows.
- Public APIs should include Rustdoc that explains relevant errors, invariants, safety, policy, or
  protocol behavior.
- Maintainer details live under `docs/` instead of the first reading path.

## Contributing

Use `just check` to run the local gate. See [docs/workflow.md](docs/agent/workflow.md) for the
development workflow and [docs/roadmap.md](docs/roadmap.md) for the planned order of work. The API
docs include the terminal protocol reference in `qwertty::docs`.
