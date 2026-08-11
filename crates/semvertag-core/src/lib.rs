//! Turn [`git describe`][git-describe]-style strings into [`semver::Version`]s
//! whose ordering matches intuition.
//!
//! `git describe --tags` produces strings like `v1.0.0-5-g87af40b`. Treated
//! naively as a SemVer string, the `-5-g87af40b` suffix parses as a *prerelease*
//! identifier, which makes a commit 5 past `v1.0.0` sort *before* `v1.0.0` -- the
//! opposite of the intended meaning. This crate rewrites such strings so that
//! increasing commit count always increases precedence:
//!
//! ```text
//! 0.9.9 < 1.0.0-rc.1 < 1.0.0-rc.1.dev.3 < 1.0.0-rc.2 < 1.0.0 < 1.0.1-dev.5 < 1.0.1
//! ```
//!
//! The core logic is pure and performs no I/O: feed it a [`Describe`] (either
//! built by hand or produced by [`parse_describe_string`]) and call [`derive`].
//!
//! # Algorithm
//!
//! Given a `tag` (parsed as a [`semver::Version`]), `commits_since`, `hash`,
//! `dirty`, and optionally a `hint` (the manifest's declared next version, e.g.
//! `Cargo.toml`'s `package.version`):
//!
//! 1. `commits_since == 0` and not dirty -> return `tag` unchanged.
//! 2. `commits_since == 0` and dirty -> `tag` with build metadata `dirty`
//!    (`1.0.0+dirty`).
//! 3. `commits_since > 0` and the hint is a legal single-step successor of
//!    `tag` (patch+1, minor+1 with patch=0, or major+1 with minor=patch=0) ->
//!    target the hint instead of a blind patch bump: `pre = "dev.{commits_since}"`,
//!    `build = "g{hash}"` (+ `.dirty`). This is how the developer's `Cargo.toml`
//!    bump (e.g. `0.2.0` after tag `v0.1.0`) surfaces as `0.2.0-dev.N` rather
//!    than `0.1.1-dev.N`.
//! 4. `commits_since > 0` and `tag` is a plain release (no legal hint) ->
//!    bump patch, set `pre = "dev.{commits_since}"`, `build = "g{hash}"` (+ `.dirty`).
//! 5. `commits_since > 0` and `tag` is itself a prerelease -> keep
//!    major/minor/patch, set `pre = "{tag.pre}.dev.{commits_since}"`,
//!    `build = "g{hash}"` (+ `.dirty`).
//!
//! The `g` prefix on build metadata mirrors `git describe`'s own `g87af40b`
//! convention; [`Describe::hash`] stores the bare short hash without the prefix.
//!
//! Dirty state is always build metadata, never a prerelease: dirty/clean isn't an
//! orderable axis, it's provenance. A dirty build and a clean build at the same
//! commit therefore compare as *equal* under strict SemVer precedence (build
//! metadata is ignored for ordering) -- this is expected, not a bug.
//!
//! # Tag prefix
//!
//! A leading `v` or `V` is stripped from [`Describe::tag`] before parsing. This
//! handles the common `v1.0.0` convention without configuration; exotic prefixes
//! can be stripped by the caller before constructing the [`Describe`].
//!
//! [git-describe]: https://git-scm.com/docs/git-describe

use std::fmt;

use semver::{BuildMetadata, Prerelease, Version};

/// The parsed components of a `git describe` output.
///
/// Usually constructed by [`parse_describe_string`], but may be built directly
/// (e.g. by an adapter that invokes git via a library rather than the shell).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Describe {
    /// The tag `git describe` anchored on, including any prefix (e.g. `v1.0.0`
    /// or `v1.0.0-rc.1`).
    pub tag: String,
    /// Number of commits between the tag and HEAD. `0` if HEAD is exactly the
    /// tag.
    pub commits_since: u64,
    /// The short commit hash, with no `g` prefix.
    pub hash: String,
    /// Whether the working tree was dirty.
    pub dirty: bool,
}

/// Errors that can arise while deriving a version.
#[derive(Debug)]
pub enum DeriveError {
    /// The tag could not be parsed as a SemVer version (after stripping any
    /// `v`/`V` prefix).
    UnparseableTag { tag: String, source: semver::Error },
    /// The raw `git describe` string did not match the expected
    /// `<tag>-<count>-g<hash>[.dirty]` shape.
    MalformedDescribeString { input: String },
    /// No tags were found -- the input looked like a bare `--always` fallback
    /// hash.
    NoTagsFound,
}

impl fmt::Display for DeriveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeriveError::UnparseableTag { tag, .. } => {
                write!(f, "tag {tag:?} is not a valid SemVer version")
            }
            DeriveError::MalformedDescribeString { input } => {
                write!(f, "not a valid `git describe` string: {input:?}")
            }
            DeriveError::NoTagsFound => write!(f, "no tags found in the repository"),
        }
    }
}

