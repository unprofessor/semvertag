# cargo-semvertag

**`cargo-semvertag`: git-tag-derived SemVer tooling.**

Two commands, plus the standard version flag:

- `cargo semvertag` (default) &mdash; print the version derived from `git describe`,
  e.g. `1.2.4-dev.3+g87af40b`. Handy in build scripts.
- `cargo semvertag check` &mdash; validate your `Cargo.toml` version against the
  latest git tag.
- `cargo-semvertag --version` / `-V` &mdash; print the tool's own version.

The default `derive` command reads `Cargo.toml`'s `package.version` and, when
it is a legal successor of the latest tag (patch + 1, minor + 1 with patch = 0,
or major + 1 with minor = patch = 0), targets it: with tag `v0.1.0` and a
manifest already bumped to `0.2.0`, the derived version is
`0.2.0-dev.N+g<hash>`, not `0.1.1-dev.N+g<hash>` &mdash; a developer-performed
minor or major bump is honored rather than overridden by a blind patch bump.
A missing, stale, or illegal manifest version silently falls back to the
patch bump; `--manifest-path` selects a different manifest (e.g. a workspace
member).

## `check`

Ever tagged `v0.3.0` only to realize `Cargo.toml` still says `0.2.0`?
`cargo-semvertag` catches that before you tag:

```sh
$ cargo install cargo-semvertag
$ cargo semvertag check
ok: Cargo.toml version 1.2.4 is a legal successor to tag 1.2.3
```

A version is legal when it is:

- equal to the latest tag, but only at the tagged release commit itself,
- patch + 1,
- minor + 1 (patch reset to 0),
- major + 1 (minor and patch reset to 0).

The strict equality rule enforces bump-on-first-commit discipline: the first
commit after a release must already carry the next version, so an untagged
commit never reports a version equal to a released one. Anything else is a
regression or an illegal jump. If the latest tag is itself a prerelease, the
check bails out rather than guessing.

`check` expects the whole repository to be **uniformly versioned** -- git
tags commits, not trees -- so a workspace is validated against its single
shared version: `version.workspace = true` members resolve through the
root's `[workspace.package].version`, and at a virtual workspace root
(`[workspace]` without `[package]`) that workspace version is checked
directly. Mixed-version workspaces (members pinning their own divergent
versions) are out of scope -- one tag cannot validate many versions.

**Exit codes:** `0` ok, `1` check failed, `2` operational error (no git, no
tags, unreadable `Cargo.toml`).

Wire it into a pre-tag hook or CI &mdash; it's a release-time check, not something
you run on every build.

Git state is probed at the *manifest's* repository root, so `--manifest-path`
into a different repository validates against that repository's tags, and
the shallow-clone guard fires consistently from any subdirectory.

The version derivation logic lives in
[`semvertag-core`](https://crates.io/crates/semvertag-core) and
[`semvertag-shell`](https://crates.io/crates/semvertag-shell), which are
published separately for build-time version embedding.

## License

MIT OR Apache-2.0, at your option.
