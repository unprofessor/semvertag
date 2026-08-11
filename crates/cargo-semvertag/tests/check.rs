//! Integration tests for `cargo-semvertag check`.
//!
//! Each test spins up a throwaway git repo with a `Cargo.toml`, tags it, and
//! invokes the binary via `assert_cmd`. Requires `git` on `PATH`; tests skip
//! (not fail) when git is unavailable.

use std::path::Path;
use std::process::Command;

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;
use tempfile::TempDir;

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fresh_repo() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().to_path_buf();
    run_git(&path, &["init", "--quiet"]);
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
    std::fs::write(repo.join("f.txt"), msg).unwrap();
    run_git(repo, &["add", "--all"]);
    run_git(repo, &["commit", "--quiet", "-m", msg]);
}

fn write_cargo_toml(repo: &Path, version: &str) {
    write_cargo_toml_at(&repo.join("Cargo.toml"), version);
}

fn write_cargo_toml_at(path: &Path, version: &str) {
    let content = format!(
        r#"[package]
name = "test-crate"
version = "{version}"
edition = "2021"
"#
    );
    std::fs::write(path, content).unwrap();
}

fn check_in(repo: &Path) -> AssertCommand {
    let mut cmd = AssertCommand::cargo_bin("cargo-semvertag").unwrap();
    cmd.arg("check");
    cmd.current_dir(repo);
    cmd
}

fn derive_in(repo: &Path) -> AssertCommand {
    let mut cmd = AssertCommand::cargo_bin("cargo-semvertag").unwrap();
    cmd.current_dir(repo);
    cmd
}

macro_rules! skip_if_no_git {
    () => {
        if !git_available() {
            return;
        }
    };
}

#[test]
fn check_equal_version_is_ok() {
    // At the tagged release commit itself, equality is the only legal state.
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);

    check_in(&repo)
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "ok: Cargo.toml version 1.2.3 is a legal successor to tag 1.2.3",
        ));
}

#[test]
fn check_patch_bump_is_ok() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    // The bump must land on the first commit after the tag.
    write_cargo_toml(&repo, "1.2.4");
    commit(&repo, "bump patch");

    check_in(&repo).assert().success();
}

#[test]
fn check_minor_bump_is_ok() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    write_cargo_toml(&repo, "1.3.0");
    commit(&repo, "bump minor");

    check_in(&repo).assert().success();
}

#[test]
fn check_major_bump_is_ok() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    write_cargo_toml(&repo, "2.0.0");
    commit(&repo, "bump major");

    check_in(&repo).assert().success();
}

#[test]
fn check_skipped_patch_is_illegal_gap() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    write_cargo_toml(&repo, "1.2.5");
    commit(&repo, "bump too far");

    check_in(&repo).assert().failure().code(1).stderr(
        predicates::str::contains("IllegalGap")
            .or(predicates::str::contains("not a legal single-step bump")),
    );
}

#[test]
fn check_regression_is_less_than_latest() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.2");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);

    check_in(&repo)
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("lower than the latest tag"));
}

#[test]
fn check_equal_version_off_tag_is_not_bumped() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    commit(&repo, "dev without bumping"); // must bump on the first commit after a release

    check_in(&repo)
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains(
            "bump the manifest version on the first commit after a release",
        ));
}

#[test]
fn check_bump_at_tagged_commit_is_mismatch() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.4");
    commit(&repo, "initial");
    // The tag must match the manifest at the release commit (clean tree).
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);

    check_in(&repo)
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains(
            "HEAD is the tagged release commit for 1.2.3, but Cargo.toml says 1.2.4",
        ))
        // ... and the error should name the legal next-release versions.
        .stderr(predicates::str::contains("patch: 1.2.4"))
        .stderr(predicates::str::contains("minor: 1.3.0"))
        .stderr(predicates::str::contains("major: 2.0.0"));
}

#[test]
fn check_uncommitted_bump_at_tagged_commit_is_ok() {
    // An uncommitted manifest bump at the tagged release commit is treated as
    // if it were the first commit of the next cycle.
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    // Bump, but don't commit.
    write_cargo_toml(&repo, "1.2.4");

    check_in(&repo)
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "ok: Cargo.toml version 1.2.4 is a legal successor to tag 1.2.3",
        ));
}

#[test]
fn check_staged_bump_at_tagged_commit_is_ok() {
    // Staged (but not committed) bumps count as uncommitted changes too.
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    write_cargo_toml(&repo, "1.3.0");
    run_git(&repo, &["add", "Cargo.toml"]);

    check_in(&repo).assert().success();
}

#[test]
fn check_uncommitted_major_bump_at_tagged_commit_is_ok() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    write_cargo_toml(&repo, "2.0.0");

    check_in(&repo).assert().success();
}

#[test]
fn check_uncommitted_illegal_bump_at_tagged_commit_is_gap() {
    // The uncommitted bump is judged by the post-release rules, so a skipped
    // version is still an IllegalGap.
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    write_cargo_toml(&repo, "1.2.5");

    check_in(&repo)
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("not a legal single-step bump"));
}

#[test]
fn check_uncommitted_regression_at_tagged_commit_is_less_than_latest() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    write_cargo_toml(&repo, "1.2.2");

    check_in(&repo)
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("lower than the latest tag"));
}

