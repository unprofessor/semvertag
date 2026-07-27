# Design Plan: git-tag-derived SemVer crate

Working name: `gitver` (placeholder — check crates.io availability before publishing;
`git-semver` is taken by a Go tool with the same scheme, so an unrelated Rust name is safer).

## 1. Problem statement

`git describe --tags` produces strings like `v1.0.0-5-g87af40b`. Treated naively as a
SemVer string, the `-5-g87af40b` suffix parses as a *prerelease* identifier, which makes
a commit 5 past `v1.0.0` sort *before* `v1.0.0` — the opposite of the intended meaning.
This breaks down further once real prerelease tags (`v1.0.0-rc.1`) are mixed in.

Goal: a small, well-tested crate that turns `git describe`-style output into a
SemVer string whose ordering matches intuition:

```
0.9.9 < 1.0.0-rc.1 < 1.0.0-rc.1.dev.3 < 1.0.0-rc.2 < 1.0.0 < 1.0.1-dev.5 < 1.0.1
```

## 2. Goals / non-goals

**Goals**
- Deterministic, monotonic version derivation from git tag + commit-count + hash.
- Pure core logic with zero I/O — testable without a git repo.
- Thin adapters for actually invoking git (shell and/or `git2`).
- Clear, typed errors instead of silent garbage output.
- Reusable across projects (crates.io-published, not vendored).

**Non-goals**
- Not a release-automation tool (no tagging, no changelog generation, no publishing).
- Not a general SemVer *parser* — delegate to `semver` (dtolnay) for parsing/validation.
- No opinion on *how* you pick the next version number when there's no prerelease
  hint (patch-bump only, no minor/major inference from commit messages).

## 3. Prior art

- `git-version`, `git-testament` — embed the raw `git describe` string, no semver
  re-derivation. Good for human-readable `--version`, not for comparable ordering.
- `vergen` (+`vergen-gitcl`/`vergen-git2`) — build.rs env-var emission, same
  raw-string limitation.
- `git-semver` (Go, mdomke) — implements this exact scheme for Go projects;
  used as the reference algorithm here, no Rust equivalent found.
- `semver-setuptools-git-version` (Python) — same idea, PEP 440 flavor.

## 4. Architecture

Workspace with five crates, each independently useful:

```
gitver/
  crates/
    gitver-core/      pure derivation logic, no I/O, no git dependency
    gitver-shell/     shells out to `git describe`, minimal deps
    gitver-git2/      optional libgit2-backed adapter (no git binary needed)
    gitver-macros/    optional proc-macro for compile-time embedding
    gitver-cli/       optional `cargo-gitver` binary (release-time checks, e.g. §8)
  Cargo.toml           workspace root
```

`gitver-core` is the crate to get right; the adapters are thin and low-risk.

### 4.1 `gitver-core`

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

### 4.2 `gitver-shell`

Runs `git describe --tags --long --always --dirty=.dirty`, feeds the output to
`gitver_core::parse_describe_string` + `derive`. Also checks for `.git/shallow`
and returns a distinct `ShallowClone` warning/error rather than silently
returning a wrong version — this is the most common real-world failure mode
(CI checkouts with `fetch-depth: 1`).

### 4.3 `gitver-git2`

Same output, via `libgit2` instead of shelling out — useful for build
environments without a `git` binary on `PATH`.

### 4.4 `gitver-macros` (optional, later milestone)

```rust
gitver::version!() // -> &'static str, resolved at compile time, git-version-style
```

### 4.5 build.rs helper

Documented pattern (not necessarily its own crate) for emitting
`cargo:rustc-env=GIT_VERSION=...` and the correct `cargo:rerun-if-changed`
directives (`.git/HEAD`, the resolved ref, `.git/index` for dirty-state).

## 5. Derivation algorithm (formal)

Given `tag` (parsed as `semver::Version`), `commits_since: u64`, `hash: &str`, `dirty: bool`:

1. If `commits_since == 0` and not `dirty`: return `tag` unchanged.
2. If `commits_since == 0` and `dirty`: return `tag` with build metadata `dirty`
   appended (`1.0.0+dirty`).
3. If `commits_since > 0` and `tag.pre.is_empty()` (plain release tag):
   - bump `patch += 1`
   - `pre = "dev.{commits_since}"`
   - `build = hash` (+ `.dirty` if dirty)
