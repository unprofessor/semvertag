//! `cargo-semvertag`: git-tag-derived SemVer tooling.
//!
//! Invoked as `cargo semvertag` (cargo subcommand) or `cargo-semvertag`
//! (standalone).
//!
//! # Commands
//!
//! - *(default)* `cargo semvertag` -- print the version derived from `git
//!   describe` (the result of [`semvertag_core::derive_with_hint`], e.g.
//!   `1.2.4-dev.3+g87af40b`). Handy for build scripts and version embedding.
//!   When `Cargo.toml`'s `package.version` is a legal successor of the latest
//!   tag (patch+1, minor+1 with patch=0, or major+1 with minor=patch=0), it is
//!   honored as the target of the next release: a `0.2.0` manifest after tag
//!   `v0.1.0` yields `0.2.0-dev.N+...` rather than `0.1.1-dev.N+...`. A
//!   missing, stale, or illegal manifest version silently falls back to the
//!   tag-based patch bump. If the manifest is present but unreadable, a
//!   warning is printed and derivation falls back the same way.
//! - `cargo semvertag derive` -- same, explicitly.
//! - `cargo semvertag check` -- validate that `Cargo.toml`'s `package.version`
//!   is legal for the current commit: equal to the latest tag at the tagged
//!   release commit itself, or a legal single-step bump (patch+1, minor+1 with
//!   patch=0, or major+1 with minor=patch=0) on any commit after it. An
//!   uncommitted manifest-version bump at the tagged release commit is treated
//!   as the first commit of the next cycle (SPEC sec. 8.1). At a virtual
//!   workspace root (`[workspace]` without `[package]`), the version compared
//!   is the root's `[workspace.package].version` -- semvertag expects the
//!   repository to be uniformly versioned, since git tags commits, not trees.
//!   Intended for CI or a pre-tag hook, not for every `cargo build` -- see
//!   SPEC sec. 8 and [`semvertag_core::is_valid_successor`].
//!
//! `--version` / `-V` prints the `cargo-semvertag` version (standard CLI
//! semantics).
//!
//! # Exit codes
//!
//! - `0` -- success.
//! - `1` -- `check` failed: the version is not legal (`LessThanLatest`,
//!   `NotBumped`, `TagManifestMismatch`, or `IllegalGap`).
//! - `2` -- an error occurred (no git, no tags, unreadable Cargo.toml, etc.).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

use clap::{Parser, Subcommand};
use semver::Version;
use semvertag_core::{derive_with_hint, is_valid_successor, SuccessorError};
use semvertag_shell::describe_raw;

#[derive(Parser)]
#[command(
    name = "cargo-semvertag",
    version,
    about = "Print the SemVer version derived from git tags (default), or validate Cargo.toml against the latest tag (`check`)"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print the version derived from git describe (the default command).
    Derive {
        /// Path to the Cargo.toml whose package.version hints the next release.
        #[arg(long, value_name = "PATH")]
        manifest_path: Option<PathBuf>,
    },
    /// Validate Cargo.toml's package.version against the latest git tag.
    Check {
        /// Path to the Cargo.toml to check.
        #[arg(long, value_name = "PATH")]
        manifest_path: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(AppError::CheckFailed(e)) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
        Err(AppError::Other(msg)) => {
            eprintln!("error: {msg}");
            ExitCode::from(2)
        }
    }
}

/// A check failure (exit 1) vs. an operational error (exit 2).
enum AppError {
    CheckFailed(String),
    Other(String),
}

fn run() -> Result<(), AppError> {
    // `cargo semvertag ...` passes the subcommand name as the first argument;
    // strip it so `cargo semvertag` and `cargo-semvertag` behave identically.
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "semvertag") {
        args.remove(0);
    }
    // clap's parse_from expects argv[0] (the program name) as the first item.
    let cli = Cli::parse_from(std::iter::once(OsString::from("cargo-semvertag")).chain(args));
    match cli.command {
        None => print_derived_version(None),
        Some(Command::Derive { manifest_path }) => print_derived_version(manifest_path),
        Some(Command::Check { manifest_path }) => check(manifest_path),
    }
}