#[test]
fn check_dirty_tree_without_version_change_at_tag_is_ok() {
    // Dirt in other files doesn't turn a matching manifest into an error.
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    // Modify a tracked file without touching Cargo.toml.
    std::fs::write(repo.join("f.txt"), "dirty").unwrap();

    check_in(&repo).assert().success();
}

#[test]
fn check_uncommitted_bump_nested_manifest_is_ok() {
    // The HEAD comparison resolves the manifest's repo-relative path, so the
    // uncommitted-bump rule also works from a subdirectory manifest.
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    let manifest = repo.join("sub").join("Cargo.toml");
    write_cargo_toml_at(&manifest, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    write_cargo_toml_at(&manifest, "1.2.4");

    let mut cmd = AssertCommand::cargo_bin("cargo-semvertag").unwrap();
    cmd.args(["check", "--manifest-path", "sub/Cargo.toml"])
        .current_dir(&repo);
    cmd.assert().success();
}

#[test]
fn check_no_tags_is_operational_error() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.0.0");
    commit(&repo, "initial");

    check_in(&repo)
        .assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("no tags"));
}

#[test]
fn check_prerelease_latest_tag_is_error() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.0.0");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.0.0-rc.1", "-m", "rc"]);

    check_in(&repo)
        .assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("prerelease"));
}

#[test]
fn version_flag_prints_version() {
    skip_if_no_git!();
    AssertCommand::cargo_bin("cargo-semvertag")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::starts_with("cargo-semvertag "));
}

#[test]
fn derive_is_default() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    commit(&repo, "dev");

    // Bare invocation (no subcommand) prints the derived version.
    AssertCommand::cargo_bin("cargo-semvertag")
        .unwrap()
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(predicates::str::starts_with("1.2.4-dev.1+g"));
}

#[test]
fn derive_subcommand_prints_version() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    commit(&repo, "dev");

    AssertCommand::cargo_bin("cargo-semvertag")
        .unwrap()
        .arg("derive")
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(predicates::str::starts_with("1.2.4-dev.1+g"));
}

#[test]
fn derive_at_tag_prints_tag_version() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);

    // On the tagged commit, derive() == tag exactly.
    AssertCommand::cargo_bin("cargo-semvertag")
        .unwrap()
        .arg("derive")
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(predicates::str::starts_with("1.2.3"));
}

#[test]
fn derive_honors_minor_bump_in_manifest() {
    // The manifest declares the next release (0.2.0 after v0.1.0); derive
    // targets it instead of blindly bumping patch.
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "0.1.0");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v0.1.0", "-m", "release"]);
    write_cargo_toml(&repo, "0.2.0");
    commit(&repo, "bump minor");

    derive_in(&repo)
        .assert()
        .success()
        .stdout(predicates::str::starts_with("0.2.0-dev.1+g"));
}

#[test]
fn derive_honors_major_bump_in_manifest() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    write_cargo_toml(&repo, "2.0.0");
    commit(&repo, "bump major");

    derive_in(&repo)
        .assert()
        .success()
        .stdout(predicates::str::starts_with("2.0.0-dev.1+g"));
}

#[test]
fn derive_unbumped_manifest_falls_back_to_patch() {
    // Manifest still on the released version: derive falls back to the blind
    // patch bump rather than inventing a version.
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    commit(&repo, "dev");

    derive_in(&repo)
        .assert()
        .success()
        .stdout(predicates::str::starts_with("1.2.4-dev.1+g"));
}

#[test]
fn derive_illegal_manifest_version_falls_back_to_patch() {
    // A skipped version in the manifest is `check`'s business; derive stays
    // monotone and orderable.
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    write_cargo_toml(&repo, "1.2.5");
    commit(&repo, "bump too far");

    derive_in(&repo)
        .assert()
        .success()
        .stdout(predicates::str::starts_with("1.2.4-dev.1+g"));
}

#[test]
fn derive_uncommitted_bump_at_tag_still_prints_tag() {
    // HEAD *is* the tagged release commit, so the tagged version wins even
    // though the working tree already carries the next version (the bump
    // surfaces from the first commit after the tag).
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);
    write_cargo_toml(&repo, "1.3.0"); // bumped, but not committed

    derive_in(&repo)
        .assert()
        .success()
        .stdout(predicates::str::starts_with("1.2.3+dirty"));
}

#[test]
fn derive_with_manifest_path_hints_from_subdirectory() {
    // --manifest-path resolves the hint for a workspace member; the git
    // describe result is repo-wide either way.
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    let manifest = repo.join("sub").join("Cargo.toml");
    write_cargo_toml_at(&manifest, "0.1.0");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v0.1.0", "-m", "release"]);
    write_cargo_toml_at(&manifest, "0.2.0");
    commit(&repo, "bump minor");

    let mut cmd = AssertCommand::cargo_bin("cargo-semvertag").unwrap();
    cmd.args(["derive", "--manifest-path", "sub/Cargo.toml"])
        .current_dir(&repo);
    cmd.assert()
        .success()
        .stdout(predicates::str::starts_with("0.2.0-dev.1+g"));
}
