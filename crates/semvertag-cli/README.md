# semvertag-cli

**`cargo semvertag check`: validate your `Cargo.toml` version against the latest git tag.**

Ever tagged `v0.3.0` only to realize `Cargo.toml` still says `0.2.0`?
`cargo-semvertag` catches that before you tag:

```sh
$ cargo install cargo-semvertag
$ cargo semvertag check
ok: Cargo.toml version 1.2.4 is a legal successor to tag 1.2.3
```

A version is legal when it is:

- equal to the latest tag (not yet bumped — fine between releases),
- patch + 1,
- minor + 1 (patch reset to 0),
- major + 1 (minor and patch reset to 0).

Anything else is a regression or an illegal jump. If the latest tag is itself a
prerelease, the check bails out rather than guessing.

**Exit codes:** `0` ok, `1` check failed, `2` operational error (no git, no
tags, unreadable `Cargo.toml`).

Wire it into a pre-tag hook or CI — it's a release-time check, not something
you run on every build.

The version derivation logic lives in
[`semvertag-core`](https://crates.io/crates/semvertag-core) and
[`semvertag-shell`](https://crates.io/crates/semvertag-shell), which are
published separately for build-time version embedding.

## License

MIT OR Apache-2.0, at your option.
