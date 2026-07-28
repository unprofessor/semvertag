//! Shell-out adapter that derives a [`semver::Version`] by invoking
//! `git describe --tags --long --always --dirty=.dirty` and feeding the output
//! to [`semvertag_core`].
//!
//! # Shallow clones
//!
//! CI checkouts with `fetch-depth: 1` are the most common real-world failure
//! mode: `git describe` runs but reports a commit count of 0 and the bare HEAD
//! hash, producing a plausible-looking but wrong version. This adapter detects
//! a `.git/shallow` file and returns [`ShellError::ShallowClone`] instead of a
//! silently incorrect result. The check is best-effort — it covers the common
//! worktree-root case but may miss `gitdir:` pointer files, submodules, or
//! other layouts; a missed detection surfaces as a wrong version rather than a
//! crash, which is acceptable until a real-world failure forces a more complete
//! probe (see SPEC §4.2).
//!
//! # Examples
//!
//! ```
//! use semvertag_shell::describe;
//!
//! // Call from a build.rs or runtime; returns a parsed version or an error.
//! match describe() {
//!     Ok(v) => println!("version: {v}"),
//!     Err(e) => eprintln!("could not derive version: {e}"),
//! }
//! ```

use std::path::Path;
use std::process::Command;

use semvertag_core::{parse_describe_string, derive, Describe, DeriveError};

/// Re-exported so callers don't need a direct `semver` dependency to name the
/// return type of [`describe`] / [`describe_in`].
pub use semver::Version;

