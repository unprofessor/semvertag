# Design Plan: git-tag-derived SemVer crate

Working name: `semvertag` (placeholder &mdash; check crates.io availability before publishing;
`git-semver` is taken by a Go tool with the same scheme, so an unrelated Rust name is safer).

## 1. Problem statement

`git describe --tags` produces strings like `v1.0.0-5-g87af40b`. Treated naively as a
SemVer string, the `-5-g87af40b` suffix parses as a *prerelease* identifier, which makes
a commit 5 past `v1.0.0` sort *before* `v1.0.0` &mdash; the opposite of the intended meaning.
This breaks down further once real prerelease tags (`v1.0.0-rc.1`) are mixed in.

Goal: a small, well-tested crate that turns `git describe`-style output into a
SemVer string whose ordering matches intuition:

```
0.9.9 < 1.0.0-rc.1 < 1.0.0-rc.1.dev.3 < 1.0.0-rc.2 < 1.0.0 < 1.0.1-dev.5 < 1.0.1
```

## 2. Goals / non-goals

**Goals**

- Deterministic, monotonic version derivation from git tag + commit-count + hash.
- Pure core logic with zero I/O &mdash; testable without a git repo.
- Thin adapters for actually invoking git (shell and/or `git2`).
- Clear, typed errors instead of silent garbage output.
- Reusable across projects (crates.io-published, not vendored).

**Non-goals**

- Not a release-automation tool (no tagging, no changelog generation, no publishing).
- Not a general SemVer *parser* &mdash; delegate to `semver` (dtolnay) for parsing/validation.
- No opinion on *how* you pick the next version number when there's no prerelease
  hint (patch-bump only, no minor/major inference from commit messages).

## 3. Prior art

- `git-version`, `git-testament` &mdash; embed the raw `git describe` string, no semver
  re-derivation. Good for human-readable `--version`, not for comparable ordering.
- `vergen` (+`vergen-gitcl`/`vergen-git2`) &mdash; build.rs env-var emission, same
  raw-string limitation.
- `git-semver` (Go, mdomke) &mdash; implements this exact scheme for Go projects;
  used as the reference algorithm here, no Rust equivalent found.
- `semver-setuptools-git-version` (Python) &mdash; same idea, PEP 440 flavor.

## 4. Architecture

Workspace with five crates, each independently useful:

```
semvertag/
  crates/
    semvertag-core/      pure derivation logic, no I/O, no git dependency
    semvertag-shell/     shells out to `git describe`, minimal deps
    semvertag-git2/      optional libgit2-backed adapter (no git binary needed)
    semvertag-macros/    optional proc-macro for compile-time embedding
    cargo-semvertag/     optional `cargo-semvertag` binary (release-time checks, e.g. sec. 8)
  Cargo.toml           workspace root
```

`semvertag-core` is the crate to get right; the adapters are thin and low-risk.

### 4.1 `semvertag-core`

```rust
pub struct Describe {
    pub tag: String,        // e.g. "v1.0.0" or "v1.0.0-rc.1"
    pub commits_since: u64, // 0 if HEAD is exactly the tag
    pub hash: String,       // short hash, no "g" prefix
    pub dirty: bool,
}

pub enum DeriveError {
    UnparseableTag { tag: String, source: semver::Error },
    MalformedDescribeString { input: String },
    NoTagsFound,
}

pub fn derive(describe: &Describe) -> Result<semver::Version, DeriveError>;

/// Parses a raw `git describe --tags --long [--dirty=.dirty]` string into `Describe`.
/// Parses from the right (last two hyphen-groups are count and hash) since tags
/// themselves may contain hyphens (e.g. "v1.0.0-rc.1").
pub fn parse_describe_string(raw: &str) -> Result<Describe, DeriveError>;
```

### 4.2 `semvertag-shell`

Runs `git describe --tags --long --always --dirty=.dirty`, feeds the output to
`semvertag_core::parse_describe_string` + `derive`. Also checks for `.git/shallow`
and returns a distinct `ShallowClone` warning/error rather than silently
returning a wrong version &mdash; this is the most common real-world failure mode
(CI checkouts with `fetch-depth: 1`). The `.git/shallow` check is best-effort:
it covers the common worktree-root case but may miss `gitdir:` pointer files,
submodules with their own shallow state, or other layouts; a missed detection
surfaces as a plausible-but-wrong version rather than a crash, which is
acceptable until a real-world failure mode forces a more complete probe.

