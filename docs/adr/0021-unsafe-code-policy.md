# ADR 0021: Unsafe Code Policy

## Status

Accepted

## Context

The crate has pinned `unsafe_code` at `forbid` in two places since its first commit: `#![forbid]`
in `src/lib.rs` and `unsafe_code = "forbid"` in the `Cargo.toml` lint table. That was the right
default while the whole surface was Unix + pure layers: `rustix` wraps every syscall the crate
needs, so no first-party `unsafe` was ever required.

Windows support (ADR 0022) changes the calculus. Every Win32 console entry point the device layer
needs (`GetConsoleMode`, `SetConsoleMode`, `ReadConsoleInputW`, `WriteFile` on a console handle,
`GetConsoleScreenBufferInfo`, `WaitForMultipleObjects`, …) is an `unsafe extern "system"` call, and
the chosen binding crate (`windows-sys`) deliberately ships no safe wrappers. `forbid` cannot be
locally overridden — a scoped `#[allow(unsafe_code)]` under a crate-level `forbid` is a hard error
(E0453) — so the policy as stated admits no Windows implementation at all.

## Decision

`unsafe_code` is relaxed from `forbid` to `deny`, in both `src/lib.rs` and the `Cargo.toml` lint
table, with exactly one sanctioned opt-in:

- **Only `#[cfg(windows)]` platform-FFI modules may scope an `#[allow(unsafe_code)]`**, and only to
  wrap Win32 API calls that have no safe wrapper in the dependency tree.
- Every `unsafe` block must wrap a **single** FFI call and carry a `// SAFETY:` comment stating the
  contract checked at the call site (handle validity, out-pointer initialization, buffer length).
- The Unix and platform-neutral layers remain effectively forbidden: any new `unsafe` outside a
  `#[cfg(windows)]` FFI module is a review-rejection, not a judgment call.

The security posture this preserves: the parsing/decoding layers — the part of the crate that
handles untrusted input — carry zero `unsafe` on every platform. The Windows FFI module talks to
the local console host, not to attacker-controlled bytes.

## Consequences

- `cargo geiger`-style audits will report `unsafe` usage; the count must stay explainable as "the
  Windows console FFI module only."
- Reviewers gain an obligation: check that new `unsafe` blocks stay inside the sanctioned module
  and keep the one-call-one-block-one-SAFETY-comment shape.
- If a maintained safe console-API wrapper crate emerges that satisfies the dependency policy
  (ADR 0016), migrating to it and restoring `forbid` is the preferred end state. Revisit then.
