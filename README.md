# semvertag

Turn `git describe`-style strings into SemVer versions whose ordering matches intuition.

`git describe --tags` produces strings like `v1.0.0-5-g87af40b`. Treated naively as a SemVer string, the `-5-g87af40b` suffix parses as a *prerelease* identifier, which makes a commit 5 past `v1.0.0` sort **before** `v1.0.0` — the opposite of what you want. This crate rewrites such strings so that increasing commit count always increases precedence.

## The ordering

This is the whole point, as a test case:

```text
0.9.9 < 1.0.0-rc.1 < 1.0.0-rc.1.dev.3 < 1.0.0-rc.2 < 1.0.0 < 1.0.1-dev.5 < 1.0.1
```

Reading left to right:

| `git describe --long` output   | derived version             | why                              |
|--------------------------------|------------------------------|----------------------------------|
| `0.9.0-0-gabc1234`             | `0.9.0`                      | exact tag (`-0-` means 0 commits past it) |
| `v1.0.0-rc.1-0-gabc1234`       | `1.0.0-rc.1`                 | prerelease tag at HEAD           |
| `v1.0.0-rc.1-3-g87af40b`       | `1.0.0-rc.1.dev.3+g87af40b`  | 3 commits past the rc, appended as `.dev.N` |
| `v1.0.0-rc.2-0-gabc1234`       | `1.0.0-rc.2`                 | next rc, higher precedence       |
| `v1.0.0-0-gabc1234`            | `1.0.0`                      | plain release beats any rc       |
| `v1.0.0-5-g87af40b`            | `1.0.1-dev.5+g87af40b`       | 5 past release → patch bump + dev prerelease |
| `v1.0.1-0-gabc1234`            | `1.0.1`                      | the release those dev builds led to |

