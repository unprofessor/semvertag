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
    let content = format!(
        r#"[package]
name = "test-crate"
version = "{version}"
edition = "2021"
"#
    );
    std::fs::write(repo.join("Cargo.toml"), content).unwrap();
}

fn check_in(repo: &Path) -> AssertCommand {
    let mut cmd = AssertCommand::cargo_bin("cargo-semvertag").unwrap();
    cmd.arg("check");
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
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.3");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);

    check_in(&repo).assert().success().stdout(
        predicates::str::contains("ok: Cargo.toml version 1.2.3 is a legal successor to tag 1.2.3"),
    );
}

#[test]
fn check_patch_bump_is_ok() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.4");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);

    check_in(&repo).assert().success();
}

#[test]
fn check_minor_bump_is_ok() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.3.0");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);

    check_in(&repo).assert().success();
}

#[test]
fn check_major_bump_is_ok() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "2.0.0");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);

    check_in(&repo).assert().success();
}

#[test]
fn check_skipped_patch_is_illegal_gap() {
    skip_if_no_git!();
    let (_dir, repo) = fresh_repo();
    write_cargo_toml(&repo, "1.2.5");
    commit(&repo, "initial");
    run_git(&repo, &["tag", "-a", "v1.2.3", "-m", "release"]);

    check_in(&repo)
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("IllegalGap").or(predicates::str::contains("not a legal single-step bump")));
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
        .arg("version")
        .assert()
        .success()
        .stdout(predicates::str::starts_with("cargo-semvertag "));
}