impl std::error::Error for DeriveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DeriveError::UnparseableTag { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Derive a comparable [`semver::Version`] from a [`Describe`].
///
/// Shorthand for [`derive_with_hint`] with no hint: derivation is driven
/// purely by the tag, always bumping patch past a plain release. When the
/// manifest's declared `package.version` is available, use [`derive_with_hint`]
/// so a developer-performed minor/major bump is honored.
///
/// See the [crate documentation](crate#algorithm) for the full algorithm.
///
/// # Examples
///
/// ```
/// # use semvertag_core::{Describe, derive};
/// let d = Describe { tag: "v2.3.1".into(), commits_since: 7, hash: "abc1234".into(), dirty: false };
/// assert_eq!(derive(&d).unwrap().to_string(), "2.3.2-dev.7+gabc1234");
/// ```
///
/// A prerelease tag chains its prerelease identifiers:
///
/// ```
/// # use semvertag_core::{Describe, derive};
/// let d = Describe { tag: "v1.0.0-rc.1".into(), commits_since: 3, hash: "87af40b".into(), dirty: false };
/// assert_eq!(derive(&d).unwrap().to_string(), "1.0.0-rc.1.dev.3+g87af40b");
/// ```
///
/// Dirty state is provenance, not ordering, so it lives in build metadata:
///
/// ```
/// # use semvertag_core::{Describe, derive};
/// let d = Describe { tag: "v1.0.0".into(), commits_since: 0, hash: "87af40b".into(), dirty: true };
/// assert_eq!(derive(&d).unwrap().to_string(), "1.0.0+dirty");
/// ```
pub fn derive(describe: &Describe) -> Result<Version, DeriveError> {
    derive_with_hint(describe, None)
}

/// Derive a comparable [`semver::Version`] from a [`Describe`], honoring an
/// optional declared next release version.
///
/// `hint` (typically `Cargo.toml`'s `package.version`) is used only when it is
/// a legal single-step successor of the tag -- as judged by
/// [`is_valid_successor`] with `commits_since = 1`; e.g. `1.3.0` after tag
/// `v1.2.3`. Derivation then targets the hinted version instead of a blind
/// patch bump: tag `v0.1.0`, 3 commits, hint `0.2.0` becomes
/// `0.2.0-dev.3+g87af40b`, not `0.1.1-dev.3+g87af40b`, so the version matches
/// what the manifest will declare at the next release.
///
/// The hint is ignored -- and the tag-based rules of [`derive`] apply -- when:
///
/// - `commits_since == 0`: the commit *is* the tag, so the tagged version
///   wins even if the working tree already carries the next version;
/// - the tag is itself a prerelease (mid-RC, successor rules don't apply);
/// - the hint is not a legal single-step successor (manifest not bumped yet,
///   skipped versions, regressions). An unusable hint silently falls back to
///   the patch bump; flagging it is [`is_valid_successor`] / `check`'s job,
///   not derivation's.
///
/// # Examples
///
/// ```
/// # use semvertag_core::{Describe, derive_with_hint};
/// # use semver::Version;
/// let d = Describe { tag: "v0.1.0".into(), commits_since: 3, hash: "87af40b".into(), dirty: false };
/// let hint = Version::parse("0.2.0").unwrap();
/// assert_eq!(derive_with_hint(&d, Some(&hint)).unwrap().to_string(), "0.2.0-dev.3+g87af40b");
/// ```
///
/// A hint that is not a legal bump (here: the manifest not bumped yet) falls
/// back to the patch-bump rules:
///
/// ```
/// # use semvertag_core::{Describe, derive_with_hint};
/// # use semver::Version;
/// let d = Describe { tag: "v1.2.3".into(), commits_since: 7, hash: "abc1234".into(), dirty: false };
/// let stale = Version::parse("1.2.3").unwrap();
/// assert_eq!(derive_with_hint(&d, Some(&stale)).unwrap().to_string(), "1.2.4-dev.7+gabc1234");
/// ```
pub fn derive_with_hint(
    describe: &Describe,
    hint: Option<&Version>,
) -> Result<Version, DeriveError> {
    let mut version =
        parse_tag_version(&describe.tag).map_err(|source| DeriveError::UnparseableTag {
            tag: describe.tag.clone(),
            source,
        })?;

    if describe.commits_since == 0 {
        if describe.dirty {
            version.build = BuildMetadata::new("dirty").expect("\"dirty\" is valid build metadata");
        }
        return Ok(version);
    }

    // commits_since > 0: target the manifest's declared version when it is a
    // legal single-step successor of the tag (patch+1, minor+1 with patch=0,
    // or major+1 with minor=patch=0). The developer's bump in Cargo.toml thus
    // drives the derivation; anything else (no hint, manifest not bumped yet,
    // illegal gap) falls back to the tag-based rules below. `is_valid_successor`
    // also rejects prerelease hints (candidate.pre must be empty) and returns
    // `LatestIsPrerelease` for prerelease tags, so the hint never overrides
    // the prerelease-chaining rule.
    let mut hinted = false;
    if let Some(h) = hint {
        if is_valid_successor(&version, h, 1).is_ok() {
            version = h.clone();
            hinted = true;
        }
    }

    if version.pre.is_empty() {
        // A hinted base is already the next release version -- only the
        // tag-derived fallback needs the blind patch bump.
        if !hinted {
            version.patch += 1;
        }
        version.pre = Prerelease::new(&format!("dev.{}", describe.commits_since))
            .expect("dev.{u64} is always a valid prerelease");
    } else {
        let new_pre = format!("{}.dev.{}", version.pre, describe.commits_since);
        version.pre = Prerelease::new(&new_pre)
            .expect("chaining .dev.{u64} onto a valid prerelease stays valid");
    }

    let mut build = format!("g{}", describe.hash);
    if describe.dirty {
        build.push_str(".dirty");
    }
    version.build = BuildMetadata::new(&build).expect("g{hash}[.dirty] is valid build metadata");

    Ok(version)
}

/// Parse a raw `git describe --tags --long [--dirty=.dirty]` string into a
/// [`Describe`].
///
/// Parsing proceeds from the right: the last two hyphen-separated groups are the
/// commit count and the `g`-prefixed hash, since tags themselves may contain
/// hyphens (e.g. `v1.0.0-rc.1`).
///
/// A trailing `.dirty` (as emitted by `--dirty=.dirty`) sets [`Describe::dirty`]
/// and is stripped before structural parsing.
///
/// # Examples
///
/// ```
/// # use semvertag_core::{parse_describe_string, Describe};
/// let d = parse_describe_string("v1.0.0-rc.1-3-g87af40b").unwrap();
/// assert_eq!(d, Describe { tag: "v1.0.0-rc.1".into(), commits_since: 3, hash: "87af40b".into(), dirty: false });
/// ```
///
/// ```
/// # use semvertag_core::parse_describe_string;
/// let d = parse_describe_string("v1.0.0-5-g87af40b.dirty").unwrap();
/// assert!(d.dirty);
/// ```
pub fn parse_describe_string(raw: &str) -> Result<Describe, DeriveError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(DeriveError::MalformedDescribeString {
            input: raw.to_string(),
        });
    }

    // Strip the --dirty=.dirty suffix first, before any structural split.
    let (body, dirty) = match raw.strip_suffix(".dirty") {
        Some(b) => (b, true),
        None => (raw, false),
    };

    let parts: Vec<&str> = body.split('-').collect();

    // Need at least <tag>-<count>-g<hash> (three groups). A single group with no
    // hyphens is a bare `--always` fallback hash -> NoTagsFound.
    if parts.len() < 3 {
        if parts.len() == 1 {
            return Err(DeriveError::NoTagsFound);
        }
        return Err(DeriveError::MalformedDescribeString {
            input: raw.to_string(),
        });
    }

    let hash_part = *parts.last().unwrap();
    let count_part = parts[parts.len() - 2];
    let tag = parts[..parts.len() - 2].join("-");

    let hash = match hash_part.strip_prefix('g') {
        Some(h) if !h.is_empty() => h.to_string(),
        _ => {
            return Err(DeriveError::MalformedDescribeString {
                input: raw.to_string(),
            })
        }
    };

    let commits_since: u64 = match count_part.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(DeriveError::MalformedDescribeString {
                input: raw.to_string(),
            })
        }
    };

    Ok(Describe {
        tag,
        commits_since,
        hash,
        dirty,
    })
}

