//! Turn [`git describe`][git-describe]-style strings into [`semver::Version`]s
//! whose ordering matches intuition.
//!
//! `git describe --tags` produces strings like `v1.0.0-5-g87af40b`. Treated
//! naively as a SemVer string, the `-5-g87af40b` suffix parses as a *prerelease*
//! identifier, which makes a commit 5 past `v1.0.0` sort *before* `v1.0.0` — the
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
//! Given a `tag` (parsed as a [`semver::Version`]), `commits_since`, `hash`, and
//! `dirty`:
//!
//! 1. `commits_since == 0` and not dirty → return `tag` unchanged.
//! 2. `commits_since == 0` and dirty → `tag` with build metadata `dirty`
//!    (`1.0.0+dirty`).
//! 3. `commits_since > 0` and `tag` is a plain release → bump patch, set
//!    `pre = "dev.{commits_since}"`, `build = "g{hash}"` (+ `.dirty`).
//! 4. `commits_since > 0` and `tag` is itself a prerelease → keep
//!    major/minor/patch, set `pre = "{tag.pre}.dev.{commits_since}"`,
//!    `build = "g{hash}"` (+ `.dirty`).
//!
//! The `g` prefix on build metadata mirrors `git describe`'s own `g87af40b`
//! convention; [`Describe::hash`] stores the bare short hash without the prefix.
//!
//! Dirty state is always build metadata, never a prerelease: dirty/clean isn't an
//! orderable axis, it's provenance. A dirty build and a clean build at the same
//! commit therefore compare as *equal* under strict SemVer precedence (build
//! metadata is ignored for ordering) — this is expected, not a bug.
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
    UnparseableTag {
        tag: String,
        source: semver::Error,
    },
    /// The raw `git describe` string did not match the expected
    /// `<tag>-<count>-g<hash>[.dirty]` shape.
    MalformedDescribeString {
        input: String,
    },
    /// No tags were found — the input looked like a bare `--always` fallback
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
    let mut version = parse_tag_version(&describe.tag)
        .map_err(|source| DeriveError::UnparseableTag {
            tag: describe.tag.clone(),
            source,
        })?;

    if describe.commits_since == 0 {
        if describe.dirty {
            version.build = BuildMetadata::new("dirty").expect("\"dirty\" is valid build metadata");
        }
        return Ok(version);
    }

    // commits_since > 0
    if version.pre.is_empty() {
        version.patch += 1;
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
    // hyphens is a bare `--always` fallback hash → NoTagsFound.
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
        assert_eq!(derive(&describe("1.0.0", 0, "87af40b", false)).unwrap().to_string(), "1.0.0");
    }

    #[test]
    fn release_tag_at_head_dirty() {
        assert_eq!(derive(&describe("1.0.0", 0, "87af40b", true)).unwrap().to_string(), "1.0.0+dirty");
    }

    #[test]
    fn release_tag_five_commits_past() {
        assert_eq!(
            derive(&describe("1.0.0", 5, "87af40b", false)).unwrap().to_string(),
            "1.0.1-dev.5+g87af40b"
        );
    }

    #[test]
    fn release_tag_five_commits_past_dirty() {
        assert_eq!(
            derive(&describe("1.0.0", 5, "87af40b", true)).unwrap().to_string(),
            "1.0.1-dev.5+g87af40b.dirty"
        );
    }

    #[test]
    fn prerelease_tag_at_head() {
        assert_eq!(
            derive(&describe("1.0.0-rc.1", 0, "87af40b", false)).unwrap().to_string(),
            "1.0.0-rc.1"
        );
    }

    #[test]
    fn prerelease_tag_three_commits_past() {
        assert_eq!(
            derive(&describe("1.0.0-rc.1", 3, "87af40b", false)).unwrap().to_string(),
            "1.0.0-rc.1.dev.3+g87af40b"
        );
    }

    #[test]
    fn zero_x_is_not_special_cased() {
        // §5/§11: always bump patch, even for 0.x.
        assert_eq!(
            derive(&describe("0.1.0", 1, "87af40b", false)).unwrap().to_string(),
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

    #[test]
    fn huge_commit_count_no_overflow() {
        let v = derive(&describe("1.0.0", 99_999_999, "87af40b", false)).unwrap();
        assert_eq!(v.to_string(), "1.0.1-dev.99999999+g87af40b");
    }

    #[test]
    fn v_prefix_is_stripped() {
        assert_eq!(
            derive(&describe("v1.0.0", 5, "87af40b", false)).unwrap().to_string(),
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
        assert_eq!(
            d,
            describe("v1.0.0", 0, "87af40b", false)
        );
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
        assert!(matches!(err, DeriveError::MalformedDescribeString { .. }), "{err:?}");
    }

    #[test]
    fn parse_garbage_is_malformed() {
        let err = parse_describe_string("not-even-close-to-valid").unwrap_err();
        assert!(matches!(err, DeriveError::MalformedDescribeString { .. }), "{err:?}");
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
        // Monotonicity: for a fixed plain-release tag, n1 < n2 ⇒ derive(n1) < derive(n2).
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
    }
}
