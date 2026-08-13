# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions are
[semver](https://semver.org/) and correspond one-to-one with git tags
(`v0.2.1` == crate version `0.2.1`).

## [Unreleased]

## [0.2.3] - 2026-08-13

### Added

- `describe_with_hint` / `describe_in_with_hint` functions in
  `semvertag-shell` that accept an optional manifest version hint,
  making shell-adapter derivation consistent with `cargo-semvertag`
  (which reads `Cargo.toml`'s `package.version`). A developer bump to
  e.g. `0.2.0` after tag `v0.1.0` now yields `0.2.0-dev.N` from either
  entry point.

### Changed

- `cargo-semvertag derive` now delegates to
  `semvertag_shell::describe_in_with_hint` instead of manually chaining
  `describe_raw` + `derive_with_hint`.
- Shared `git describe` invocation extracted into a private
  `git_describe_raw` helper; no behavioural change.

## [0.2.2] - 2026-08-11

### Added

- `derive` now honors the `package.version` declared in `Cargo.toml` as a
  hint for the next release version, instead of always applying a patch
  bump. The hint is used only when it is a legal successor of the latest
  tag (patch +1, minor +1 when patch is 0, major +1 when minor and patch
  are 0); it is ignored at the tag commit, for prerelease tags, and when
  stale or illegal &mdash; falling back to the plain patch bump. Derived
  versions keep the prerelease form `0.2.2-dev.N+g&lt;hash&gt;`. New
  `--manifest-path` flag on `derive`.
- Virtual workspace roots: `check` and `derive` now resolve
  `[workspace.package].version` when the manifest has `[workspace]` but no
  `[package]`, including the uncommitted-bump flow.
- Git state is now probed from the manifest's repository: `--manifest-path`
  into a foreign repo validates against that repo's tags, and the
  shallow-clone guard fires from nested subdirectories.
- Documented expectation: semvertag assumes the whole repository is
  uniformly versioned (git tags commits, not trees) &mdash; SPEC
  &sect;8.1/&sect;8.3 and both READMEs.

### Changed

- Lockstep versioning: all crates inherit version, edition, and metadata
  from `[workspace.package]` at the workspace root; the whole workspace
  moves to 0.2.2 together from one bump site.

### Fixed

- Workspace-inherited version resolution from inside a member directory
  (the ancestor walk could not climb above the cwd, silently dropping the
  manifest hint).

## [0.2.1] - 2026-08-07

### Changed

- CI publish pipeline hardened: OIDC trusted publishing behind a dedicated
  `publish` environment; stable toolchain pinned in CI steps.
- Version bump to 0.2.1.

### Fixed

- OIDC token exchange in the publish job (crates-io-auth-action).

## [0.2.0] - 2026-08-06

### Changed

- Strict successor rule enforced (`is_valid_successor`); CLI rewritten
  with clap.
- Crate renamed `semvertag-cli` &rarr; `cargo-semvertag`.
- Repository URLs corrected (exfed &rarr; unprofessor); per-crate READMEs
  for the crates.io pages; non-ASCII punctuation scrubbed.
- CI publishes all crates on release tags via trusted publishing.
- Version bump to 0.2.0.

## [0.1.0] - 2026-08-01

### Added

- `semvertag-core`: `derive`, `parse_describe_string`, `is_valid_successor`.
- `semvertag-shell`: build-script integration, dogfooded by this workspace;
  falls back to `0.0.0-dev.N+g&lt;hash&gt;` when no tags exist.
- `cargo-semvertag` CLI (`check`, `derive`, `parse`).
- SPEC, READMEs, dual license (MIT OR Apache-2.0).
- CI: fmt, clippy, test.

[Unreleased]: https://github.com/unprofessor/semvertag/compare/v0.2.3...HEAD
[0.2.3]: https://github.com/unprofessor/semvertag/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/unprofessor/semvertag/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/unprofessor/semvertag/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/unprofessor/semvertag/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/unprofessor/semvertag/compare/cc42dbe...v0.1.0