/// Parse a tag string into a [`semver::Version`], stripping an optional leading
/// `v`/`V` prefix.
fn parse_tag_version(tag: &str) -> Result<Version, semver::Error> {
    let stripped = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
    Version::parse(stripped)
}

// ----------------------------------------------------------- sec. 8 ---------

/// Errors from [`is_valid_successor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuccessorError {
    /// `candidate` has lower precedence than `latest_release` (e.g. a
    /// regression, or a prerelease of `latest_release` itself such as
    /// `1.2.3-rc.1` < `1.2.3`).
    LessThanLatest { latest: Version, candidate: Version },
    /// `candidate` equals `latest_release` but HEAD is not the tagged release
    /// commit -- the manifest must be bumped on the first commit after a
    /// release, so no untagged commit ever reports a released version.
    NotBumped {
        latest: Version,
        candidate: Version,
        commits_since: u64,
    },
    /// HEAD is the tagged release commit but `candidate` differs from
    /// `latest_release` -- the manifest must equal the tag there, or the
    /// published artifact would diverge from the tag.
    TagManifestMismatch { latest: Version, candidate: Version },
    /// `candidate` is strictly greater than `latest_release` but is not a
    /// legal single-step bump -- skipped versions, arbitrary jumps, or a bump
    /// that doesn't reset lower fields (e.g. minor bump without resetting
    /// patch to 0).
    IllegalGap { latest: Version, candidate: Version },
    /// `latest_release` is itself a prerelease. Deciding what counts as a legal
    /// next version mid-RC cycle (bump the rc, finish the release, or jump
    /// ahead) is ambiguous and out of scope for v1; see SPEC sec. 8.1.
    LatestIsPrerelease { latest: Version },
}

impl fmt::Display for SuccessorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SuccessorError::LessThanLatest { latest, candidate } => {
                write!(f, "{candidate} is lower than the latest tag {latest}")
            }
            SuccessorError::NotBumped {
                latest,
                candidate,
                commits_since,
            } => {
                let plural = if *commits_since == 1 { "" } else { "s" };
                write!(
                    f,
                    "{candidate} equals the latest tag {latest}, but HEAD is {commits_since} \
                     commit{plural} past it -- bump the manifest version on the first commit \
                     after a release"
                )
            }
            SuccessorError::TagManifestMismatch { latest, candidate } => {
                // The common cause of this error is starting the next release
                // cycle, so list the three legal next-release versions rather
                // than just stating the equality rule.
                let patch =
                    Version::new(latest.major, latest.minor, latest.patch.saturating_add(1));
                let minor = Version::new(latest.major, latest.minor.saturating_add(1), 0);
                let major = Version::new(latest.major.saturating_add(1), 0, 0);
                write!(
                    f,
                    "HEAD is the tagged release commit for {latest}, but Cargo.toml says \
                     {candidate} -- valid Cargo.toml versions for next release:\n  \
                     patch: {patch}\n  minor: {minor}\n  major: {major}"
                )
            }
            SuccessorError::IllegalGap { latest, candidate } => {
                write!(
                    f,
                    "{candidate} is not a legal single-step bump from {latest} \
                     (legal: patch+1, minor+1 with patch=0, or major+1 with minor=patch=0)"
                )
            }
            SuccessorError::LatestIsPrerelease { latest } => {
                write!(f, "latest tag {latest} is a prerelease; successor validation mid-RC is out of scope for v1")
            }
        }
    }
}