/// Print the version derived from `git describe` (SPEC sec. 5).
fn print_derived_version(manifest_path: Option<PathBuf>) -> Result<(), AppError> {
    // Probe git at the manifest's repository root when one is given, the
    // cwd's repository root otherwise -- see `probe_dir`.
    let anchor = manifest_path
        .as_deref()
        .map(manifest_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let describe = describe_raw(&probe_dir(&anchor))
        .map_err(|e| AppError::Other(format!("could not read git state: {e}")))?;
    // The manifest's package.version hints the declared next release; when it
    // is a legal successor of the tag, derivation targets it (a minor bump in
    // Cargo.toml yields 0.2.0-dev.N instead of 0.1.1-dev.N). Absent or
    // unusable, it falls back to the tag-based rules.
    let hint = read_version_hint(manifest_path);
    let version = derive_with_hint(&describe, hint.as_ref())
        .map_err(|e| AppError::Other(format!("could not derive version: {e}")))?;
    println!("{version}");
    Ok(())
}

/// Read `package.version` as the derivation hint. Returns `None` when the
/// manifest is absent (a non-Cargo repo -- derivation simply falls back to the
/// tag-based rules) or its version cannot be determined. A *present but
/// unreadable* manifest gets a warning, since that usually means the hint is
/// being silently ignored.
fn read_version_hint(manifest_path: Option<PathBuf>) -> Option<Version> {
    let path = manifest_path.unwrap_or_else(|| PathBuf::from("Cargo.toml"));
    match read_package_version(&path) {
        Ok(v) => Some(v),
        Err(e) => {
            if path.exists() {
                eprintln!(
                    "warning: could not read a package version from {}: {e}; \
                     deriving without a version hint",
                    path.display()
                );
            }
            None
        }
    }
}

fn check(manifest_path: Option<PathBuf>) -> Result<(), AppError> {
    // 1. The manifest anchors both the version read and the git probe: git
    //    state must come from the manifest's repository, not the cwd's.
    let manifest_path = manifest_path.unwrap_or_else(|| PathBuf::from("Cargo.toml"));

    let cargo_version = read_package_version(&manifest_path)
        .map_err(|e| AppError::Other(format!("could not read {}: {e}", manifest_path.display())))?;

    // 2. Resolve the latest tag via the shell adapter, probing at the repo
    //    root of the manifest's directory (see `probe_dir`).
    let describe = describe_raw(&probe_dir(&manifest_dir(&manifest_path)))
        .map_err(|e| AppError::Other(format!("could not read git state: {e}")))?;

    let latest = parse_tag_version(&describe.tag).map_err(|e| {
        AppError::Other(format!(
            "latest tag `{}` is not a valid SemVer version: {e}",
            describe.tag
        ))
    })?;

    // 3. At the tagged release commit, treat an uncommitted manifest-version
    //    change as if it were already the first commit of the next cycle:
    //    bumping Cargo.toml and running `check` before committing is the
    //    natural release workflow, and strict at-the-tag equality would punish
    //    exactly that order of operations. A clean tree at the tag (or a dirty
    //    tree whose manifest version is unchanged from HEAD's) is still judged
    //    by strict equality -- as is a manifest that already equals the tag.
    let commits_since = if describe.commits_since == 0 && describe.dirty && cargo_version != latest
    {
        match committed_package_version(&manifest_path) {
            // The working-tree manifest carries a version HEAD doesn't: judge
            // it by the post-release rules.
            Some(committed) if committed != cargo_version => 1,
            _ => 0,
        }
    } else {
        describe.commits_since
    };

    // 4. Validate: equality with the latest tag is legal only at the tagged
    //    release commit itself; every commit after it must already carry the
    //    next version.
    match is_valid_successor(&latest, &cargo_version, commits_since) {
        Ok(()) => {
            println!(
                "ok: Cargo.toml version {} is a legal successor to tag {}",
                cargo_version, latest
            );
            Ok(())
        }
        Err(e @ SuccessorError::LessThanLatest { .. })
        | Err(e @ SuccessorError::NotBumped { .. })
        | Err(e @ SuccessorError::TagManifestMismatch { .. })
        | Err(e @ SuccessorError::IllegalGap { .. }) => Err(AppError::CheckFailed(format!("{e}"))),
        Err(e @ SuccessorError::LatestIsPrerelease { .. }) => Err(AppError::Other(format!("{e}"))),
    }
}

/// Parse a tag string into a [`semver::Version`], stripping an optional `v`/`V`
/// prefix. Mirrors `semvertag_core`'s internal logic.
fn parse_tag_version(tag: &str) -> Result<Version, semver::Error> {
    let stripped = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
    Version::parse(stripped)
}

/// Read `package.version` from a Cargo.toml file, resolving workspace
/// inheritance (`version.workspace = true`) by walking up to the workspace
/// root's `[workspace.package].version`.
///
/// The manifest path is canonicalized first: the ancestor walk must be able
/// to climb above the current directory, which a relative parent like `.` or
/// an empty path cannot -- running inside a workspace member with a bare
/// `Cargo.toml` is the common case, and its workspace root may be several
/// levels up.
fn read_package_version(path: &Path) -> Result<Version, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    version_from_manifest(&content, &manifest_dir(path), &|dir| {
        std::fs::read_to_string(dir.join("Cargo.toml")).ok()
    })
}

