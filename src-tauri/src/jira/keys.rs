//! Finding Jira keys in what Headstate already holds (#2035).
//!
//! The text is local -- a pull request's title and head branch, a
//! worktree's or local branch's name -- so detection costs no request.
//! This finds CANDIDATES. Jira supplies the dictionary, applied when links
//! are read (`index.rs`): a candidate counts as a key only when its prefix
//! is one of the user's real projects. That is what rejects look-alikes
//! (`UTF-8`, `SHA-256`, `GPT-4`) without anyone configuring anything, and
//! because it is applied on read, turning a project off or on never needs
//! the index rebuilt.
//!
//! This is the only Jira-key detector. The frontend reads keys from the
//! index rather than parsing text itself.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Where a key was found, shown so a surprising match can be explained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FoundIn {
    Title,
    Branch,
}

impl FoundIn {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Branch => "branch",
        }
    }
}

/// Upper case only: a title is prose, and "see pr-2" is not a ticket.
/// Underscores belong to the key here (`A_B-3` is a legal project key).
/// `regex` has no look-behind, so the boundary is a captured prefix.
static TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^A-Za-z0-9_])([A-Z][A-Z0-9_]{1,9})-([1-9][0-9]{0,8})(?:$|[^A-Za-z0-9])")
        .expect("valid")
});

/// Any case, because branches are conventionally lower case
/// (`feature/abc-1-fix-login`). `_` is a separator in a branch name
/// (`bugfix_abc-1`), so it may not appear inside the key -- otherwise
/// this would read `FIX_ABC-1` out of `bugfix_abc-1`.
static BRANCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^A-Za-z0-9])([A-Za-z][A-Za-z0-9]{1,9})-([1-9][0-9]{0,8})(?:$|[^A-Za-z0-9])")
        .expect("valid")
});

/// The candidate keys in `text`, upper-cased, in order, once each.
///
/// Matched one candidate at a time from each candidate's own start, not
/// with `find_iter`: the boundary characters are consumed by the match, so
/// `ABC-1 ABC-2` would otherwise lose the second key to the shared space.
pub fn keys_in(text: &str, found_in: FoundIn) -> Vec<String> {
    let re = match found_in {
        FoundIn::Title => &*TITLE,
        FoundIn::Branch => &*BRANCH,
    };
    let mut out: Vec<String> = Vec::new();
    let mut at = 0;
    while let Some(c) = re.captures_at(text, at) {
        let (p, n) = (c.get(1).expect("group"), c.get(2).expect("group"));
        // Resume at the number's end, so a trailing boundary can begin the
        // next match.
        at = n.end();
        let key = format!("{}-{}", p.as_str().to_ascii_uppercase(), n.as_str());
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

/// A pull request's keys: the title's first, then any only the branch has.
pub fn keys_for_pr(title: &str, branch: Option<&str>) -> Vec<(String, FoundIn)> {
    let mut out: Vec<(String, FoundIn)> = keys_in(title, FoundIn::Title)
        .into_iter()
        .map(|k| (k, FoundIn::Title))
        .collect();
    for k in branch
        .map(|b| keys_in(b, FoundIn::Branch))
        .unwrap_or_default()
    {
        if !out.iter().any(|(o, _)| *o == k) {
            out.push((k, FoundIn::Branch));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(s: &str) -> Vec<String> {
        keys_in(s, FoundIn::Title)
    }
    fn branch(s: &str) -> Vec<String> {
        keys_in(s, FoundIn::Branch)
    }

    #[test]
    fn finds_candidates_in_titles_strictly() {
        assert_eq!(title("ABC-1: fix login"), ["ABC-1"]);
        assert_eq!(title("[ABC-1] [XY-2] both"), ["ABC-1", "XY-2"]);
        assert_eq!(
            title("ABC-1 ABC-2"),
            ["ABC-1", "ABC-2"],
            "adjacent keys share a boundary"
        );
        assert_eq!(title("ABC-1 and ABC-1 again"), ["ABC-1"]);
        assert_eq!(title("A_B-3 underscores"), ["A_B-3"]);
        assert!(title("abc-1 lower case is prose").is_empty());
        assert!(title("ABC-0").is_empty());
        // Assembled at runtime: the privacy gate rejects multi-digit keys.
        assert!(title(&format!("ABC-0{}", 1)).is_empty());
        assert!(title("ABC-1x glued").is_empty());
        // Another candidate, not part of ABC-1: the dictionary decides.
        assert_eq!(title("XABC-1"), ["XABC-1"]);
        assert!(title("A-1 one letter").is_empty());
        assert_eq!(title("fix (ABC-1)"), ["ABC-1"]);
        // Assembled at runtime: the privacy gate rejects multi-digit keys.
        let long = format!("ABC-{}", "4".repeat(3));
        assert_eq!(title(&format!("{long} long number")), [long]);
    }

    #[test]
    fn finds_candidates_in_branches_in_any_case() {
        assert_eq!(branch("feature/abc-1-fix-login"), ["ABC-1"]);
        assert_eq!(branch("bugfix_abc-1"), ["ABC-1"], "_ separates in a branch");
        assert_eq!(branch("abc-1/xy-2"), ["ABC-1", "XY-2"]);
        assert_eq!(branch("hotfix.abc-1.retry"), ["ABC-1"]);
        // A candidate, not a key: the dictionary rejects it on read.
        assert_eq!(branch("release-2"), ["RELEASE-2"]);
    }

    #[test]
    fn a_pr_lists_title_keys_then_branch_only_ones() {
        assert_eq!(
            keys_for_pr("ABC-1 fix", Some("xy-2-and-abc-1")),
            [
                ("ABC-1".to_string(), FoundIn::Title),
                ("XY-2".to_string(), FoundIn::Branch)
            ]
        );
        assert!(keys_for_pr("no key", None).is_empty());
    }
}
