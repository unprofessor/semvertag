//! `cargo-semvertag`: git-tag-derived SemVer tooling.
//!
//! Invoked as `cargo semvertag` (cargo subcommand) or `cargo-semvertag`
//! (standalone).
//!
//! # Commands
//!
//! - *(default)* `cargo semvertag` — print the version derived from `git
//!   describe` (the result of [`semvertag_core::derive`], e.g.
//!   `1.2.4-dev.3+g87af40b`). Handy for build scripts and version embedding.
//! - `cargo semvertag derive` — same, explicitly.
//! - `cargo semvertag check` — validate that `Cargo.toml`'s `package.version`
//!   is legal for the current commit: equal to the latest tag at the tagged
//!   release commit itself, or a legal single-step bump (patch+1, minor+1 with
//!   patch=0, or major+1 with minor=patch=0) on any commit after it. Intended
//!   for CI or a pre-tag hook, not for every `cargo build` — see SPEC §8 and
//!   [`semvertag_core::is_valid_successor`].
//!
//! `--version` / `-V` prints the `cargo-semvertag` version (standard CLI
//! semantics).
//!
//! # Exit codes
//!
//! - `0` — success.
//! - `1` — `check` failed: the version is not legal (`LessThanLatest`,
//!   `NotBumped`, `TagManifestMismatch`, or `IllegalGap`).
//! - `2` — an error occurred (no git, no tags, unreadable Cargo.toml, etc.).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use semver::Version;
use semvertag_core::{derive, is_valid_successor, SuccessorError};
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
    Derive,
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
        None | Some(Command::Derive) => print_derived_version(),
        Some(Command::Check { manifest_path }) => check(manifest_path),
    }
}

/// Print the version derived from `git describe` (SPEC §5).
fn print_derived_version() -> Result<(), AppError> {
    let describe = describe_raw(Path::new("."))
        .map_err(|e| AppError::Other(format!("could not read git state: {e}")))?;
    let version =
        derive(&describe).map_err(|e| AppError::Other(format!("could not derive version: {e}")))?;
    println!("{version}");
    Ok(())
}

fn check(manifest_path: Option<PathBuf>) -> Result<(), AppError> {
    // 1. Resolve the latest tag via the shell adapter.
    let describe = describe_raw(Path::new("."))
        .map_err(|e| AppError::Other(format!("could not read git state: {e}")))?;

    let latest = parse_tag_version(&describe.tag).map_err(|e| {
        AppError::Other(format!(
            "latest tag `{}` is not a valid SemVer version: {e}",
            describe.tag
        ))
    })?;

    // 2. Read package.version from Cargo.toml (workspace-aware).
    let manifest_path = manifest_path.unwrap_or_else(|| PathBuf::from("Cargo.toml"));

    let cargo_version = read_package_version(&manifest_path)
        .map_err(|e| AppError::Other(format!("could not read {}: {e}", manifest_path.display())))?;

    // 3. Validate: equality with the latest tag is legal only at the tagged
    //    release commit itself; every commit after it must already carry the
    //    next version.
    match is_valid_successor(&latest, &cargo_version, describe.commits_since) {
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
fn read_package_version(path: &Path) -> Result<Version, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let manifest: toml::Value = toml::from_str(&content)?;

    let package = manifest
        .get("package")
        .ok_or_else(|| "missing [package] table".to_string())?;

    // Direct version: package.version = "1.2.3"
    if let Some(v) = package.get("version").and_then(|v| v.as_str()) {
        return Ok(Version::parse(v)?);
    }

    // Inherited: version.workspace = true → resolve from workspace root.
    let workspace_val = package
        .get("version")
        .and_then(|v| v.get("workspace"))
        .and_then(|v| v.as_bool());

    if workspace_val == Some(true) {
        let manifest_dir = path.parent().unwrap_or(Path::new("."));
        return resolve_inherited_version(manifest_dir);
    }

    Err("package.version not found or not a string".into())
}

/// Walk upward from `member_dir` to find a `Cargo.toml` containing a
/// `[workspace]` table, then read `[workspace.package].version`.
fn resolve_inherited_version(member_dir: &Path) -> Result<Version, Box<dyn std::error::Error>> {
    for dir in member_dir.ancestors() {
        let candidate = dir.join("Cargo.toml");
        if !candidate.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&candidate)?;
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
    Err("version.workspace = true but no workspace root found".into())
}