impl std::error::Error for SuccessorError {}

/// Check that `candidate` is a legal version for the current commit, relative
/// to `latest_release` (the newest release tag).
///
/// `latest_release` must be a plain release (no prerelease); if it is itself a
/// prerelease, returns [`SuccessorError::LatestIsPrerelease`] rather than
/// guessing -- see SPEC sec. 8.1.
///
/// `commits_since` is the number of commits between HEAD and the tag -- `0`
/// exactly when HEAD is the tagged release commit itself.
///
/// A `candidate` is legal when it is one of:
///
/// - at the tagged release commit (`commits_since == 0`): equal to
///   `latest_release` -- the manifest must match the tag there, or the
///   published artifact would diverge from the tag;
/// - on any other commit: `patch + 1` (minor/major unchanged), `minor + 1`
///   (patch reset to `0`, major unchanged), or `major + 1` (minor and patch
///   reset to `0`).
///
/// This enforces bump-on-first-commit discipline: the first commit after a
/// release must already carry the next version, so no untagged commit ever
/// reports a version equal to a released one, and the manifest stays
/// consistent with what [`derive`] reports (`manifest >= derive(HEAD)`, with
/// equality only at the tag).
///
/// The result is decided by precedence (see SPEC sec. 8.2):
///
/// 1. `candidate < latest_release` -> [`SuccessorError::LessThanLatest`]. This
///    naturally captures prereleases of `latest_release` itself (e.g.
///    `1.2.3-rc.1` < `1.2.3`).
/// 2. `commits_since == 0` and `candidate != latest_release` ->
///    [`SuccessorError::TagManifestMismatch`].
/// 3. `candidate == latest_release` (off the tag) ->
///    [`SuccessorError::NotBumped`].
/// 4. else, if `candidate` is not one of the legal bumps above ->
///    [`SuccessorError::IllegalGap`].
/// 5. else -> `Ok`.
///
/// This crate does **not** inspect commit history or diffs to judge *which*
/// bump the changes warrant -- it only validates that the bump, whatever it is,
/// is a legal single step.
///
/// # Examples
///
/// ```
/// # use semvertag_core::is_valid_successor;
/// # use semver::Version;
/// let latest = Version::new(1, 2, 3);
/// assert!(is_valid_successor(&latest, &latest, 0).is_ok());                // the tagged release commit
/// assert!(is_valid_successor(&latest, &Version::new(1, 2, 4), 1).is_ok()); // patch
/// assert!(is_valid_successor(&latest, &Version::new(1, 3, 0), 1).is_ok()); // minor
/// assert!(is_valid_successor(&latest, &Version::new(2, 0, 0), 1).is_ok()); // major
/// assert!(is_valid_successor(&latest, &latest, 1).is_err());               // not yet bumped
/// assert!(is_valid_successor(&latest, &Version::new(1, 2, 5), 1).is_err()); // skipped patch
/// assert!(is_valid_successor(&latest, &Version::new(1, 2, 2), 1).is_err()); // regression
/// ```
pub fn is_valid_successor(
    latest_release: &Version,
    candidate: &Version,
    commits_since: u64,
) -> Result<(), SuccessorError> {
    if !latest_release.pre.is_empty() {
        return Err(SuccessorError::LatestIsPrerelease {
            latest: latest_release.clone(),
        });
    }

    // 1. Precedence: a candidate strictly below latest is a regression /
    //    lower-precedence prerelease, not an illegal gap. This also catches
    //    "tagged v1.2.3 but the manifest still says 1.2.2" at the tag commit.
    if candidate < latest_release {
        return Err(SuccessorError::LessThanLatest {
            latest: latest_release.clone(),
            candidate: candidate.clone(),
        });
    }

    // 2. At the tagged release commit itself, only equality is valid: the
    //    published version must match the tag. Anything else is a mismatch,
    //    legal bump or not.
    if commits_since == 0 {
        if candidate == latest_release {
            return Ok(());
        }
        return Err(SuccessorError::TagManifestMismatch {
            latest: latest_release.clone(),
            candidate: candidate.clone(),
        });
    }

    // 3. Equality off the tag is "not yet bumped": the first commit after a
    //    release must carry the next version.
    if candidate == latest_release {
        return Err(SuccessorError::NotBumped {
            latest: latest_release.clone(),
            candidate: candidate.clone(),
            commits_since,
        });
    }

    // 4. candidate > latest: must be exactly one of the legal single-step bumps.
    //    Prereleases are never legal successors of a plain release (they sort
    //    below their own release and were caught at step 1, or below latest and
    //    caught there; a prerelease above latest would be e.g. 1.2.3-rc.1 vs
    //    latest 1.2.2 which is handled as IllegalGap here -- it's not a plain
    //    bump).
    let is_legal = candidate.pre.is_empty()
        && (
            // patch + 1
            (candidate.major == latest_release.major
                && candidate.minor == latest_release.minor
                && candidate.patch == latest_release.patch + 1)
            // minor + 1, patch = 0
            || (candidate.major == latest_release.major
                && candidate.minor == latest_release.minor + 1
                && candidate.patch == 0)
            // major + 1, minor = 0, patch = 0
            || (candidate.major == latest_release.major + 1
                && candidate.minor == 0
                && candidate.patch == 0)
        );

    if is_legal {
        Ok(())
    } else {
        Err(SuccessorError::IllegalGap {
            latest: latest_release.clone(),
            candidate: candidate.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn describe(tag: &str, n: u64, hash: &str, dirty: bool) -> Describe {
        Describe {
            tag: tag.into(),
            commits_since: n,
            hash: hash.into(),
            dirty,
        }
    }

    // ---------------------------------------------------------------- 7.1 ----

    #[test]
    fn release_tag_at_head() {
        assert_eq!(
            derive(&describe("1.0.0", 0, "87af40b", false))
                .unwrap()
                .to_string(),
            "1.0.0"
        );
    }

    #[test]
    fn release_tag_at_head_dirty() {
        assert_eq!(
            derive(&describe("1.0.0", 0, "87af40b", true))
                .unwrap()
                .to_string(),
            "1.0.0+dirty"
        );
    }

    #[test]
    fn release_tag_five_commits_past() {
        assert_eq!(
            derive(&describe("1.0.0", 5, "87af40b", false))
                .unwrap()
                .to_string(),
            "1.0.1-dev.5+g87af40b"
        );
    }

    #[test]
    fn release_tag_five_commits_past_dirty() {
        assert_eq!(
            derive(&describe("1.0.0", 5, "87af40b", true))
                .unwrap()
                .to_string(),
            "1.0.1-dev.5+g87af40b.dirty"
        );
    }

    #[test]
    fn prerelease_tag_at_head() {
        assert_eq!(
            derive(&describe("1.0.0-rc.1", 0, "87af40b", false))
                .unwrap()
                .to_string(),
            "1.0.0-rc.1"
        );
    }

    #[test]
    fn prerelease_tag_three_commits_past() {
        assert_eq!(
            derive(&describe("1.0.0-rc.1", 3, "87af40b", false))
                .unwrap()
                .to_string(),
            "1.0.0-rc.1.dev.3+g87af40b"
        );
    }

    #[test]
    fn zero_x_is_not_special_cased() {
        // sec. 5/sec. 11: always bump patch, even for 0.x.
        assert_eq!(
            derive(&describe("0.1.0", 1, "87af40b", false))
                .unwrap()
                .to_string(),
            "0.1.1-dev.1+g87af40b"
        );
    }

    #[test]
    fn numeric_prerelease_compares_numerically() {
        // The case that motivated the whole crate: dev.9 < dev.10, not lexical.
        let a = derive(&describe("1.0.0", 9, "aaa0000", false)).unwrap();
        let b = derive(&describe("1.0.0", 10, "bbb1111", false)).unwrap();
        assert!(a < b, "{a} should sort before {b}");
    }

    // --------------------------------------------- derive_with_hint (§5) ----

    #[test]
    fn hint_minor_bump_targets_manifest_version() {
        // sec. 5 step 3: a legal manifest hint replaces the blind patch bump;
        // 0.x minor bumps are honored like any other (consistent with the 0.x
        // not-special-cased stance: the hint, not the crate, decides the bump).
        let hint = v(0, 2, 0);
        assert_eq!(
            derive_with_hint(&describe("0.1.0", 3, "87af40b", false), Some(&hint))
                .unwrap()
                .to_string(),
            "0.2.0-dev.3+g87af40b"
        );
    }

    #[test]
    fn hint_patch_and_major_bumps() {
        let patch = v(1, 2, 4);
        assert_eq!(
            derive_with_hint(&describe("1.2.3", 1, "87af40b", false), Some(&patch))
                .unwrap()
                .to_string(),
            "1.2.4-dev.1+g87af40b"
        );
        let major = v(2, 0, 0);
        assert_eq!(
            derive_with_hint(&describe("1.2.3", 5, "87af40b", false), Some(&major))
                .unwrap()
                .to_string(),
            "2.0.0-dev.5+g87af40b"
        );
    }

    #[test]
    fn hint_propagates_dirty_into_build_metadata() {
        let hint = v(0, 2, 0);
        assert_eq!(
            derive_with_hint(&describe("0.1.0", 3, "87af40b", true), Some(&hint))
                .unwrap()
                .to_string(),
            "0.2.0-dev.3+g87af40b.dirty"
        );
    }

    #[test]
    fn hint_ignored_at_tag_commit() {
        // The commit *is* the tag: the hinted next version must not leak out
        // of a commit whose version is the released one -- clean or dirty.
        let hint = v(1, 3, 0);
        assert_eq!(
            derive_with_hint(&describe("1.2.3", 0, "87af40b", false), Some(&hint))
                .unwrap()
                .to_string(),
            "1.2.3"
        );
        assert_eq!(
            derive_with_hint(&describe("1.2.3", 0, "87af40b", true), Some(&hint))
                .unwrap()
                .to_string(),
            "1.2.3+dirty"
        );
    }

    #[test]
    fn hint_equals_tag_falls_back_to_patch_bump() {
        // Manifest not bumped yet: equality is not a successor, so derive
        // keeps the blind patch bump.
        let stale = v(1, 2, 3);
        assert_eq!(
            derive_with_hint(&describe("1.2.3", 7, "abc1234", false), Some(&stale))
                .unwrap()
                .to_string(),
            "1.2.4-dev.7+gabc1234"
        );
    }

    #[test]
    fn hint_illegal_gap_falls_back_to_patch_bump() {
        // Skipped versions and other illegal bumps are `check`'s business, not
        // derivation's: the derived version stays monotone and orderable.
        let skipped = v(1, 2, 5);
        assert_eq!(
            derive_with_hint(&describe("1.2.3", 7, "abc1234", false), Some(&skipped))
                .unwrap()
                .to_string(),
            "1.2.4-dev.7+gabc1234"
        );
        let minor_not_reset = v(1, 3, 1);
        assert_eq!(
            derive_with_hint(&describe("1.2.3", 7, "abc1234", false), Some(&minor_not_reset))
                .unwrap()
                .to_string(),
            "1.2.4-dev.7+gabc1234"
        );
        let behind = v(1, 2, 2);
        assert_eq!(
            derive_with_hint(&describe("1.2.3", 7, "abc1234", false), Some(&behind))
                .unwrap()
                .to_string(),
            "1.2.4-dev.7+gabc1234"
        );
    }

    #[test]
    fn hint_ignored_for_prerelease_tags() {
        // Mid-RC, successor rules don't apply: the hint never overrides the
        // prerelease-chaining rule, even when it names the eventual release.
        let release = v(1, 0, 0);
        assert_eq!(
            derive_with_hint(&describe("1.0.0-rc.1", 3, "87af40b", false), Some(&release))
                .unwrap()
                .to_string(),
            "1.0.0-rc.1.dev.3+g87af40b"
        );
    }

    #[test]
    fn derive_is_derive_with_hint_without_hint() {
        let d = describe("1.0.0", 5, "87af40b", true);
        assert_eq!(derive(&d).unwrap(), derive_with_hint(&d, None).unwrap());
    }

    #[test]
    fn huge_commit_count_no_overflow() {
        let v = derive(&describe("1.0.0", 99_999_999, "87af40b", false)).unwrap();
        assert_eq!(v.to_string(), "1.0.1-dev.99999999+g87af40b");
    }

    #[test]
    fn v_prefix_is_stripped() {
        assert_eq!(
            derive(&describe("v1.0.0", 5, "87af40b", false))
                .unwrap()
                .to_string(),
            "1.0.1-dev.5+g87af40b"
        );
    }

    #[test]
    fn unparseable_tag_errors() {
        let err = derive(&describe("release-2021", 0, "87af40b", false)).unwrap_err();
        assert!(matches!(err, DeriveError::UnparseableTag { .. }), "{err:?}");
    }

    // ---------------------------------------------------------------- 7.2 ----

    #[test]
    fn parse_release_tag_at_head() {
        let d = parse_describe_string("v1.0.0-0-g87af40b").unwrap();
        assert_eq!(d, describe("v1.0.0", 0, "87af40b", false));
    }

    #[test]
    fn parse_release_tag_five_commits() {
        let d = parse_describe_string("v1.0.0-5-g87af40b").unwrap();
        assert_eq!(d.commits_since, 5);
    }

    #[test]
    fn parse_prerelease_tag_rightmost_split() {
        // Regression: a naive first-hyphen split would misparse this.
        let d = parse_describe_string("v1.0.0-rc.1-3-g87af40b").unwrap();
        assert_eq!(d.tag, "v1.0.0-rc.1");
        assert_eq!(d.commits_since, 3);
        assert_eq!(d.hash, "87af40b");
    }

    #[test]
    fn parse_dirty_suffix() {
        let d = parse_describe_string("v1.0.0-5-g87af40b.dirty").unwrap();
        assert!(d.dirty);
        assert_eq!(d.commits_since, 5);
    }

    #[test]
    fn parse_bare_hash_is_no_tags_found() {
        let err = parse_describe_string("87af40b").unwrap_err();
        assert!(matches!(err, DeriveError::NoTagsFound), "{err:?}");
    }

    #[test]
    fn parse_empty_is_malformed() {
        let err = parse_describe_string("").unwrap_err();
        assert!(
            matches!(err, DeriveError::MalformedDescribeString { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn parse_garbage_is_malformed() {
        let err = parse_describe_string("not-even-close-to-valid").unwrap_err();
        assert!(
            matches!(err, DeriveError::MalformedDescribeString { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn parse_double_describe_does_not_panic() {
        // Out-of-contract input; we only guarantee no panic, no specific result.
        let _ = parse_describe_string("v1.0.0-rc.1-3-g87af40b-4-gdeadbee");
    }

    // ---------------------------------------------- end-to-end ordering ----

    #[test]
    fn spec_ordering_table() {
        // 0.9.9 < 1.0.0-rc.1 < 1.0.0-rc.1.dev.3 < 1.0.0-rc.2 < 1.0.0
        //   < 1.0.1-dev.5 < 1.0.1
        let v: Vec<Version> = vec![
            "0.9.9".parse().unwrap(),
            derive(&describe("1.0.0-rc.1", 0, "h1", false)).unwrap(),
            derive(&describe("1.0.0-rc.1", 3, "h2", false)).unwrap(),
            "1.0.0-rc.2".parse().unwrap(),
            "1.0.0".parse().unwrap(),
            derive(&describe("1.0.0", 5, "h3", false)).unwrap(),
            "1.0.1".parse().unwrap(),
        ];
        for w in v.windows(2) {
            assert!(w[0] < w[1], "{} should sort before {}", w[0], w[1]);
        }
    }

    // --------------------------------------------------------------- sec. 8.4 ----

    fn v(maj: u64, min: u64, pat: u64) -> Version {
        Version::new(maj, min, pat)
    }

    fn vp(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn successor_equal_on_tag_is_ok() {
        // sec. 8.4: at the tagged release commit, the manifest must equal the tag.
        assert!(is_valid_successor(&v(1, 2, 3), &v(1, 2, 3), 0).is_ok());
    }

    #[test]
    fn successor_equal_off_tag_is_not_bumped() {
        // sec. 8.4: the same version off the tag is "not yet bumped" -- the first
        // commit after a release must carry the next version, however long
        // that takes.
        let err = is_valid_successor(&v(1, 2, 3), &v(1, 2, 3), 1).unwrap_err();
        assert!(matches!(err, SuccessorError::NotBumped { .. }), "{err:?}");
        let err = is_valid_successor(&v(1, 2, 3), &v(1, 2, 3), 42).unwrap_err();
        assert!(matches!(err, SuccessorError::NotBumped { .. }), "{err:?}");
    }

    #[test]
    fn successor_bump_at_tagged_commit_is_mismatch() {
        // sec. 8.4: even a legal bump at the tagged commit itself is a mismatch --
        // at the release commit only equality is valid.
        let err = is_valid_successor(&v(1, 2, 3), &v(1, 2, 4), 0).unwrap_err();
        assert!(
            matches!(err, SuccessorError::TagManifestMismatch { .. }),
            "{err:?}"
        );
        // The same holds for an otherwise-illegal candidate: the tag commit is
        // judged by equality alone.
        let err = is_valid_successor(&v(1, 2, 3), &v(1, 2, 5), 0).unwrap_err();
        assert!(
            matches!(err, SuccessorError::TagManifestMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn tag_manifest_mismatch_message_lists_legal_bumps() {
        // The error should name the concrete legal next-release versions, not
        // just state the equality rule.
        let err = is_valid_successor(&v(0, 2, 1), &v(1, 2, 1), 0).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("HEAD is the tagged release commit for 0.2.1, but Cargo.toml says 1.2.1"),
            "{msg}"
        );
        assert!(
            msg.contains("-- valid Cargo.toml versions for next release:"),
            "{msg}"
        );
        for line in ["patch: 0.2.2", "minor: 0.3.0", "major: 1.0.0"] {
            assert!(msg.contains(line), "missing {line:?} in:\n{msg}");
        }
    }

    #[test]
    fn successor_patch_bump_ok() {
        assert!(is_valid_successor(&v(1, 2, 3), &v(1, 2, 4), 1).is_ok());
    }

    #[test]
    fn successor_minor_bump_ok() {
        assert!(is_valid_successor(&v(1, 2, 3), &v(1, 3, 0), 1).is_ok());
    }

    #[test]
    fn successor_major_bump_ok() {
        assert!(is_valid_successor(&v(1, 2, 3), &v(2, 0, 0), 1).is_ok());
    }

    #[test]
    fn successor_skipped_patch_is_illegal_gap() {
        let err = is_valid_successor(&v(1, 2, 3), &v(1, 2, 5), 1).unwrap_err();
        assert!(matches!(err, SuccessorError::IllegalGap { .. }), "{err:?}");
    }

    #[test]
    fn successor_skipped_minor_is_illegal_gap() {
        let err = is_valid_successor(&v(1, 2, 3), &v(1, 4, 0), 1).unwrap_err();
        assert!(matches!(err, SuccessorError::IllegalGap { .. }), "{err:?}");
    }

    #[test]
    fn successor_minor_bump_must_reset_patch() {
        let err = is_valid_successor(&v(1, 2, 3), &v(1, 3, 1), 1).unwrap_err();
        assert!(matches!(err, SuccessorError::IllegalGap { .. }), "{err:?}");
    }

    #[test]
    fn successor_regression_is_less_than_latest() {
        let err = is_valid_successor(&v(1, 2, 3), &v(1, 2, 2), 1).unwrap_err();
        assert!(
            matches!(err, SuccessorError::LessThanLatest { .. }),
            "{err:?}"
        );
        // ... and the same at the tag commit: "tagged v1.2.3 but forgot to
        // bump the manifest".
        let err = is_valid_successor(&v(1, 2, 3), &v(1, 2, 2), 0).unwrap_err();
        assert!(
            matches!(err, SuccessorError::LessThanLatest { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn successor_same_release_prerelease_is_less_than_latest() {
        let err = is_valid_successor(&v(1, 2, 3), &vp("1.2.3-rc.1"), 1).unwrap_err();
        assert!(
            matches!(err, SuccessorError::LessThanLatest { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn successor_latest_is_prerelease_errors() {
        let err = is_valid_successor(&vp("1.2.3-rc.1"), &v(1, 2, 3), 1).unwrap_err();
        assert!(
            matches!(err, SuccessorError::LatestIsPrerelease { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn successor_zero_x_not_special_cased() {
        // sec. 8.4: 0.x is not specially cased here either, consistent with sec. 5/sec. 11.
        // 0.1.0 -> 0.2.0 is a minor bump (patch reset to 0) -- legal.
        assert!(is_valid_successor(&v(0, 1, 0), &v(0, 2, 0), 1).is_ok());
    }

    #[test]
    fn successor_major_bump_must_reset_minor_and_patch() {
        let err = is_valid_successor(&v(1, 2, 3), &v(2, 0, 1), 1).unwrap_err();
        assert!(matches!(err, SuccessorError::IllegalGap { .. }), "{err:?}");
        let err = is_valid_successor(&v(1, 2, 3), &v(2, 1, 0), 1).unwrap_err();
        assert!(matches!(err, SuccessorError::IllegalGap { .. }), "{err:?}");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    fn tag_strategy() -> impl Strategy<Value = String> {
        // Valid SemVer tags, optionally v-prefixed.
        (any::<bool>(), 0u64..=50, 0u64..=50, 0u64..=50).prop_map(|(vprefix, maj, min, pat)| {
            let base = format!("{maj}.{min}.{pat}");
            if vprefix {
                format!("v{base}")
            } else {
                base
            }
        })
    }

    proptest! {
        // Monotonicity: for a fixed plain-release tag, n1 < n2 => derive(n1) < derive(n2).
        #[test]
        fn monotonic_in_commits(tag in tag_strategy(), n1 in 0u64..1000, n2 in 0u64..1000) {
            let (n1, n2) = if n1 < n2 { (n1, n2) } else { (n2, n1) };
            if n1 == n2 { return Ok(()); }
            let a = derive(&Describe { tag: tag.clone(), commits_since: n1, hash: "a".into(), dirty: false })?;
            let b = derive(&Describe { tag, commits_since: n2, hash: "b".into(), dirty: false })?;
            prop_assert!(a < b, "{} should sort before {}", a, b);
        }

        // Monotonicity across the release boundary: derive(tag, n>0) < tag_next.
        #[test]
        fn below_next_release(maj in 0u64..=20, min in 0u64..=20, pat in 0u64..=20, n in 1u64..1000) {
            let tag = format!("{maj}.{min}.{pat}");
            let derived = derive(&Describe { tag: tag.clone(), commits_since: n, hash: "h".into(), dirty: false })?;
            let next = Version::new(maj, min, pat + 1);
            prop_assert!(derived < next, "{} should sort below {}", derived, next);
        }

        // Identity at zero: derive(tag, 0, clean) == tag.
        #[test]
        fn identity_at_zero(maj in 0u64..=50, min in 0u64..=50, pat in 0u64..=50) {
            let tag = format!("{maj}.{min}.{pat}");
            let derived = derive(&Describe { tag: tag.clone(), commits_since: 0, hash: "h".into(), dirty: false })?;
            let expected = Version::new(maj, min, pat);
            prop_assert_eq!(derived, expected);
        }

        // No panics: arbitrary strings never panic parse_describe_string.
        #[test]
        fn parse_never_panics(raw in ".*") {
            let _ = parse_describe_string(&raw);
        }

        // Round-trip: derive(...).to_string() re-parses via semver.
        #[test]
        fn derive_roundtrips_through_semver(tag in tag_strategy(), n in 0u64..1000, dirty in any::<bool>()) {
            let v = derive(&Describe { tag, commits_since: n, hash: "87af40b".into(), dirty })?;
            let reparsed = semver::Version::parse(&v.to_string())?;
            prop_assert_eq!(v, reparsed);
        }

        // sec. 8.4: any legal bump applied to a random plain-release is accepted at
        // an untagged commit; equality is accepted only on the tag itself.
        #[test]
        fn successor_accepts_legal_bumps(
            maj in 0u64..=50,
            min in 0u64..=50,
            pat in 0u64..=50,
            n in 1u64..1000,
        ) {
            let latest = Version::new(maj, min, pat);
            prop_assert!(is_valid_successor(&latest, &Version::new(maj, min, pat + 1), n).is_ok());
            prop_assert!(is_valid_successor(&latest, &Version::new(maj, min + 1, 0), n).is_ok());
            prop_assert!(is_valid_successor(&latest, &Version::new(maj + 1, 0, 0), n).is_ok());
            prop_assert!(is_valid_successor(&latest, &latest, 0).is_ok());
            prop_assert!(is_valid_successor(&latest, &latest, n).is_err());
        }

        // sec. 8.4: any candidate strictly less than latest is LessThanLatest.
        #[test]
        fn successor_rejects_lower(maj in 1u64..=50, min in 0u64..=50, pat in 0u64..=50) {
            let latest = Version::new(maj, min, pat);
            let lower = if pat > 0 {
                Version::new(maj, min, pat - 1)
            } else if min > 0 {
                Version::new(maj, min - 1, 99)
            } else {
                Version::new(maj - 1, 99, 99)
            };
            prop_assert!(lower < latest);
            let err = is_valid_successor(&latest, &lower, 1).unwrap_err();
            prop_assert_eq!(err, SuccessorError::LessThanLatest { latest: latest.clone(), candidate: lower });
        }
    }
}
