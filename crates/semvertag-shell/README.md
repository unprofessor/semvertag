# semvertag-shell

**Shell-out adapter: derive a SemVer version from `git describe`.**

Invokes `git describe --tags --long --always --dirty=.dirty`, feeds the output
to [`semvertag-core`](https://crates.io/crates/semvertag-core), and returns a
parsed `semver::Version` that orders correctly — every commit forward is a
version forward.

## Usage: embed your git version at build time

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

```rust,no_run
// src/lib.rs
pub const VERSION: &str = env!("SEMVERTAG_VERSION");
```

## Shallow clones

CI checkouts with `fetch-depth: 1` are the most common real-world failure mode:
`git describe` runs but reports a commit count of 0, yielding a plausible-looking
but wrong version. This crate detects a `.git/shallow` file and returns
`ShellError::ShallowClone` instead of a silently incorrect result.

For more control, use `describe_in(repo)` (a custom repository path) or
`describe_raw(repo)` (the raw `Describe` struct, before derivation).

If your crate also uses `semvertag-core` at runtime, depend on `semvertag-core`
directly instead and invoke `git` yourself — see the
[project README](https://github.com/unprofessor/semvertag) for a worked
`build.rs` example.

## License

MIT OR Apache-2.0, at your option.