4. If `commits_since > 0` and `!tag.pre.is_empty()` (tag is itself a prerelease):
   - major/minor/patch unchanged
   - `pre = "{tag.pre}.dev.{commits_since}"`
   - `build = hash` (+ `.dirty` if dirty)

This guarantees: `derive(count=0) == tag` exactly, and increasing `commits_since`
for a fixed tag always increases precedence, per SemVer's prerelease-identifier
comparison rules (numeric fields compare numerically, so `dev.9 < dev.10`).

## 6. Edge cases and design decisions

| Case | Decision |
|---|---|
| No tags in repo at all | `DeriveError::NoTagsFound`; caller decides fallback (e.g. `0.0.0-dev.N`) — crate does not silently invent a base version. |
| Tag doesn't parse as SemVer (`release-2021-01`) | `DeriveError::UnparseableTag`. Recommend `git describe --match 'v[0-9]*'` at the adapter level to avoid ever seeing these. |
| Tag has a `v` prefix | Strip before parsing, note the prefix, but don't hardcode — make prefix configurable (default `"v"`, allow `""`). |
| Tag already contains build metadata (rare, malformed) | Reject in `parse_describe_string`; ambiguous to merge two build-metadata segments correctly. |
| Dirty worktree | Always build metadata, never prerelease — dirty/clean isn't an orderable axis, it's provenance. Known consequence: dirty and clean at the same commit compare as *equal* under strict SemVer precedence (build metadata is ignored for ordering) — document this as expected, not a bug. |
| Shallow clone (CI `fetch-depth: 1`) | Detect `.git/shallow`, surface a distinct error/warning in `gitver-shell` rather than a silently wrong commit count. |
| Lightweight vs. annotated tags | Always invoke `git describe` with `--tags` so lightweight tags are found; document this requirement. |
| Multiple tags on the same commit | Git's own `describe` picks one (most recent by default); out of scope for this crate to second-guess. |
| Huge commit counts | Use `u64` for `commits_since`, not `u32`. |
| Detached HEAD | No special handling needed — `git describe` works the same regardless of branch state. |

## 7. Test plan

### 7.1 Unit tests — `derive()` (pure, table-driven)

| tag | commits_since | dirty | expected |
|---|---|---|---|
| `1.0.0` | 0 | false | `1.0.0` |
| `1.0.0` | 0 | true | `1.0.0+dirty` |
| `1.0.0` | 5 | false | `1.0.1-dev.5+g87af40b` |
| `1.0.0` | 5 | true | `1.0.1-dev.5+g87af40b.dirty` |
| `1.0.0-rc.1` | 0 | false | `1.0.0-rc.1` |
| `1.0.0-rc.1` | 3 | false | `1.0.0-rc.1.dev.3+g87af40b` |
| `0.1.0` | 1 | false | `0.2.0-dev.1+g87af40b` *(patch bump only — confirm 0.x is NOT special-cased; document explicitly since Cargo treats 0.x specially for compatibility but this crate is about ordering, not compatibility)* |
| `1.0.0` | 9 vs `1.0.0` at 10 | — | assert `derive(9) < derive(10)` (numeric prerelease comparison, not lexical — this is the case that motivated the whole crate) |
| `1.0.0` | 99999999 | false | parses without overflow |

### 7.2 Unit tests — `parse_describe_string()`

| input | expected |
|---|---|
| `v1.0.0-0-g87af40b` | `Describe { tag: "v1.0.0", commits_since: 0, hash: "87af40b", dirty: false }` |
| `v1.0.0-5-g87af40b` | `commits_since: 5` |
| `v1.0.0-rc.1-3-g87af40b` | `tag: "v1.0.0-rc.1"` — regression test for parsing from the right, since a naive first-hyphen split would misparse this |
| `v1.0.0-5-g87af40b.dirty` (from `--dirty=.dirty`) | `dirty: true` |
| `87af40b` (no tags, `--always` fallback) | `DeriveError::NoTagsFound` or a distinct `NoTagVariant` the caller can match on |
| `` (empty string) | `DeriveError::MalformedDescribeString` |
| `not-even-close-to-valid` | `DeriveError::MalformedDescribeString` |
| `v1.0.0-rc.1-3-g87af40b-4-gdeadbee` (double-describe, shouldn't occur but shouldn't panic) | parsed via rightmost split, tag becomes everything but the last two groups — assert no panic, result may be `UnparseableTag` |

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

