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
//! On any failure (no git, no tags, shallow clone, unparseable output) we emit
//! `0.0.0-unknown` rather than failing the build — a missing version string
//! should never break `cargo build` for someone who just unpacked a tarball.

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
    let describe = semvertag_core::parse_describe_string(raw.trim()).ok()?;
    let version = semvertag_core::derive(&describe).ok()?;
    Some(version.to_string())
}