### 4.3 `semvertag-git2`

Same output, via `libgit2` instead of shelling out &mdash; useful for build
environments without a `git` binary on `PATH`.

### 4.4 `semvertag-macros` (optional, later milestone)

```rust
semvertag::version!() // -> &'static str, resolved at compile time, git-version-style
```

### 4.5 build.rs helper

Documented pattern (not necessarily its own crate) for emitting
`cargo:rustc-env=GIT_VERSION=...` and the correct `cargo:rerun-if-changed`
directives (`.git/HEAD`, the resolved ref, `.git/index` for dirty-state). This
directive set is best-effort: it misses packed refs (`.git/packed-refs`), so a
tag move that gets the loose ref packed won't necessarily retrigger the build.
Acceptable until it bites in practice.

## 5. Derivation algorithm (formal)

Given `tag` (parsed as `semver::Version`), `commits_since: u64`, `hash: &str`, `dirty: bool`,
and `hint: Option<Version>` (the manifest's declared next version, e.g. `Cargo.toml`'s
`package.version`; `None` for plain `derive()`):

1. If `commits_since == 0` and not `dirty`: return `tag` unchanged.
2. If `commits_since == 0` and `dirty`: return `tag` with build metadata `dirty`
   appended (`1.0.0+dirty`).
3. If `commits_since > 0` and `hint` is a legal single-step successor of `tag`
   (per &sect;8: patch + 1, or minor + 1 with patch = 0, or major + 1 with
   minor = patch = 0):
   - use `hint`'s major/minor/patch as the base instead of the blind patch bump
   - `pre = "dev.{commits_since}"`
   - `build = "g" + hash` (+ `.dirty` if dirty)
4. If `commits_since > 0` and `tag.pre.is_empty()` (plain release tag, no legal hint):
   - bump `patch += 1`
   - `pre = "dev.{commits_since}"`
   - `build = "g" + hash` (+ `.dirty` if dirty)
5. If `commits_since > 0` and `!tag.pre.is_empty()` (tag is itself a prerelease):
   - major/minor/patch unchanged
   - `pre = "{tag.pre}.dev.{commits_since}"`
   - `build = "g" + hash` (+ `.dirty` if dirty)

