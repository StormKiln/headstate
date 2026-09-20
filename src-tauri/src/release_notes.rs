//! The URL of the running build's release notes (#1239).
//!
//! # Why this is not one `format!` at the call site
//!
//! `latest_release` can tell the user a newer version exists and
//! cannot tell them what changed -- they get a version number and go
//! looking for the rest themselves. Closing that is a small feature
//! with three distinct ways to be confidently wrong, and each one is
//! easy to write by accident:
//!
//! **The version must be the RUNTIME one.** `latest_release`'s own
//! comment records why: the release workflow stamps the tag into the
//! manifests at build time and never commits them, so the compiled-in
//! `CARGO_PKG_VERSION` reads `0.1.0` in a dev build. A menu item
//! pointing at `v0.1.0`'s notes would be wrong on every developer
//! machine and right in CI, which is the worst way for a bug to
//! behave.
//!
//! **An unreleased build has no notes, and that is not an error.** A
//! dev build's version does not name a published tag. Pointing it at
//! the releases INDEX is the honest answer -- there is a real page
//! listing what exists -- where a tag URL would 404 and read as
//! something broken.
//!
//! So the two cases are [`Target::Tag`] and [`Target::Index`], and
//! they are separate variants rather than one string, because the menu
//! label differs: "Release notes for v6.0.1" is a promise about a
//! specific page and "All releases" is not.
//!
//! Fetching the BODY of the notes is deliberately not done here. That
//! would add the failure mode the issue warns about -- a request that
//! fails must read as "could not reach GitHub", never as "this release
//! shipped without notes" -- and opening a URL has no such mode: the
//! browser reports its own failure in its own words.

/// Where "Release notes" should go for a given build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// This build's version names a published tag.
    Tag(String),
    /// This build does not correspond to a release. The index lists
    /// what does exist, which is the true answer rather than a 404.
    Index,
}

/// The repository the releases belong to.
///
/// Spelled once here and once in `latest_release`. They are asserted
/// to agree by `the_repo_matches_the_update_check`, rather than left
/// for a reader to notice -- a menu pointing at one repository while
/// the update check reads another would be silently wrong in a way no
/// test would otherwise catch.
pub const REPO: &str = "pktstorm/headstate";

impl Target {
    /// The URL to open.
    pub fn url(&self) -> String {
        match self {
            Self::Tag(v) => format!("https://github.com/{REPO}/releases/tag/v{v}"),
            Self::Index => format!("https://github.com/{REPO}/releases"),
        }
    }

    /// The menu item's label.
    ///
    /// Names the version, so the user knows WHICH notes before
    /// clicking -- the tray is the one place the running version is
    /// otherwise invisible.
    pub fn label(&self) -> String {
        match self {
            Self::Tag(v) => format!("Release notes for v{v}"),
            Self::Index => "All releases".to_string(),
        }
    }
}

/// Decide from the runtime version string.
///
/// A version is a release when it is non-empty and is not the
/// placeholder an unstamped build carries. Nothing here validates
/// semver: the only strings that ever reach this are the manifest's,
/// and being wrong costs one wrong link rather than anything harmful
/// -- the same trade `latest_release` makes for the same reason.
pub fn target_for(version: &str) -> Target {
    let v = version.trim();
    if v.is_empty() || v == UNSTAMPED {
        return Target::Index;
    }
    Target::Tag(v.to_string())
}

/// The version a build carries when the release workflow has not
/// stamped it -- the value in `Cargo.toml` as committed.
const UNSTAMPED: &str = "0.1.0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_released_build_points_at_its_own_tag() {
        let t = target_for("6.0.1");
        assert_eq!(t, Target::Tag("6.0.1".into()));
        assert_eq!(
            t.url(),
            "https://github.com/pktstorm/headstate/releases/tag/v6.0.1"
        );
        assert_eq!(t.label(), "Release notes for v6.0.1");
    }

    /// The trap this module exists for. An unstamped dev build must
    /// NOT link to `v0.1.0`, which is not a release anyone published.
    #[test]
    fn an_unstamped_dev_build_points_at_the_index_not_a_bogus_tag() {
        let t = target_for(UNSTAMPED);
        assert_eq!(t, Target::Index);
        assert_eq!(t.url(), "https://github.com/pktstorm/headstate/releases");
        assert!(
            !t.url().contains("0.1.0"),
            "linked to the placeholder version"
        );
        assert_eq!(t.label(), "All releases");
    }

    /// An absent version is the same case, and must not produce
    /// `.../tag/v` -- a URL that looks constructed and 404s.
    #[test]
    fn an_empty_version_points_at_the_index() {
        for v in ["", "   "] {
            assert_eq!(target_for(v), Target::Index, "{v:?}");
        }
        assert!(!target_for("").url().ends_with("/tag/v"));
    }

    /// The `v` prefix is added exactly once. `latest_release` strips
    /// one from the tag it reads, so a version that arrived with it
    /// still must not become `vv6.0.1`.
    #[test]
    fn the_v_prefix_is_not_doubled() {
        assert!(!target_for("6.0.1").url().contains("vv"));
    }

    /// The two spellings of the repository must agree. `latest_release`
    /// builds its own path; if either moved, the menu would open a
    /// different project's notes than the update check reads.
    #[test]
    fn the_repo_matches_the_update_check() {
        let src = include_str!("commands.rs");
        assert!(
            src.contains(&format!("/repos/{REPO}/releases/latest")),
            "commands.rs no longer fetches releases from {REPO}"
        );
    }
}
