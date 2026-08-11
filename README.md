# semvertag

[![crates.io](https://img.shields.io/crates/v/semvertag-core?label=semvertag-core)](https://crates.io/crates/semvertag-core)
[![docs.rs](https://img.shields.io/docsrs/semvertag-core)](https://docs.rs/semvertag-core)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](./LICENSE-MIT)

**Embed your git version at build time &mdash; _and_ have it sort correctly.**

---

The problem: you want your Rust binary to know its own version. The obvious answer is `git describe --tags`, which produces strings like `v1.0.0-5-g87af40b`. But if you feed that to a SemVer parser, the `-5-g87af40b` suffix gets treated as a _prerelease_ identifier &mdash; which means a build 5 commits past `v1.0.0` sorts **before** `v1.0.0`. That's backwards. Your users see `1.0.0` as the "latest" even when they're running something older.

`semvertag` rewrites `git describe` output into proper SemVer strings where every commit forward is a version forward:

```text
0.9.9 < 1.0.0-rc.1 < 1.0.0-rc.1.dev.3 < 1.0.0-rc.2 < 1.0.0 < 1.0.1-dev.5 < 1.0.1
```

In other words: **monotonic ordering from git history**. Releases sort above release candidates, RCs sort above dev snapshots, and every dev build knows exactly where it sits on the timeline.

## How it works

| `git describe --long` output   | derived version             | what's happening                          |
|--------------------------------|------------------------------|-------------------------------------------|
| `0.9.0-0-gabc1234`             | `0.9.0`                      | tagged release at HEAD &rarr; unchanged        |
| `v1.0.0-rc.1-0-gabc1234`       | `1.0.0-rc.1`                 | prerelease tag at HEAD &rarr; unchanged        |
| `v1.0.0-rc.1-3-g87af40b`       | `1.0.0-rc.1.dev.3+g87af40b`  | 3 commits past an RC &rarr; `.dev.3` appended  |
| `v1.0.0-rc.2-0-gabc1234`       | `1.0.0-rc.2`                 | later RC sorts above earlier one          |
| `v1.0.0-0-gabc1234`            | `1.0.0`                      | plain release beats any RC                |
| `v1.0.0-5-g87af40b`            | `1.0.1-dev.5+g87af40b`       | 5 past release &rarr; patch bump + dev marker  |
| `v0.1.0-3-g87af40b` (Cargo.toml: `0.2.0`) | `0.2.0-dev.3+g87af40b` | manifest declares the next release &rarr; hint replaces the blind patch bump |
| `v1.0.1-0-gabc1234`            | `1.0.1`                      | that release the dev builds led to        |

The derivation rules (see [SPEC.md &sect;5](SPEC.md#5-derivation-algorithm-formal) for the formal spec):

1. **Tag at HEAD, clean** &rarr; the tag as-is.
2. **Tag at HEAD, dirty** &rarr; tag with `+dirty` build metadata (not a prerelease &mdash; dirty/clean is provenance, not ordering).
3. **Commits past a plain release** &rarr; bump patch and mark as `dev.{N}` prerelease with `g{hash}` build metadata -- *unless* `Cargo.toml` declares a legal next version (e.g. `0.2.0` after tag `v0.1.0`); then that version is targeted instead (`0.2.0-dev.{N}`), so a developer-performed minor or major bump is honored rather than overridden by a blind patch bump.
4. **Commits past a prerelease tag** &rarr; keep the version, append `.dev.{N}` to the existing prerelease, attach `g{hash}` build metadata.

A dirty build and a clean build at the same commit compare as **equal** under strict SemVer (build metadata is ignored for precedence). This is intentional &mdash; you can't meaningfully order "built on someone's dirty laptop" against "built in CI."

## When to use semvertag

- **Build-time version embedding** &mdash; stamp your binary with a SemVer-compatible version derived from git, so `--version` always tells the truth.
- **CI and release pipelines** &mdash; every build has a unique, ordered version; tooling can tell an RC from a dev snapshot from a release.
- **Pre-release workflows** &mdash; if you ship release candidates (`v1.0.0-rc.1`, `-rc.2`, &hellip;), `semvertag` preserves prerelease ordering while still letting commits after an RC sort correctly.
- **`cargo semvertag check` in your tagging hook** &mdash; validate that `Cargo.toml`'s version is a legal bump from the latest git tag before you tag.

## When _not_ to use it

- You need a full release-automation tool (tagging, changelog generation, publishing). `semvertag` is one piece of the puzzle, not the whole pipeline.
- You want Cargo-style left-shifted semver for 0.x versions. `semvertag` treats `0.x` the same as `1.x` &mdash; it's about ordering, not compatibility semantics.

## Crates

| crate              | what it does                                          | deps            |
|--------------------|-------------------------------------------------------|-----------------|
| `semvertag-core`   | Pure derivation logic. No I/O.                        | `semver`        |
| `semvertag-shell`  | Shells out to `git describe`, feeds core.             | core + `semver` |
| `cargo-semvertag`  | `cargo semvertag` &mdash; print the git-derived version; `cargo semvertag check` &mdash; validate `Cargo.toml` version against the latest tag. | core + shell + `toml` + `clap` |

`semvertag-core` is the heart of it &mdash; feed it a `Describe` struct and call `derive()`. It's zero-I/O, fully testable, and useful even if you already call `git describe` yourself (or use `git2`).

## Quick start

### Verify compliance with `cargo semvertag check`

The quickest way to see if your project is compliant: install the CLI and run `check`.
Here's what a **non-compliant** project looks like — the tag says `1.0.0` but
`Cargo.toml` still reads `1.0.0` despite being several commits ahead:

```sh
$ cargo install cargo-semvertag
$ cargo semvertag check
error: Cargo.toml version 1.0.0 is not a legal successor to tag 1.0.0
       (tag 1.0.0 is a plain release and must already have a bumped manifest
        on the first post-release commit)
```

The first commit after a tagged release must bump `Cargo.toml`; otherwise `check`
exits with code `1`. Wire this into your tagging hook or CI to catch mismatches
before they ship (see [SPEC §8](SPEC.md#8-optional-cargotoml-successor-validation)).

The same command on a green project reports success — see the _Validate
`Cargo.toml` before you tag_ subsection further down.

### Embed your version with `semvertag-shell`

This is the easy path &mdash; one dependency, five lines of `build.rs`:

```toml
# Cargo.toml
[build-dependencies]
semvertag-shell = "0.1"
```

```rust
// build.rs
fn main() {
    let version = semvertag_shell::describe()
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "0.0.0-unknown".to_string());
    println!("cargo:rustc-env=SEMVERTAG_VERSION={version}");
}
```

```rust
// src/lib.rs -- now your crate exposes a compile-time version constant
pub const VERSION: &str = env!("SEMVERTAG_VERSION");
```

`describe()` runs `git describe --tags --long --always --dirty=.dirty` in the current directory, detects shallow clones (the most common CI gotcha), and returns a parsed `semver::Version`. Use `describe_in(repo)` or `describe_raw(repo)` when you need more control.

### Manual `git` invocation (escape hatch)

If your crate _is_ `semvertag-core` (or depends on it at runtime), using `semvertag-shell` in `build-dependencies` creates a cycle &mdash; shell depends on core, and your crate would depend on shell at build time while core depends on your crate at runtime. In that case, use `semvertag-core` directly and invoke `git` yourself:

```toml
# Cargo.toml
[build-dependencies]
semvertag-core = "0.1"
```

```rust
// build.rs
use std::process::Command;

fn main() {
    // Best-effort rerun triggers. Misses packed refs; see SPEC sec. 4.5.
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

The inline `Command::new("git")` approach is ~15 lines and avoids the cycle entirely. Most projects won't need this &mdash; it only matters when your crate is part of the `semvertag` workspace itself or transitively depends on `semvertag-core` at runtime.

### Validate `Cargo.toml` before you tag

Ever tagged `v0.3.0` only to realize `Cargo.toml` still says `0.2.0`? `cargo-semvertag` catches that:

```sh
$ cargo install cargo-semvertag
$ cargo semvertag check
ok: Cargo.toml version 1.2.4 is a legal successor to tag 1.2.3
```

A version is legal when it's one of:

- equal to the latest tag, but only at the tagged release commit itself &mdash; the manifest must match the tag there,
- patch + 1,
- minor + 1 (patch reset to 0),
- major + 1 (minor and patch reset to 0).

The strict equality rule enforces bump-on-first-commit discipline: the first
commit after a release must already carry the next version, so an untagged
commit never reports a version equal to a released one. Anything else is either
a regression or an illegal jump. If the latest tag is itself a prerelease, the
check bails out rather than guessing &mdash; deciding what's legal mid-RC is outside
v1 scope.

`check` expects the whole repository to be **uniformly versioned** &mdash; git tags
commits, not trees. Workspaces validate against their single shared version
(`version.workspace = true` resolves through the root's
`[workspace.package].version`; at a virtual workspace root that workspace
version is checked directly). One tag cannot validate several divergent
member versions.

Exit codes: `0` ok, `1` check failed, `2` operational error (no git, no tags, unreadable `Cargo.toml`). Wire this into a pre-tag hook or CI &mdash; it's a release-time check, not something you run on every build (see [SPEC &sect;8](SPEC.md#8-optional-cargotoml-successor-validation)).

Bare `cargo semvertag` (no subcommand) prints the version derived from `git`
`describe` instead &mdash; the same value `semvertag-core`'s `derive()` computes, e.g.
`1.2.4-dev.3+g87af40b`. `--version` prints the tool's own version.

## Design notes

### Tag prefix

A leading `v` or `V` is stripped from tags (`v1.0.0` &rarr; `1.0.0`). This covers the common convention without configuration. If you use an exotic prefix, strip it yourself before constructing a `Describe`.

### 0.x versions

`semvertag` treats 0.x the same as any other major version &mdash; `0.1.0` &rarr; `0.2.0` is a minor bump, just like `1.1.0` &rarr; `1.2.0`. It does _not_ apply Cargo's left-shifted semver rules for pre-1.0 crates. The crate is about ordering, not compatibility promises. If you need different bump semantics for 0.x, enforce that in your own release tooling.

### Shallow clones

The most common real-world failure: CI checks out with `fetch-depth: 1`, `git describe` reports a commit count of 0, and you ship a binary that thinks it's a tagged release when it isn't. `semvertag-shell` detects a `.git/shallow` file and returns `ShellError::ShallowClone` instead of a silently wrong version. The check is best-effort &mdash; it covers the common worktree-root case but may miss `gitdir:` pointer files or submodules. A missed detection means a wrong version, not a crash.

## License

MIT OR Apache-2.0, at your option.