The hint (step 3) is how a developer-performed bump shows up in derived
versions: the manifest is the declaration of the *next* release version, so a
`Cargo.toml` carrying `0.2.0` after tag `v0.1.0` yields `0.2.0-dev.N` instead
of `0.1.1-dev.N`. The hint is ignored -- and steps 4/5 apply -- when the
manifest is missing (non-Cargo repos), not bumped yet, carries an illegal
version, or when HEAD *is* the tag (the tagged version wins even if the
working tree already holds the next version), or when the tag is itself a
prerelease (mid-RC, successor rules don't apply per &sect;8.1). An ignored hint
is never an error in `derive()`: judging manifests is `is_valid_successor` /
`check`'s job, not derivation's.

The `g` prefix on the build metadata mirrors `git describe`'s own `g87af40b`
convention; `Describe.hash` stores the bare short hash without the prefix, and
`derive()` adds it. This guarantees: `derive(count=0) == tag` exactly, and
for a fixed tag always increases precedence, per SemVer's prerelease-identifier
comparison rules (numeric fields compare numerically, so `dev.9 < dev.10`).

## 6. Edge cases and design decisions

| Case | Decision |
| --- | --- |
| No tags in repo at all | `DeriveError::NoTagsFound`; caller decides fallback (e.g. `0.0.0-dev.N`) &mdash; crate does not silently invent a base version. |
| Tag doesn't parse as SemVer (`release-2021-01`) | `DeriveError::UnparseableTag`. Recommend `git describe --match 'v[0-9]*'` at the adapter level to avoid ever seeing these. |
| Tag has a `v` prefix | Strip before parsing, note the prefix, but don't hardcode &mdash; make prefix configurable (default `"v"`, allow `""`). |
| Tag already contains build metadata (rare, malformed) | Reject in `parse_describe_string`; ambiguous to merge two build-metadata segments correctly. |
| Dirty worktree | Always build metadata, never prerelease &mdash; dirty/clean isn't an orderable axis, it's provenance. Known consequence: dirty and clean at the same commit compare as *equal* under strict SemVer precedence (build metadata is ignored for ordering) &mdash; document this as expected, not a bug. |
| Shallow clone (CI `fetch-depth: 1`) | Detect `.git/shallow`, surface a distinct error/warning in `semvertag-shell` rather than a silently wrong commit count. |
| Lightweight vs. annotated tags | Always invoke `git describe` with `--tags` so lightweight tags are found; document this requirement. |
| Multiple tags on the same commit | Git's own `describe` picks one (most recent by default); out of scope for this crate to second-guess. |
| Huge commit counts | Use `u64` for `commits_since`, not `u32`. |
| Detached HEAD | No special handling needed &mdash; `git describe` works the same regardless of branch state. |

## 7. Test plan

### 7.1 Unit tests &mdash; `derive()` (pure, table-driven)

| tag | commits_since | dirty | hint | expected |
| --- | --- | --- | --- | --- |
| `1.0.0` | 0 | false | none | `1.0.0` |
| `1.0.0` | 0 | true | none | `1.0.0+dirty` |
| `1.0.0` | 5 | false | none | `1.0.1-dev.5+g87af40b` |
| `1.0.0` | 5 | true | none | `1.0.1-dev.5+g87af40b.dirty` |
| `1.0.0-rc.1` | 0 | false | none | `1.0.0-rc.1` |
| `1.0.0-rc.1` | 3 | false | none | `1.0.0-rc.1.dev.3+g87af40b` |
| `0.1.0` | 1 | false | none | `0.1.1-dev.1+g87af40b` *(patch bump only &mdash; 0.x is NOT special-cased when no hint is given; document explicitly since Cargo treats 0.x specially for compatibility but this crate is about ordering, not compatibility)* |
| `0.1.0` | 3 | false | `0.2.0` | `0.2.0-dev.3+g87af40b` *(the manifest declares a minor bump &mdash; the legal hint replaces the blind patch bump; 0.x is not special-cased here either, the hint decides the bump)* |
| `1.2.3` | 1 | false | `1.2.4` | `1.2.4-dev.1+g87af40b` *(legal patch hint, coincides with the blind bump)* |
| `1.2.3` | 5 | true | `2.0.0` | `2.0.0-dev.5+g87af40b.dirty` *(legal major hint, dirty propagates)* |
| `1.2.3` | 7 | any | `1.2.3` (not bumped) | `1.2.4-dev.7+g87af40b` *(stale hint falls back to the blind patch bump)* |
| `1.2.3` | 7 | any | `1.2.5` (illegal gap) | `1.2.4-dev.7+g87af40b` *(illegal hint falls back &mdash; judging it is `check`'s job)* |
| `1.0.0-rc.1` | 3 | false | `1.0.0` | `1.0.0-rc.1.dev.3+g87af40b` *(hint ignored mid-RC &mdash; chaining rule wins)* |
| `1.2.3` | 0 | true | `1.3.0` | `1.2.3+dirty` *(the tag wins at the tag commit, even with a bumped working tree)* |
| `1.0.0` | 9 vs `1.0.0` at 10 | &mdash; | none | assert `derive(9) < derive(10)` (numeric prerelease comparison, not lexical &mdash; this is the case that motivated the whole crate) |
| `1.0.0` | 99999999 | false | none | parses without overflow |

### 7.2 Unit tests &mdash; `parse_describe_string()`

| input | expected |
| --- | --- |
| `v1.0.0-0-g87af40b` | `Describe { tag: "v1.0.0", commits_since: 0, hash: "87af40b", dirty: false }` |
| `v1.0.0-5-g87af40b` | `commits_since: 5` |
| `v1.0.0-rc.1-3-g87af40b` | `tag: "v1.0.0-rc.1"` &mdash; regression test for parsing from the right, since a naive first-hyphen split would misparse this |
| `v1.0.0-5-g87af40b.dirty` (from `--dirty=.dirty`) | `dirty: true` |
| `87af40b` (no tags, `--always` fallback) | `DeriveError::NoTagsFound` or a distinct `NoTagVariant` the caller can match on |
| `` (empty string) | `DeriveError::MalformedDescribeString` |
| `not-even-close-to-valid` | `DeriveError::MalformedDescribeString` |
| `v1.0.0-rc.1-3-g87af40b-4-gdeadbee` (double-describe, shouldn't occur but shouldn't panic) | asserts no panic; no specific result is pinned (the input is malformed and out of contract) |

### 7.3 Property-based tests (`proptest`)

- **Monotonicity**: for a fixed valid plain-release tag `T` and `n1 < n2` (both `u64`),
  `derive(T, n1) < derive(T, n2)`.
- **Monotonicity across the release boundary**: `derive(T, n) < T_next` where `T_next`
  is `T` with patch bumped and no prerelease, for any `n > 0`.
- **Identity at zero**: `derive(T, 0, dirty=false) == T` for all valid `T`.
- **No panics**: arbitrary `&str` fed to `parse_describe_string` never panics,
  always returns `Result`.
- **Round-trip**: `derive(...).to_string()` always re-parses via `semver::Version::parse`.

### 7.4 Integration tests (real git repos via `tempfile` + `git2` or shelled commands)

Each spins up a throwaway repo and asserts `semvertag-shell`'s end-to-end output:

1. Fresh repo, one commit, annotated tag `v1.0.0` on HEAD &rarr; `1.0.0`.
2. Same, plus 3 more commits &rarr; `1.0.1-dev.3+g<hash>` (hash format matches real
   short-hash length, not a fixed placeholder).
3. Repo with `v1.0.0-rc.1` tag plus 2 commits &rarr; `1.0.0-rc.1.dev.2+g<hash>`.
4. Repo with an uncommitted modified tracked file &rarr; `dirty: true` reflected in output.
5. Repo with zero tags &rarr; `NoTagsFound` surfaced correctly through the shell adapter.
6. Repo with only a **lightweight** tag (not annotated) &rarr; still found (confirms
   `--tags` flag is actually being passed).
7. Simulated shallow clone (`git clone --depth 1` from fixture #2) &rarr;
   `semvertag-shell` returns the shallow-clone warning/error rather than a
   plausible-looking wrong version.
8. Two tags on the same commit &rarr; doesn't panic, accepts whichever `git describe`
   picks (documents behavior, doesn't assert a specific winner).

### 7.5 Golden/snapshot tests

Table-driven, doc-tested in `semvertag-core`'s docs so the examples in documentation
are themselves the test cases (`cargo test --doc`):

```rust
/// ```
/// # use semvertag_core::*;
/// let d = Describe { tag: "v2.3.1".into(), commits_since: 7, hash: "abc1234".into(), dirty: false };
/// assert_eq!(derive(&d).unwrap().to_string(), "2.3.2-dev.7+gabc1234");
/// ```
```

### 7.6 CI-specific regression test

A GitHub Actions job that deliberately checks out with `fetch-depth: 1` and
asserts `semvertag-shell` reports the shallow-clone condition rather than a
silently-wrong version &mdash; this is the failure mode most likely to bite real
users and the easiest one to regress on without a dedicated test.

## 8. Optional: Cargo.toml successor validation

A separate, optional feature: check that `Cargo.toml`'s `package.version` is a
legal next step from the latest tag &mdash; catches "tagged v0.3.0 but forgot to bump
Cargo.toml", an accidental skip/misfire on the bump, and a manifest left
sitting on the released version after the release. Validation is deliberately
decoupled from `derive()`: `derive()` runs on every build and answers "what
version am I". It may *consume* the manifest's version as a hint (&sect;5 step 3)
&mdash; the developer's bump is the best answer to "what is the next release"
&mdash; but it never judges it: an illegal manifest version silently falls back to
the tag-based bump. Judging "did someone bump correctly" only matters at
release/tag time and shouldn't fail a routine `cargo build`.

### 8.1 Scope

- Pure function lives in `semvertag-core`, alongside `derive()` &mdash; same reasoning
  (testable without I/O or a real repo).
- Actually invoking it (reading Cargo.toml, resolving the latest tag) is a new
  thin CLI, not folded into `semvertag-shell`'s existing build-time responsibilities.
- v1 only handles a **plain-release** latest tag. If the latest tag is itself a
  prerelease (`v1.0.0-rc.1`), return a distinct error rather than guessing &mdash;
  deciding what counts as a legal "next" version mid-RC cycle (bump the rc,
  finish the release, or jump ahead) is genuinely ambiguous and deserves its
  own design pass rather than a guess baked in now.
- Deliberately strict about *when* the bump lands: equality with the latest tag
  is valid only at the tagged release commit itself &mdash; the first commit after a
  release must already carry the next version (the invariants this buys are
  spelled out in &sect;8.2). This is a workflow opinion in a way &sect;2's non-goals are
  not; it is the price of guaranteeing that no untagged commit ever reports a
  released version.
- One relaxation of that strictness, at the CLI layer only (`cargo-semvertag`,
  not `is_valid_successor`): at the tagged release commit, an *uncommitted*
  manifest-version change is validated as if it were already the first commit
  of the next cycle &mdash; the check compares the working-tree manifest against
  `HEAD:`'s and, when the version differs, validates with `commits_since = 1`.
  Bumping Cargo.toml and running `cargo semvertag check` before committing is
  the natural release workflow; strict at-the-tag equality would punish exactly
  that order of operations. A *clean* tree at the tag is still judged by
  equality alone, as is a dirty tree whose manifest version is unchanged from
  HEAD's.
- Explicitly does **not** inspect commit history or diffs to judge *which*
  bump the changes actually warrant (that's breaking-change detection &mdash;
  `cargo-semver-checks` / `cargo-smart-release` territory, out of scope here).
  This only validates that the bump, whatever it is, is a legal single step.
- Expects the whole repository to be **uniformly versioned**, because git
  tags *commits*, not trees: a tag names one commit for the entire repo, so
  `check` compares that one latest tag against the one version the
  repository declares. That version is the member's `package.version` (with
  `version.workspace = true` resolved through the root), or &mdash; at a
  virtual workspace root (`[workspace]` without `[package]`) &mdash; the
  root's `[workspace.package].version` directly. Workspaces where individual
  members pin versions that diverge from the shared one cannot be validated
  by a single tag and are out of scope; the lockstep layout
  (`[workspace.package]`, all members inheriting) is the supported model.

### 8.2 API sketch

```rust
pub enum SuccessorError {
    LessThanLatest { latest: Version, candidate: Version },
    NotBumped { latest: Version, candidate: Version, commits_since: u64 },
    TagManifestMismatch { latest: Version, candidate: Version },
    IllegalGap { latest: Version, candidate: Version },
    LatestIsPrerelease { latest: Version }, // out of scope for v1, explicit error rather than guessing
}

/// `latest_release` must have an empty `pre` field (see LatestIsPrerelease above).
/// `commits_since` is 0 iff HEAD is the tagged release commit itself.
pub fn is_valid_successor(
    latest_release: &Version,
    candidate: &Version,
    commits_since: u64,
) -> Result<(), SuccessorError>;
```

Legal `candidate` values for a plain-release `latest_release` (major.minor.patch, no prerelease):

- at the tagged release commit (`commits_since == 0`): equal to `latest_release` &mdash; the manifest must match the tag exactly there, or the published artifact would diverge from the tag.
- on any other commit: `patch + 1` (minor/major unchanged), `minor + 1` (patch reset to `0`, major unchanged), or `major + 1` (minor and patch reset to `0`).

This is deliberately stricter than "equal is fine between releases": it
enforces bump-on-first-commit discipline, so the first commit after a release
tag must carry the next version. In exchange it buys two invariants:

- **No untagged commit ever reports a released version.** An untagged commit's
  manifest is always a version that has not been published yet, so a
  git-dependency pin can never collide with (or be mistaken for) a published
  release.
- **The manifest stays consistent with `derive()`.** `manifest >= derive(HEAD)`
  everywhere, with equality exactly at the tagged commit. The lenient rule
  allowed a commit to claim `1.2.3` in `Cargo.toml` while `derive()` reported
  `1.2.4-dev.1` for the same commit &mdash; two answers to "what version is this".
  The hint (&sect;5 step 3) strengthens this: on a legal bump, `derive()`
  *converges* to the manifest (`derive(HEAD) = 1.3.0-dev.N < manifest = 1.3.0`)
  instead of answering with a blind patch bump the manifest will never ship.

The result is decided by precedence, not by membership in the list above:

1. If `candidate < latest_release` (by `semver` ordering) &mdash; `LessThanLatest`.
   This naturally captures prereleases of `latest_release` itself (e.g.
   `1.2.3-rc.1` < `1.2.3`), which are lower precedence rather than illegal gaps,
   and it also catches "tagged v1.2.3 but the manifest still says 1.2.2" at the
   tag commit.
2. Else if `commits_since == 0` and `candidate != latest_release` &mdash;
   `TagManifestMismatch`. At the release commit only equality is meaningful,
   so this is judged before bump legality.
3. Else if `candidate == latest_release` &mdash; `NotBumped` (off the tag, the
   manifest must already be bumped).
4. Else if `candidate` is not one of the legal bumps listed above &mdash; `IllegalGap`
   (skipped versions, arbitrary jumps, decrements that aren't strictly lower, etc.).
5. Else &mdash; `Ok`.

Ordering the checks this way means a `candidate` that is a prerelease of the
same release falls out as `LessThanLatest` without an ad-hoc exception, the
tag-commit state is judged by equality alone, and the legal-bump list only
governs the `> latest_release`, off-tag region.

### 8.3 CLI integration

New crate `cargo-semvertag`, binary `cargo-semvertag`, argument parsing via
`clap`:

- `cargo semvertag` (default) / `cargo semvertag derive` &mdash; print the version
  derived from `git describe` (`derive_with_hint()`, &sect;5). Honors
  `package.version` (read with the same workspace-aware logic as `check`) as
  the hinted next release when it is a legal successor of the tag; a missing,
  stale, or illegal manifest version silently falls back to the tag-based
  patch bump &mdash; only a *present but unreadable* manifest gets a warning,
  never an error, so the command stays safe for build scripts and non-Cargo
  repos. Optional `--manifest-path` for workspace members. Handy in build
  scripts.
- `cargo semvertag check` &mdash; the validator:
  - Reads `package.version` from `Cargo.toml` (workspace-member-aware,
    `version.workspace = true` resolved through the workspace root; at a
    virtual workspace root the version checked is
    `[workspace.package].version` &mdash; the repo is expected to be
    uniformly versioned, see &sect;8.1).
  - Resolves the latest tag via the existing `semvertag-shell` adapter and
    passes the tag's commit distance to `is_valid_successor` (`0` when HEAD is
    the tagged release commit itself) &mdash; with the uncommitted-bump relaxation
    of &sect;8.1 applied on top. Git state (describe, the shallow-clone guard)
    is probed at the *manifest's* repository root (`git rev-parse
    --show-toplevel` from the manifest's directory), so a `--manifest-path`
    into a different repository validates against that repository's tags,
    and the shallow guard fires consistently no matter how deep below the
    repo root the command runs.
  - Exits non-zero with a readable diagnostic on failure; the
    `TagManifestMismatch` diagnostic names the three legal next-release
    versions (patch/minor/major) so the fix is copy-pasteable. Intended for CI
    or a pre-commit/pre-tag hook, not for every `cargo build`.
- `cargo-semvertag --version` / `-V` &mdash; print the tool's own version (standard
  CLI semantics; there is no `version` subcommand).

### 8.4 Test cases

| latest tag | Cargo.toml version | HEAD | expected |
| --- | --- | --- | --- |
| `1.2.3` | `1.2.3` | on tag | Ok &mdash; the tagged release commit |
| `1.2.3` | `1.2.3` | 1 commit past | `NotBumped` &mdash; must bump on the first commit after a release |
| `1.2.3` | `1.2.4` | on tag | `TagManifestMismatch` &mdash; manifest must equal the tag at the release commit |
| `1.2.3` | `1.2.4` | 1 commit past | Ok &mdash; patch bump |
| `1.2.3` | `1.3.0` | 1 commit past | Ok &mdash; minor bump |
| `1.2.3` | `2.0.0` | 1 commit past | Ok &mdash; major bump |
| `1.2.3` | `1.2.5` | 1 commit past | `IllegalGap` &mdash; skipped patch |
| `1.2.3` | `1.4.0` | 1 commit past | `IllegalGap` &mdash; skipped minor |
| `1.2.3` | `1.3.1` | 1 commit past | `IllegalGap` &mdash; minor bump must reset patch to 0 |
| `1.2.3` | `1.2.2` | any | `LessThanLatest` &mdash; including "tagged v1.2.3 but forgot to bump" at the tag commit |
| `1.2.3` | `1.2.3-rc.1` | any | `LessThanLatest` &mdash; lower precedence than `1.2.3` itself |
| `1.2.3-rc.1` (latest tag is itself a prerelease) | anything | any | `LatestIsPrerelease` &mdash; out of scope v1 |
| `0.1.0` | `0.2.0` | 1 commit past | Ok &mdash; confirm 0.x is *not* specially cased here either, consistent with the same open call in &sect;5/&sect;11 |

Property tests: for any legal bump type applied programmatically to a random
valid `Version`, `is_valid_successor` accepts it at an untagged commit and
accepts equality only on the tag; for any candidate strictly less than
`latest`, always `LessThanLatest`.

## 9. Dependencies

- `semver` (dtolnay) &mdash; parsing/validation/comparison, matches Cargo's own interpretation.
- `semvertag-shell`: no extra deps beyond `std::process::Command`.
- `semvertag-git2`: `git2` crate (optional feature, not default &mdash; keeps `semvertag-core`
  and `semvertag-shell` dependency-light).
- `cargo-semvertag`: a TOML-parsing crate (`toml` + `cargo_toml`, or hand-rolled
  minimal parsing of just `package.version`) and `clap` (derive feature) for
  argument parsing.
- Dev-deps: `proptest`, `tempfile`, `assert_cmd` (for integration tests).

## 10. Milestones

1. `semvertag-core`: `Describe`, `derive`, `parse_describe_string`, full unit +
   property test suite (7.1&ndash;7.3). Publishable alone as a useful building block.
2. `semvertag-shell`: real-git integration tests (7.4), shallow-clone detection.
3. Docs pass: doc-tested examples (7.5), README with the ordering table from
   &sect;1 as the lead example, worked build.rs snippet.
4. `semvertag-git2` adapter (optional, only if the `git` binary dependency
   actually becomes a problem in practice).
5. `semvertag-macros` (optional, ergonomics-only &mdash; not required for correctness).
6. `is_valid_successor` in `semvertag-core` + `cargo-semvertag`'s `cargo semvertag check`
   (&sect;8) &mdash; independent of 4&ndash;5, can slot in any time after milestone 1.

## 11. Open questions

- Configurable tag prefix (`v`) and tag `--match` glob &mdash; expose as `Config`
  struct on the adapters, or push entirely to the caller's `git describe` invocation?
- Should `0.x.y` tags get any special patch-vs-minor treatment given Cargo's
  own left-shifted semver rules for pre-1.0 crates? Current lean: no &mdash; this
  crate is about *ordering*, not *compatibility*, so it stays out of that debate
  and always bumps patch. Worth flagging clearly in the README to avoid confusion.
- Is `NoTagsFound` a hard error or should `semvertag-core` offer an opt-in
  bootstrap default (e.g. `0.0.0-dev.N`) computed from `git rev-list --count HEAD`?
  Current lean: hard error in core, optional convenience wrapper in `semvertag-shell`.
- What should `is_valid_successor` (&sect;8) actually do once the latest tag is a
  prerelease? Deferred past v1, but worth deciding before it comes up in
  practice: legal next steps mid-RC-cycle plausibly include another `rc.N`,
  finishing the plain release, or abandoning the RC line entirely for a new
  major/minor &mdash; each is a different validation rule.