/// The directory containing `path`, canonicalized so ancestor walks and git
/// probes can climb above the current directory. Falls back to the raw parent
/// when canonicalization fails.
fn manifest_dir(path: &Path) -> PathBuf {
    path.canonicalize()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| {
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf()
        })
}

/// The repository root containing `anchor`, so git state (describe output,
/// the shallow-clone guard) is probed consistently no matter how deep below
/// the repo root the tool runs -- and from whichever repo a `--manifest-path`
/// points into. Falls back to `anchor` itself when it isn't inside a
/// repository, preserving the adapter's own "not a repository" diagnostics.
fn probe_dir(anchor: &Path) -> PathBuf {
    git_stdout(anchor, &["rev-parse", "--show-toplevel"])
        .map(|s| PathBuf::from(s.trim_end()))
        .unwrap_or_else(|| anchor.to_path_buf())
}

/// Read `package.version` as committed at HEAD, resolving workspace
/// inheritance against committed ancestor manifests. Returns `None` when the
/// manifest (or its workspace root) isn't in HEAD or can't be parsed -- the
/// caller falls back to strict at-the-tag rules in that case.
fn committed_package_version(manifest_path: &Path) -> Option<Version> {
    let dir = manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Repo-relative path of the manifest ("" at the repo root), so the file can
    // be addressed as a git object even when the tool runs from a subdirectory.
    let prefix = git_stdout(dir, &["rev-parse", "--show-prefix"])?;
    let rel_dir = PathBuf::from(prefix.trim_end());
    let rel_manifest = rel_dir.join(manifest_path.file_name()?);
    let object = format!("HEAD:{}", slash(&rel_manifest));
    let content = git_stdout(dir, &["show", &object])?;
    version_from_manifest(&content, &rel_dir, &|d| {
        git_stdout(
            dir,
            &["show", &format!("HEAD:{}", slash(&d.join("Cargo.toml")))],
        )
    })
    .ok()
}

/// Parse `package.version` out of manifest `content`, resolving
/// `version.workspace = true` by walking `manifest_dir`'s ancestors and reading
/// each `Cargo.toml` via `read_ancestor` (the filesystem for the working tree,
/// `git show HEAD:` for the committed state).
///
/// A manifest without a `[package]` table is a virtual workspace root: its
/// version -- if any -- lives in `[workspace.package].version`, on the
/// assumption that the repository is uniformly versioned (git tags commits,
/// not trees; see SPEC sec. 8.1).
fn version_from_manifest(
    content: &str,
    manifest_dir: &Path,
    read_ancestor: &dyn Fn(&Path) -> Option<String>,
) -> Result<Version, Box<dyn std::error::Error>> {
    let manifest: toml::Value = toml::from_str(content)?;

    // Direct version: package.version = "1.2.3", or inherited via
    // version.workspace = true resolved from the workspace root.
    if let Some(package) = manifest.get("package") {
        if let Some(v) = package.get("version").and_then(|v| v.as_str()) {
            return Ok(Version::parse(v)?);
        }

        let workspace_val = package
            .get("version")
            .and_then(|v| v.get("workspace"))
            .and_then(|v| v.as_bool());

        if workspace_val == Some(true) {
            for dir in manifest_dir.ancestors() {
                let Some(content) = read_ancestor(dir) else {
                    continue;
                };
                let manifest: toml::Value = toml::from_str(&content)?;
                if manifest.get("workspace").is_some() {
                    if let Some(v) = manifest
                        .get("workspace")
                        .and_then(|w| w.get("package"))
                        .and_then(|p| p.get("version"))
                        .and_then(|v| v.as_str())
                    {
                        return Ok(Version::parse(v)?);
                    }
                    return Err("workspace.package.version not found in workspace root".into());
                }
            }
            return Err("version.workspace = true but no workspace root found".into());
        }

        return Err("package.version not found or not a string".into());
    }

    // No [package] table: a virtual workspace root. The next release version
    // is the one [workspace.package] declares for the whole repository.
    if let Some(v) = manifest
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
    {
        return Ok(Version::parse(v)?);
    }

    Err("missing [package] table (and no [workspace.package].version)".into())
}

/// Run `git` in `dir`, returning stdout on success and `None` on any failure.
fn git_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Render a repo-relative path with forward slashes, for `git show HEAD:...`.
fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
