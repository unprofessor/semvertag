//! `cargo-semvertag`: validate that `Cargo.toml`'s `package.version` is a legal
//! next step from the latest git tag.
//!
//! Invoked as `cargo semvertag check` (cargo subcommand) or
//! `cargo-semvertag check` (standalone). Intended for CI or a pre-tag hook, not
//! for every `cargo build` — see SPEC §8.
//!
//! # What it checks
//!
//! Given the latest git tag (e.g. `v1.2.3`) and the `package.version` in
//! `Cargo.toml`, it verifies the Cargo.toml version is a legal single-step
//! bump: equal, patch+1, minor+1 (patch=0), or major+1 (minor=patch=0). See
//! [`semvertag_core::is_valid_successor`].
//!
//! # Exit codes
//!
//! - `0` — the version is a legal successor (or equal).
//! - `1` — the version is not a legal successor (`LessThanLatest` or `IllegalGap`).
//! - `2` — an error occurred (no git, no tags, unreadable Cargo.toml, etc.).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use semver::Version;
use semvertag_core::{is_valid_successor, SuccessorError};
use semvertag_shell::VERSION;

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
    let args = parse_args()?;
    match args.command {
        Command::Check => check(&args),
        Command::Version => {
            println!("cargo-semvertag {VERSION}");
            Ok(())
        }
    }
}

struct Args {
    command: Command,
    manifest: Option<PathBuf>,
}

enum Command {
    Check,
    Version,
}

fn parse_args() -> Result<Args, AppError> {
    use lexopt::prelude::*;

    let mut parser = lexopt::Parser::from_env();

    let mut command = None;
    let mut manifest = None;

    while let Some(arg) = parser.next().map_err(|e| AppError::Other(e.to_string()))? {
        match arg {
            // When invoked as `cargo semvertag check`, cargo passes `semvertag`
            // as the first arg. Skip it.
            Value(v) if command.is_none() => {
                match v
                    .string()
                    .map_err(|e| AppError::Other(e.to_string()))?
                    .as_str()
                {
                    "semvertag" => continue,
                    "check" => command = Some(Command::Check),
                    "version" => command = Some(Command::Version),
                    other => {
                        return Err(AppError::Other(format!(
                            "unknown subcommand `{other}`; expected `check` or `version`"
                        )));
                    }
                }
            }
            Long("manifest-path") => {
                let val = parser
                    .value()
                    .map_err(|e| AppError::Other(e.to_string()))?
                    .string()
                    .map_err(|e| AppError::Other(e.to_string()))?;
                manifest = Some(PathBuf::from(val));
            }
            Long("version") => {
                command = Some(Command::Version);
            }
            Long(name) => {
                return Err(AppError::Other(format!("unknown option --{name}")));
            }
            Short('V') => {
                command = Some(Command::Version);
            }
            _ => return Err(AppError::Other("unexpected argument".to_string())),
        }
    }

    let command = command.unwrap_or(Command::Check);
    Ok(Args { command, manifest })
}

fn check(args: &Args) -> Result<(), AppError> {
    // 1. Resolve the latest tag via the shell adapter.
    let describe = semvertag_shell::describe_raw(Path::new("."))
        .map_err(|e| AppError::Other(format!("could not read git state: {e}")))?;

    let latest = parse_tag_version(&describe.tag).map_err(|e| {
        AppError::Other(format!(
            "latest tag `{}` is not a valid SemVer version: {e}",
            describe.tag
        ))
    })?;

    // 2. Read package.version from Cargo.toml (workspace-aware).
    let manifest_path = args
        .manifest
        .clone()
        .unwrap_or_else(|| PathBuf::from("Cargo.toml"));

    let cargo_version = read_package_version(&manifest_path)
        .map_err(|e| AppError::Other(format!("could not read {}: {e}", manifest_path.display())))?;

    // 3. Validate.
    match is_valid_successor(&latest, &cargo_version) {
        Ok(()) => {
            println!(
                "ok: Cargo.toml version {} is a legal successor to tag {}",
                cargo_version, latest
            );
            Ok(())
        }
        Err(e @ SuccessorError::LessThanLatest { .. })
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