Each spins up a throwaway repo and asserts `gitver-shell`'s end-to-end output:

1. Fresh repo, one commit, annotated tag `v1.0.0` on HEAD → `1.0.0`.
2. Same, plus 3 more commits → `1.0.1-dev.3+g<hash>` (hash format matches real
   short-hash length, not a fixed placeholder).
3. Repo with `v1.0.0-rc.1` tag plus 2 commits → `1.0.0-rc.1.dev.2+g<hash>`.
4. Repo with an uncommitted modified tracked file → `dirty: true` reflected in output.
5. Repo with zero tags → `NoTagsFound` surfaced correctly through the shell adapter.
6. Repo with only a **lightweight** tag (not annotated) → still found (confirms
   `--tags` flag is actually being passed).
7. Simulated shallow clone (`git clone --depth 1` from fixture #2) →
   `gitver-shell` returns the shallow-clone warning/error rather than a
   plausible-looking wrong version.
8. Two tags on the same commit → doesn't panic, accepts whichever `git describe`
   picks (documents behavior, doesn't assert a specific winner).

### 7.5 Golden/snapshot tests

Table-driven, doc-tested in `gitver-core`'s docs so the examples in documentation
are themselves the test cases (`cargo test --doc`):

```rust
/// ```
/// # use gitver_core::*;
/// let d = Describe { tag: "v2.3.1".into(), commits_since: 7, hash: "abc1234".into(), dirty: false };
/// assert_eq!(derive(&d).unwrap().to_string(), "2.3.2-dev.7+gabc1234");
/// ```
```

### 7.6 CI-specific regression test

A GitHub Actions job that deliberately checks out with `fetch-depth: 1` and
asserts `gitver-shell` reports the shallow-clone condition rather than a
silently-wrong version — this is the failure mode most likely to bite real
users and the easiest one to regress on without a dedicated test.

## 8. Optional: Cargo.toml successor validation

A separate, optional feature: check that `Cargo.toml`'s `package.version` is a
legal next step from the latest tag — catches "tagged v0.3.0 but forgot to bump
Cargo.toml" or an accidental skip/misfire on the bump. Kept out of the `derive()`
path deliberately: `derive()` runs on every build and answers "what version am
I"; this answers "did someone bump correctly," which only matters at
release/tag time and shouldn't be wired into routine `cargo build`.

### 8.1 Scope

- Pure function lives in `gitver-core`, alongside `derive()` — same reasoning
  (testable without I/O or a real repo).
- Actually invoking it (reading Cargo.toml, resolving the latest tag) is a new
  thin CLI, not folded into `gitver-shell`'s existing build-time responsibilities.
- v1 only handles a **plain-release** latest tag. If the latest tag is itself a
  prerelease (`v1.0.0-rc.1`), return a distinct error rather than guessing —
  deciding what counts as a legal "next" version mid-RC cycle (bump the rc,
  finish the release, or jump ahead) is genuinely ambiguous and deserves its
  own design pass rather than a guess baked in now.
- Explicitly does **not** inspect commit history or diffs to judge *which*
  bump the changes actually warrant (that's breaking-change detection —
  `cargo-semver-checks` / `cargo-smart-release` territory, out of scope here).
  This only validates that the bump, whatever it is, is a legal single step.

### 8.2 API sketch

```rust
pub enum SuccessorError {
    NotGreater { latest: Version, candidate: Version },
    IllegalGap { latest: Version, candidate: Version },
    LatestIsPrerelease { latest: Version }, // out of scope for v1, explicit error rather than guessing
}

/// `latest_release` must have an empty `pre` field (see LatestIsPrerelease above).
pub fn is_valid_successor(
    latest_release: &Version,
    candidate: &Version,
) -> Result<(), SuccessorError>;
```

Legal `candidate` values for a plain-release `latest_release` (major.minor.patch, no prerelease):

- equal to `latest_release` — valid; not yet bumped is fine between releases.
- `patch + 1`, minor/major unchanged.
- `minor + 1`, patch reset to `0`, major unchanged.
- `major + 1`, minor and patch reset to `0`.

Anything else — skipped versions, decrements, arbitrary jumps — is `IllegalGap`.

### 8.3 CLI integration

New crate `gitver-cli`, binary `cargo-gitver`:

```
cargo gitver check
```

- Reads `package.version` from `Cargo.toml` (workspace-member-aware).
- Resolves the latest tag via the existing `gitver-shell` adapter.
- Calls `is_valid_successor`.
- Exits non-zero with a readable diagnostic on failure. Intended for CI or a
  pre-commit/pre-tag hook, not for every `cargo build`.

### 8.4 Test cases

| latest tag | Cargo.toml version | expected |
|---|---|---|
| `1.2.3` | `1.2.3` | Ok — not yet bumped |
| `1.2.3` | `1.2.4` | Ok — patch bump |
| `1.2.3` | `1.3.0` | Ok — minor bump |
| `1.2.3` | `2.0.0` | Ok — major bump |
| `1.2.3` | `1.2.5` | `IllegalGap` — skipped patch |
| `1.2.3` | `1.4.0` | `IllegalGap` — skipped minor |
| `1.2.3` | `1.3.1` | `IllegalGap` — minor bump must reset patch to 0 |
| `1.2.3` | `1.2.2` | `NotGreater` |
| `1.2.3` | `1.2.3-rc.1` | `NotGreater` — lower precedence than `1.2.3` itself |
| `1.2.3-rc.1` (latest tag is itself a prerelease) | anything | `LatestIsPrerelease` — out of scope v1 |
| `0.1.0` | `0.2.0` | Ok — confirm 0.x is *not* specially cased here either, consistent with the same open call in §5/§11 |

Property tests: for any legal bump type applied programmatically to a random
valid `Version`, `is_valid_successor` accepts it; for any candidate strictly
less than `latest`, always `NotGreater`.

## 9. Dependencies

- `semver` (dtolnay) — parsing/validation/comparison, matches Cargo's own interpretation.
- `gitver-shell`: no extra deps beyond `std::process::Command`.
- `gitver-git2`: `git2` crate (optional feature, not default — keeps `gitver-core`
  and `gitver-shell` dependency-light).
- `gitver-cli`: a TOML-parsing crate (`toml` + `cargo_toml`, or hand-rolled
  minimal parsing of just `package.version`) and a small arg parser (`lexopt`
  or `clap`, leaning toward the lighter option given the CLI surface is tiny).
- Dev-deps: `proptest`, `tempfile`, `assert_cmd` (for integration tests).

## 10. Milestones

1. `gitver-core`: `Describe`, `derive`, `parse_describe_string`, full unit +
   property test suite (7.1–7.3). Publishable alone as a useful building block.
2. `gitver-shell`: real-git integration tests (7.4), shallow-clone detection.
3. Docs pass: doc-tested examples (7.5), README with the ordering table from
   §1 as the lead example, worked build.rs snippet.
4. `gitver-git2` adapter (optional, only if the `git` binary dependency
   actually becomes a problem in practice).
5. `gitver-macros` (optional, ergonomics-only — not required for correctness).
6. `is_valid_successor` in `gitver-core` + `gitver-cli`'s `cargo gitver check`
   (§8) — independent of 4–5, can slot in any time after milestone 1.

## 11. Open questions

- Configurable tag prefix (`v`) and tag `--match` glob — expose as `Config`
  struct on the adapters, or push entirely to the caller's `git describe` invocation?
- Should `0.x.y` tags get any special patch-vs-minor treatment given Cargo's
  own left-shifted semver rules for pre-1.0 crates? Current lean: no — this
  crate is about *ordering*, not *compatibility*, so it stays out of that debate
  and always bumps patch. Worth flagging clearly in the README to avoid confusion.
- Is `NoTagsFound` a hard error or should `gitver-core` offer an opt-in
  bootstrap default (e.g. `0.0.0-dev.N`) computed from `git rev-list --count HEAD`?
  Current lean: hard error in core, optional convenience wrapper in `gitver-shell`.
- What should `is_valid_successor` (§8) actually do once the latest tag is a
  prerelease? Deferred past v1, but worth deciding before it comes up in
  practice: legal next steps mid-RC-cycle plausibly include another `rc.N`,
  finishing the plain release, or abandoning the RC line entirely for a new
  major/minor — each is a different validation rule.

