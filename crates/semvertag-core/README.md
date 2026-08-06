# semvertag-core

**Turn `git describe`-style strings into SemVer versions with intuitive ordering.**

`git describe --tags` produces strings like `v1.0.0-5-g87af40b`. Treated naively
as SemVer, the `-5-g87af40b` suffix parses as a *prerelease*, so a commit 5 past
`v1.0.0` sorts **before** `v1.0.0` — the opposite of what you meant. This crate
rewrites such strings so that every commit forward is a version forward:

```text
0.9.9 < 1.0.0-rc.1 < 1.0.0-rc.1.dev.3 < 1.0.0-rc.2 < 1.0.0 < 1.0.1-dev.5 < 1.0.1
```

The logic is pure and performs no I/O: feed it a `Describe` struct (built by
hand or via [`parse_describe_string`]) and call [`derive`].

## Usage

```rust
use semvertag_core::{parse_describe_string, derive};

let describe = parse_describe_string("v1.0.0-5-g87af40b")?;
let version = derive(&describe)?;
assert_eq!(version.to_string(), "1.0.1-dev.5+g87af40b");
```

## Derivation rules

1. Tag at HEAD, clean → the tag as-is (`v1.0.0` → `1.0.0`).
2. Tag at HEAD, dirty → tag with `+dirty` build metadata (provenance, not ordering).
3. Commits past a plain release → patch bump + `dev.{N}` prerelease + `g{hash}` build metadata.
4. Commits past a prerelease tag → keep the version, append `.dev.{N}` to the prerelease + `g{hash}` build metadata.

A leading `v`/`V` is stripped from tags. 0.x versions are treated like any other
major — no Cargo-style left-shifted semantics.

## When to use this crate directly

`semvertag-core` is the right choice when you already invoke `git` yourself (or
use `git2`) and only need the derivation — for example in a `build.rs`, where
depending on `semvertag-shell` would create a dependency cycle if your crate
also uses `semvertag-core` at runtime. Otherwise, prefer
[`semvertag-shell`](https://crates.io/crates/semvertag-shell), which handles the
`git describe` invocation and shallow-clone detection for you.

See the [project repository](https://github.com/unprofessor/semvertag) for the
full spec (`SPEC.md`), the CLI (`cargo semvertag check`), and worked examples.

## License

MIT OR Apache-2.0, at your option.
