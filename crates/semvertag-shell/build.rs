//! Dogfooding: derive semvertag-shell's own version from git at build time.
//!
//! This is the §4.5 "documented pattern" — a build.rs that uses
//! `semvertag-core` (as a build-dependency) to parse `git describe` output and
//! derive a comparable version, then emits it as `cargo:rustc-env` so
//! `env!("SEMVERTAG_VERSION")` is available in the crate as
//! [`semvertag_shell::VERSION`](crate::VERSION).
//!
//! We intentionally do *not* depend on `semvertag-shell` itself here (that would
//! be a cyclic build-dependency). Instead we inline the minimal `git describe`
//! invocation and lean on `semvertag-core` for parsing/derivation — exactly the
//! pattern the spec documents for crates that want to embed their own version
//! without pulling in a full adapter at build time. Once `semvertag-macros`
//! exists (milestone 5) this build.rs collapses to a single macro call.
//!
//! On any failure (no git, no tags, shallow clone, unparseable output) we fall
//! back rather than failing the build — a missing version string should never
//! break `cargo build` for someone who just unpacked a tarball.
//!
//! When git is reachable but there are no tags, we derive `0.0.0-dev.N+g<hash>`
//! where `N` is the total commit count (`git rev-list --count HEAD`). This keeps
//! an untagged project's versions monotonic and below any tagged release,
//! matching the §5 derivation scheme with `0.0.0` as the implicit base.
//!
//! Only when git itself is unreachable (no `.git` dir, shallow clone, git
//! binary missing) do we fall back to the static `0.0.0-unknown`.

use std::path::{Path, PathBuf};
use std::process::Command;

const FALLBACK: &str = "0.0.0-unknown";

fn main() {
    // Re-run whenever the git state might have changed. Best-effort per §4.5;
    // misses packed refs, but that's documented.
    for p in [".git/HEAD", ".git/index", ".git/refs/tags"] {
        println!("cargo:rerun-if-changed={p}");
    }

    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let version = derive_version(&manifest).unwrap_or_else(|| FALLBACK.to_string());
    println!("cargo:rustc-env=SEMVERTAG_VERSION={version}");
}

/// Walk upward from `start` to find a directory containing `.git`.
fn find_git_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

fn derive_version(manifest_dir: &Path) -> Option<String> {
    let repo = find_git_root(manifest_dir)?;

    // Best-effort shallow-clone check (mirrors semvertag-shell's own logic, but
    // we can't use that crate here without a cycle).
    if repo.join(".git/shallow").exists() {
        return None;
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "describe",
            "--tags",
            "--long",
            "--always",
            "--dirty=.dirty",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    let raw = raw.trim();

    // `parse_describe_string` returns `NoTagsFound` when `--always` emitted a
    // bare hash (no tags in the repo). In that case derive `0.0.0-dev.N+g<hash>`
    // using the total commit count as `N`.
    match semvertag_core::parse_describe_string(raw) {
        Ok(describe) => {
            let version = semvertag_core::derive(&describe).ok()?;
            Some(version.to_string())
        }
        Err(semvertag_core::DeriveError::NoTagsFound) => {
            let count = commit_count(&repo)?;
            let hash = bare_short_hash(&repo, raw)?;
            let mut build = format!("g{hash}");
            if raw.ends_with(".dirty") {
                build.push_str(".dirty");
            }
            Some(format!("0.0.0-dev.{count}+{build}"))
        }
        Err(_) => None,
    }
}

/// `git rev-list --count HEAD` — total number of commits reachable from HEAD.
fn commit_count(repo: &Path) -> Option<u64> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?;
    s.trim().parse().ok()
}

/// Extract the short hash from a `--always` bare output (which is just the
/// hash, optionally with a `.dirty` suffix). Returns the hash without the `g`
/// prefix (we add it ourselves).
fn bare_short_hash(_repo: &Path, raw: &str) -> Option<String> {
    let hash = raw.strip_suffix(".dirty").unwrap_or(raw);
    if hash.chars().all(|c| c.is_ascii_hexdigit()) && !hash.is_empty() {
        Some(hash.to_string())
    } else {
        None
    }
}
