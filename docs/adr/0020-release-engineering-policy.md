# ADR 0020: Release Engineering Policy

## Status

Accepted (amended 2026-07-12 — see [Amendment](#amendment-2026-07-12))

## Context

qwertty is preparing its first publication (0.1.0, ADR 0018). Before publishing, the project needs
release-grade CI and supply-chain tooling (caching, `cargo-deny`, zizmor, CodeQL, dependabot,
`cargo-semver-checks`, typos, minimal-versions, MSRV verification, release automation, governance
files). Most of that is standard and adopted wholesale from the maintainer's existing library
conventions. A few choices are opinionated and worth recording so they are not re-litigated, and so
the same policy can be applied to the maintainer's other libraries.

## Decision

### 1. No conventional commits

Commit messages stay free-form. The project does not adopt the Conventional Commits specification,
does not lint commit messages or pull-request titles, and does not use commit-message-driven
changelog generation (git-cliff).

Release automation (release-plz) therefore determines version bumps from the **public API diff**
(`cargo-semver-checks`), not from commit prefixes, and `CHANGELOG.md` is **maintained by hand** in
keep-a-changelog form so release notes are curated for readers rather than assembled from commit
subjects.

Revisit if the maintenance cost of hand-curated changelogs and API-diff versioning outweighs the
cost of imposing a commit convention.

### 2. Three documents, deliberately separate (no cargo-rdme)

qwertty keeps three front pages, each written for its audience and **not** auto-synchronised:

- the **repository README** (GitHub landing page): badges, quickstart, contributor pointers;
- the **crate README** (crates.io landing page): reuses the repository README, or a trimmed variant
  if the repository README carries GitHub-only chrome;
- the **`lib.rs` crate-root doc** (docs.rs landing page): an API-oriented introduction for a reader
  already in the documentation.

The project does not use `cargo-rdme` or any README↔doc synchronisation tool. Forcing these three to
be identical serves none of the three audiences well; keeping them separate lets each be good.

### 3. No binaries or heavy test data in the published tarball

The published crate ships only what a *consumer* compiles against: source, the reference docs that
are `include_str!`-embedded, the README, the licenses, and the examples. Test corpora (`fixtures/`),
raw-byte capture sidecars (`db/captures/`), tapes, and integration tests are kept in the repository
as evidence but **excluded** from the package via `Cargo.toml` `exclude`. Committed binary blobs are
avoided in general; where evidence must be stored, prefer a text-encoded form over raw bytes.

### 4. MSRV policy: stable minus one

The minimum supported Rust version tracks **one release behind current stable** (a rolling "stable
N-1" policy), verified by a dedicated CI job so the `rust-version` claim is tested rather than
asserted. `rust-toolchain.toml` pins `channel = "stable"`; nightly is pulled per-job in CI only where
required (rustfmt, fuzzing, some doc builds).

## Consequences

- Releases are cut with release-plz driving version detection from the API diff; a human writes the
  changelog entry in the release pull request.
- New contributors are not gated on a commit convention, but must update `CHANGELOG.md` by hand for
  user-visible changes (documented in `CONTRIBUTING.md`).
- The published crate is materially smaller than the repository, and downstream builds do not carry
  qwertty's test evidence.
- Bumping the MSRV is a deliberate, telegraphed change (it moves once per stable release at most),
  and the MSRV CI job fails loudly if a dependency or code change raises the real floor.
- These decisions are captured as a reusable standard in
  [`docs/development/release-engineering.md`](../development/release-engineering.md) so they can be
  applied consistently across the maintainer's libraries.

## Amendment (2026-07-12)

The policy above is unchanged; this records how it landed in practice once the 0.1.x releases
shipped, where reality refined the original wording:

- **The bootstrap sequence completed.** 0.1.0 was published manually (local `cargo login`), the
  repository was registered as a crates.io trusted publisher, and 0.1.1+ have published via OIDC
  with no long-lived registry token.
- **Release tags use `qwertty-v<version>`** — release-plz's default `<crate>-v` format, kept
  deliberately rather than shortened to `v*`; CHANGELOG version links match it.
- **`release_always = true`** was added to `release-plz.toml` so a `workflow_dispatch` can publish
  whenever main's version is ahead of crates.io. The release workflow's `if` gate still keeps
  ordinary pushes from publishing; this enables the dispatch path (used for 0.1.1 and 0.1.2)
  alongside the release-PR path.
- **Changelog curation is spread across PRs, not done in the release PR** as the Consequences
  section first assumed: contributors add entries under `[Unreleased]` in the PRs that make the
  changes, and the maintainer converts that section to a version heading at release time.
  release-plz never writes `CHANGELOG.md` (`changelog_update = false`).
- The repository setting "Allow GitHub Actions to create and approve pull requests" is enabled;
  without it the release-PR job cannot open its PRs.