The key rules (formally in [SPEC.md §5](SPEC.md#5-derivation-algorithm-formal)):

1. **Tag at HEAD, clean** → the tag unchanged.
2. **Tag at HEAD, dirty** → tag with `+dirty` build metadata.
3. **Commits past a plain release** → bump patch, prerelease `dev.{N}`, build `g{hash}`.
4. **Commits past a prerelease tag** → keep the version, append `.dev.{N}` to the existing prerelease, build `g{hash}`.

Dirty state is always build metadata (`+dirty`), never a prerelease — dirty/clean isn't an orderable axis, it's provenance. A dirty build and a clean build at the same commit compare as **equal** under strict SemVer precedence (build metadata is ignored for ordering). This is expected, not a bug.

## Crates

| crate              | what it does                                          | deps          |
|--------------------|-------------------------------------------------------|---------------|
| `semvertag-core`   | Pure derivation logic. No I/O.                        | `semver`      |
| `semvertag-shell`  | Shells out to `git describe`, feeds core.            | core + `semver` |
| `semvertag-cli`    | `cargo semvertag check` — validate `Cargo.toml` version against the latest tag. | core + shell + `toml` + `lexopt` |

`semvertag-core` is publishable and useful on its own — if you already invoke `git describe` yourself (or use a library), feed it a `Describe` and call `derive()`.

## Quick start

### Embed your version at build time

This is the canonical `build.rs` pattern (also documented in [SPEC §4.5](SPEC.md#45-buildrs-helper)). Add `semvertag-core` as a **build-dependency** (not a regular dependency — you don't want it in your runtime dep tree just for version embedding), invoke `git describe` yourself, and feed the output to `derive()`:

```toml
# Cargo.toml
[build-dependencies]
semvertag-core = "0.1"
```

```rust,no_run
// build.rs
use std::process::Command;

fn main() {
    // Best-effort rerun triggers. Misses packed refs; see SPEC §4.5.
    for p in [".git/HEAD", ".git/index", ".git/refs/tags"] {
        println!("cargo:rerun-if-changed={p}");
    }

    let version = derive_version().unwrap_or_else(|| "0.0.0-unknown".to_string());
    println!("cargo:rustc-env=SEMVERTAG_VERSION={version}");
}

fn derive_version() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--long", "--always", "--dirty=.dirty"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let describe = semvertag_core::parse_describe_string(raw.trim()).ok()?;
    let version = semvertag_core::derive(&describe).ok()?;
    Some(version.to_string())
}
```

```rust,no_run
// src/lib.rs
pub const VERSION: &str = env!("SEMVERTAG_VERSION");
```

> **Why not depend on `semvertag-shell` in build.rs?** You can — it handles the shallow-clone check, the no-tags fallback, and the `git` invocation for you. But `semvertag-shell` depends on `semvertag-core`, and if your crate *is* `semvertag-core` (or depends on it at runtime), Cargo rejects the cyclic build-dependency. The inlined `git describe` call above is ~15 lines and avoids the cycle entirely. For external consumers, `semvertag-shell` in `build-dependencies` is the ergonomic choice.

### Let `semvertag-shell` do the git work

If you don't have the cyclic-dependency problem, `semvertag-shell` handles everything:

```toml
# Cargo.toml
[build-dependencies]
semvertag-shell = "0.1"
```

```rust,no_run
// build.rs
fn main() {
    let version = semvertag_shell::describe()
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "0.0.0-unknown".to_string());
    println!("cargo:rustc-env=SEMVERTAG_VERSION={version}");
}
```

`describe()` runs `git describe --tags --long --always --dirty=.dirty` in the current directory, detects shallow clones (the most common CI failure mode), and returns a parsed `semver::Version`. Use `describe_in(repo)` or `describe_raw(repo)` for more control.

### Validate your `Cargo.toml` version before tagging

`cargo-semvertag` checks that `package.version` in `Cargo.toml` is a legal single-step bump from the latest git tag — catches "tagged v0.3.0 but forgot to bump Cargo.toml" or an accidental skip:

```sh
$ cargo install cargo-semvertag
$ cargo semvertag check
ok: Cargo.toml version 1.2.4 is a legal successor to tag 1.2.3
```

Exit codes: `0` ok, `1` check failed (regression or illegal gap), `2` operational error (no git, no tags, unreadable `Cargo.toml`). Wire it into a pre-tag hook or CI, not every `cargo build` — it's a release-time check, not a build-time one (see [SPEC §8](SPEC.md#8-optional-cargotoml-successor-validation)).

A version is a legal successor when it is one of:

- equal to the latest tag (not yet bumped — fine between releases),
- patch + 1,
- minor + 1 with patch reset to 0,
- major + 1 with minor and patch reset to 0.

Anything else is either a regression (`LessThanLatest`) or an illegal jump (`IllegalGap`). If the latest tag is itself a prerelease, the check errors out rather than guessing — deciding what's legal mid-RC-cycle is out of scope for v1.

## Tag prefix

A leading `v` or `V` is stripped from tags before parsing (`v1.0.0` → `1.0.0`). This handles the common convention without configuration; exotic prefixes can be stripped by the caller before constructing a `Describe`.

## 0.x is not special-cased

This crate is about *ordering*, not *compatibility*. It does not apply Cargo's left-shifted semver rules for pre-1.0 crates — `0.1.0` → `0.2.0` is a minor bump (patch reset to 0), same as `1.1.0` → `1.2.0`. If you want different bump semantics for 0.x, validate that in your own release tooling; `semvertag` just makes the ordering monotonic.

## Shallow clones

CI checkouts with `fetch-depth: 1` are the most common real-world failure mode: `git describe` runs but reports a commit count of 0 and the bare HEAD hash, producing a plausible-looking but **wrong** version. `semvertag-shell` detects a `.git/shallow` file and returns `ShellError::ShallowClone` instead of a silently incorrect result. The check is best-effort — it covers the common worktree-root case but may miss `gitdir:` pointer files or submodules. A missed detection surfaces as a wrong version, not a crash, which is acceptable until a real-world failure forces a more complete probe.

## License

MIT OR Apache-2.0.
