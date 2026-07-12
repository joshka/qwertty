# Release engineering standard

The release-grade CI, supply-chain, and packaging setup for this library. Written to be **liftable to
other libraries** — qwertty is the first adopter. The opinionated policy choices are recorded in
[ADR 0020](../adr/0020-release-engineering-policy.md); this page is the concrete tooling standard and
an adoption checklist.

## Policy (from ADR 0020)

- **No conventional commits.** Version bumps are driven by API diff (`cargo-semver-checks`); the
  changelog is hand-maintained keep-a-changelog. No commit/PR-title linting, no git-cliff.
- **Three separate front pages** (repo README, crate README, `lib.rs` doc) — hand-written per
  audience, not synced; no `cargo-rdme`.
- **No binaries / heavy test data in the published tarball** — `exclude` fixtures, capture sidecars,
  tapes, and tests; ship only what a consumer compiles against.
- **MSRV = stable N-1**, verified by a CI job; `rust-toolchain.toml` pins `channel = "stable"`.

## Supply-chain security

- **Pin every GitHub Action to a full commit SHA** with a `# vX.Y` comment; dependabot bumps them.
- **`permissions: {}`** at each workflow's top level; grant the minimum per job.
- **zizmor** (`zizmorcore/zizmor-action`) scans the workflows themselves; SARIF to the security tab.
- **CodeQL** (Rust extractor, `+security-extended`) on push/PR and a weekly schedule.
- **cargo-deny** (`deny.toml`): advisories, a license allow-list, `bans` (`multiple-versions = warn`,
  `wildcards = deny`), and `sources` (`unknown-registry`/`unknown-git = deny`).

## Dependency hygiene

- **dependabot** (`.github/dependabot.yml`): `cargo` + `github-actions`, weekly, 7-day cooldown,
  grouped, PR limit.
- **cargo-minimal-versions** (`--direct --all-features`) proves the Cargo.toml lower bounds compile.
- **cargo-machete** (fast, stable) for unused dependencies; cargo-udeps optional.
- **cargo-semver-checks** gates PRs against accidental breaking changes.
- **Cargo.lock committed**; MSRV job builds/tests on the stable-N-1 toolchain.

## CI structure and speed

- **`Swatinem/rust-cache`** in every build job.
- **`concurrency: { cancel-in-progress: true }`** to kill superseded runs.
- **Fast jobs first** (fmt, typos, clippy, deny) before the heavy test/cross matrix.
- A single **`required`** job aggregates all checks for branch protection.
- The **local `justfile` mirrors CI** so "green locally == green in CI"; recipes that need
  not-always-installed tools guard themselves or are documented as `cargo install`.

### Project-specific jobs kept for qwertty

These are ahead of the baseline template and must survive any CI reshape: the **fuzz** smoke job,
**verify-emulators** (tmux + betamax, kept out of the required gate), the **default-build gate trio**
(default-feature clippy / doctests / docs — closing the gaps `--all-features` hides), **loom**
concurrency models, and the **windows + wasm cross-compile** jobs.

## Release automation

Status for qwertty: **this machinery is live** — 0.1.0 was published manually, trusted publishing
was then registered, and 0.1.1+ have shipped through it. Releases are the maintainer's act;
sessions never merge release PRs or dispatch the release workflow.

- **release-plz** cuts releases: it opens a version-bump PR (`chore: release vX.Y.Z`), with the
  bump determined by `cargo-semver-checks` (no conventional commits). `changelog_update = false`:
  release-plz never writes `CHANGELOG.md`. Entries accumulate under `[Unreleased]` in the PRs
  that make the changes; the maintainer converts that section into a version heading at release
  time (in the release PR, or in the version-bump commit on the dispatch path).
- **Two release paths**, both gated by the release job's `if` (ordinary pushes never publish):
  1. **Merge the release-plz PR** — the squash commit's `chore: release` subject triggers the
     release job, which publishes, tags, and creates the GitHub Release.
  2. **`workflow_dispatch` the Release-plz workflow** — `release_always = true` lets the release
     job publish whenever main's version is ahead of crates.io, so a hand-made version-bump +
     changelog commit followed by a dispatch also releases (how 0.1.1 and 0.1.2 shipped).
- **Tag format is `qwertty-v<version>`** (release-plz's `<crate>-v` default, kept deliberately).
  CHANGELOG version links and anything matching release tags must use it.
- **Trusted Publishing** (crates.io OIDC, `id-token: write`, no `CARGO_REGISTRY_TOKEN` secret).
  Bootstrap for a **new** library: (1) publish the first version **manually** with a local
  `cargo login` (trusted publishing can only be configured for a crate that already exists);
  (2) register the repo as a crates.io trusted publisher; (3) release-plz then publishes
  subsequent versions with no long-lived token.
- The release job runs in the protected **`release` environment**; the repository setting
  **"Allow GitHub Actions to create and approve pull requests"** (Settings → Actions → General)
  must be enabled or the release-PR job cannot open its PR.

## Governance and packaging

- **`LICENSE-MIT` + `LICENSE-APACHE`** files (the `MIT OR Apache-2.0` expression requires both).
- **SECURITY.md** (private advisories), **CONTRIBUTING.md** (notes the hand-maintained changelog),
  **CODE_OF_CONDUCT.md** (Contributor Covenant), **CODEOWNERS**, **FUNDING.yml**.
- **README badges**: crates.io, docs.rs, CI, license, deps.rs.
- **`[package] exclude`** slims the tarball. Keep `src/`, the `include_str!`-embedded
  `docs/reference/*.md`, `README.md`, `LICENSE-*`, and `examples/`; exclude `fixtures/`,
  `db/captures/`, `tapes/`, `tests/`, `fuzz/`, `scripts/`. Verify with
  `cargo publish --dry-run --all-features` that it still *verifies* (compiles from the packaged form)
  and that the size dropped.
- **`[package.metadata.docs.rs]`**: `all-features = true` + `--cfg docsrs` for feature badges.

## Repo hygiene

`rust-toolchain.toml` (`channel = "stable"`), `clippy.toml`, `bacon.toml` (local dev loop),
`.editorconfig`, `tombi.toml` (TOML formatting), `rustfmt.toml`. `unsafe_code = "forbid"` and
`#![warn(missing_docs)]` in the crate.

## Adoption checklist (for a new library)

1. Add `LICENSE-MIT` + `LICENSE-APACHE`; set `license = "MIT OR Apache-2.0"`.
2. Copy `deny.toml`, `typos.toml`, `dependabot.yml`, `rust-toolchain.toml`, `.editorconfig`,
   `bacon.toml`, `tombi.toml`, `clippy.toml` from an existing library; adjust the typos allow-list.
3. Copy the CI workflows (ci / zizmor / codeql / release-plz), keeping actions SHA-pinned and
   `permissions: {}`; add the project's own jobs (fuzz, cross, etc.).
4. Seed a keep-a-changelog `CHANGELOG.md`; add `SECURITY.md`/`CONTRIBUTING.md`/`CODE_OF_CONDUCT.md`/
   `CODEOWNERS`/`FUNDING.yml`; add README badges.
5. Add `[package] exclude` and verify the tarball with `cargo publish --dry-run`.
6. Set MSRV to stable-N-1 and add the MSRV CI job.
7. Enable "Allow GitHub Actions to create and approve pull requests" (Settings → Actions →
   General) so release-plz can open its release PRs.
8. Publish the first version manually; then register crates.io trusted publishing and let release-plz
   take over (tags follow release-plz's `<crate>-v<version>` format).
