# Standards

qwertty is maintained as a Rust library whose public behavior should be easier to understand than
the terminal details beneath it.

## Principles

- Optimize for long-term maintenance, reader locality, and low cognitive burden.
- Prefer boring, explicit, idiomatic Rust APIs.
- Keep visibility narrow.
- Use strong types where protocol, ownership, policy, or lifecycle semantics matter.
- Add abstractions only when they reduce the number of concepts a reader must hold at once.
- Keep changes small, atomic, and reviewable.
- Use issues, pull requests, and ADRs to show planned library evolution.

## Influences

These references influence the local standards, but local project guidance is the source of truth
when there is a conflict:

- <https://epage.github.io/dev/rust-style/>
- <https://microsoft.github.io/rust-guidelines/agents/all.txt>
- <https://rust-lang.github.io/api-guidelines/checklist.html>