/// Errors from the shell adapter.
#[derive(Debug)]
pub enum ShellError {
    /// The working directory (or one of its parents) has a `.git/shallow` file,
    /// indicating a shallow clone. `git describe` would report a wrong commit
    /// count, so we refuse rather than produce a misleading version.
    ShallowClone,
    /// `git` could not be found on `PATH`, or invoking it failed.
    GitUnavailable,
    /// `git describe` exited non-zero (e.g. no tags and `--always` not reached,
    /// or a genuine git internal error). `stderr` is included for diagnostics.
    GitDescribeFailed { stderr: String },
    /// The `git describe` output was produced but could not be parsed, or the
    /// derived version was invalid. Wraps the core [`DeriveError`].
    Core(DeriveError),
}

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellError::ShallowClone => write!(
                f,
                "shallow clone detected (.git/shallow present); git describe would report a wrong commit count — fetch full history or use fetch-depth: 0"
            ),
            ShellError::GitUnavailable => write!(f, "git binary not found on PATH"),
            ShellError::GitDescribeFailed { stderr } => {
                write!(f, "git describe failed: {stderr}")
            }
            ShellError::Core(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ShellError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ShellError::Core(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DeriveError> for ShellError {
    fn from(e: DeriveError) -> Self {
        ShellError::Core(e)
    }
}

/// The version of semvertag-shell itself, derived from git at build time via
/// the crate's own `build.rs` (dogfooding — see SPEC §4.5).
///
/// Falls back to `"0.0.0-unknown"` when the build couldn't reach git.
pub const VERSION: &str = env!("SEMVERTAG_VERSION");

/// Run `git describe` in the current working directory and derive a
/// [`semver::Version`] from its output.
///
/// Equivalent to [`describe_in`] with `.` as the repo path.
pub fn describe() -> Result<semver::Version, ShellError> {
    describe_in(Path::new("."))
}

/// Run `git describe` in `repo` and derive a [`semver::Version`] from its
/// output.
///
/// `repo` is passed to `git -C <repo>`. Detects a shallow clone (a
/// `.git/shallow` file under `repo`) and returns [`ShellError::ShallowClone`]
/// before invoking git.
pub fn describe_in(repo: &Path) -> Result<semver::Version, ShellError> {
    if is_shallow(repo) {
        return Err(ShellError::ShallowClone);
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "describe",
            "--tags",
            "--long",
            "--always",
            "--dirty=.dirty",
        ])
        .output()
        .map_err(|_| ShellError::GitUnavailable)?;

    if !output.status.success() {
        return Err(ShellError::GitDescribeFailed {
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let describe = parse_describe_string(&raw).map_err(ShellError::from)?;
    let version = derive(&describe).map_err(ShellError::from)?;
    Ok(version)
}

/// Like [`describe_in`] but returns the parsed [`Describe`] without deriving the
/// final version — useful when a caller wants to inspect the commit count or
/// hash, or apply a custom derivation.
pub fn describe_raw(repo: &Path) -> Result<Describe, ShellError> {
    if is_shallow(repo) {
        return Err(ShellError::ShallowClone);
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "describe",
            "--tags",
            "--long",
            "--always",
            "--dirty=.dirty",
        ])
        .output()
        .map_err(|_| ShellError::GitUnavailable)?;

    if !output.status.success() {
        return Err(ShellError::GitDescribeFailed {
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    parse_describe_string(&raw).map_err(ShellError::from)
}

/// Best-effort shallow-clone check: look for `.git/shallow` under `repo`.
///
/// This handles the common worktree-root layout. It may miss `gitdir:` pointer
/// files, submodules, or worktrees split via `git worktree` — all known
/// limitations, documented in SPEC §4.2.
fn is_shallow(repo: &Path) -> bool {
    repo.join(".git").join("shallow").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    /// Whether `git` is available and functional on this host. The integration
    /// tests below all require it; we skip (not fail) when it's absent.
    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Set up an isolated git repo in a temp dir with sane identity, returning
    /// the `TempDir` (kept alive for the test's duration) and the repo path.
    fn fresh_repo() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().to_path_buf();
        run_git(&path, &["init", "--quiet"]);
        // Avoid touching the user's global git config.
        for (k, v) in [
            ("user.name", "semvertag-test"),
            ("user.email", "test@semvertag.invalid"),
            ("commit.gpgsign", "false"),
            ("tag.gpgsign", "false"),
        ] {
            run_git(&path, &["config", k, v]);
        }
        (dir, path)
    }

    fn run_git(repo: &Path, args: &[&'static str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git invocation");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn commit(repo: &Path, msg: &'static str) {
        // Deterministic content so each commit is distinct.
        let count = std::fs::read_dir(repo).map(|d| d.count()).unwrap_or(0);
        std::fs::write(repo.join(format!("f{count}.txt")), msg).unwrap();
        run_git(repo, &["add", "--all"]);
        run_git(repo, &["commit", "--quiet", "-m", msg]);
    }

    /// Extract the short hash for HEAD from the derived version's build
    /// metadata, after stripping the `g` prefix.
    fn head_short_hash(repo: &Path) -> String {
        let full = run_git(repo, &["rev-parse", "HEAD"]);
        full.chars().take(7).collect()
    }

    #[test]
    fn release_tag_at_head() {
        if !git_available() {
            return;
        }
        let (_dir, repo) = fresh_repo();
        commit(&repo, "initial");
        run_git(&repo, &["tag", "-a", "v1.0.0", "-m", "release"]);
        let v = describe_in(&repo).unwrap();
        assert_eq!(v.to_string(), "1.0.0");
    }

    #[test]
    fn release_tag_three_commits_past() {
        if !git_available() {
            return;
        }
        let (_dir, repo) = fresh_repo();
        commit(&repo, "initial");
        run_git(&repo, &["tag", "-a", "v1.0.0", "-m", "release"]);
        commit(&repo, "two");
        commit(&repo, "three");
        commit(&repo, "four");
        let v = describe_in(&repo).unwrap();
        let expected = format!("1.0.1-dev.3+g{}", head_short_hash(&repo));
        assert_eq!(v.to_string(), expected);
    }

    #[test]
    fn prerelease_tag_two_commits_past() {
        if !git_available() {
            return;
        }
        let (_dir, repo) = fresh_repo();
        commit(&repo, "initial");
        run_git(&repo, &["tag", "-a", "v1.0.0-rc.1", "-m", "rc"]);
        commit(&repo, "two");
        commit(&repo, "three");
        let v = describe_in(&repo).unwrap();
        let expected = format!("1.0.0-rc.1.dev.2+g{}", head_short_hash(&repo));
        assert_eq!(v.to_string(), expected);
    }

    #[test]
    fn dirty_worktree_reflected() {
        if !git_available() {
            return;
        }
        let (_dir, repo) = fresh_repo();
        commit(&repo, "initial");
        run_git(&repo, &["tag", "-a", "v1.0.0", "-m", "release"]);
        // Modify a tracked file without committing. Find the one commit() created.
        let tracked = run_git(&repo, &["ls-files"]);
        let tracked = tracked.trim();
        assert!(!tracked.is_empty(), "expected a tracked file to exist");
        std::fs::write(repo.join(tracked), "changed").unwrap();
        let v = describe_in(&repo).unwrap();
        assert!(
            v.build.as_str().contains("dirty"),
            "expected 'dirty' in build metadata, got {}",
            v.build
        );
    }

    #[test]
    fn no_tags_surfaces_no_tags_found() {
        if !git_available() {
            return;
        }
        let (_dir, repo) = fresh_repo();
        commit(&repo, "initial");
        let err = describe_in(&repo).unwrap_err();
        // `git describe --always` succeeds with a bare hash, which core parses
        // as NoTagsFound.
        assert!(
            matches!(
                err,
                ShellError::Core(DeriveError::NoTagsFound)
            ),
            "expected NoTagsFound, got {err:?}"
        );
    }

    #[test]
    fn lightweight_tag_is_found() {
        if !git_available() {
            return;
        }
        let (_dir, repo) = fresh_repo();
        commit(&repo, "initial");
        // Lightweight: no -a, no -m.
        run_git(&repo, &["tag", "v1.0.0"]);
        let v = describe_in(&repo).unwrap();
        assert_eq!(v.to_string(), "1.0.0");
    }

    #[test]
    fn shallow_clone_is_detected() {
        if !git_available() {
            return;
        }
        // Set up fixture #2's state in a source repo...
        let (_src_dir, src) = fresh_repo();
        commit(&src, "initial");
        run_git(&src, &["tag", "-a", "v1.0.0", "-m", "release"]);
        commit(&src, "two");
        commit(&src, "three");
        commit(&src, "four");

        // ...then shallow-clone it with depth 1. Local clones ignore --depth
        // unless the source is given as a file:// URL.
        let clone_dir = TempDir::new().unwrap();
        let clone_path = clone_dir.path().to_path_buf();
        let src_url = format!("file://{}", src.display());
        let ok = Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                "--quiet",
                &src_url,
                clone_path.to_str().unwrap(),
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            // Some sandboxed environments block clone; skip rather than fail.
            return;
        }

        let err = describe_in(&clone_path).unwrap_err();
        assert!(
            matches!(err, ShellError::ShallowClone),
            "expected ShallowClone, got {err:?}"
        );
    }

    #[test]
    fn two_tags_same_commit_does_not_panic() {
        if !git_available() {
            return;
        }
        let (_dir, repo) = fresh_repo();
        commit(&repo, "initial");
        run_git(&repo, &["tag", "-a", "v1.0.0", "-m", "one"]);
        run_git(&repo, &["tag", "-a", "v1.1.0", "-m", "two"]);
        // Whichever git describe picks, we just require no panic and a valid
        // Ok result at HEAD (commits_since == 0 for both).
        let v = describe_in(&repo).unwrap();
        assert!(matches!(v.major, 1));
        assert_eq!(v.pre.as_str(), "");
    }
}
