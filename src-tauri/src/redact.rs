//! Patterns for the things that must never reach the diagnostic log.
//!
//! The log exists to be sent. `lib.rs:190-218` describes the workflow it
//! was built for -- "turn the log on, reproduce the problem, send the
//! file" -- and that is the whole reason this module exists. A user who
//! attaches the log to an issue is acting on what Settings told them it
//! contains, so anything Settings does not warn about must not be in it.
//!
//! What is deliberately NOT redacted: the pull-request action lines
//! (`commands.rs:178` and its siblings). Those name a repository and a
//! number on purpose -- README advertises them as an audit trail of
//! every write the app makes, and redacting them would destroy a
//! documented feature to satisfy a sentence of copy. The copy is what
//! was wrong. It now says what these lines hold.
//!
//! This mirrors `src/lib/report.ts`'s `SCRUB` table, which had the right
//! patterns on the wrong side of the boundary: it guarded the issue
//! report body while the log itself was written unredacted.
//! `mirroredConstants.test.ts` asserts the two tables still agree, for
//! the reason #850 records -- five constant pairs whose comments claimed
//! agreement, none of which read the other side, one already drifted.

use std::sync::LazyLock;

use regex::Regex;

/// A token, a home directory, or a filesystem path.
///
/// Order matters, and it is the same order `report.ts:33-39` uses: a
/// token is checked before a path because a token can appear inside one.
///
/// Repository names are absent by design -- see the module comment.
static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        // Every token shape gh can hand out.
        (
            Regex::new(r"\b(gh[pousr]|github_pat)_[A-Za-z0-9_]+").expect("token pattern compiles"),
            "[redacted]",
        ),
        // A home directory carries a username; a checkout path can name
        // a private project. Windows verbatim prefixes (`\\?\C:\`) are
        // matched by the same alternation because `canonicalize` returns
        // them and they carry the same username.
        (
            Regex::new(r#"(/Users/|/home/|[A-Za-z]:\\Users\\|\\\\\?\\[A-Za-z]:\\Users\\)[^\s"']*"#)
                .expect("path pattern compiles"),
            "[path]",
        ),
    ]
});

/// Replace anything that must not be written to the log.
///
/// Returns an owned `String` even when nothing matched. The alternative
/// -- `Cow`, to skip the allocation on the common no-match path -- was
/// measured as not worth the call-site noise: these sites are user
/// actions and disk removals, not a per-request path.
pub fn redact(text: &str) -> String {
    PATTERNS
        .iter()
        .fold(text.to_string(), |acc, (pattern, with)| {
            pattern.replace_all(&acc, *with).into_owned()
        })
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn a_token_is_replaced() {
        assert_eq!(redact("using ghp_abc123DEF456"), "using [redacted]");
        assert_eq!(redact("github_pat_11ABCDE_xyz"), "[redacted]");
    }

    #[test]
    fn a_token_inside_a_path_goes_first() {
        // The ordering guarantee from report.ts:33-39. If the path
        // pattern ran first it would swallow the token into `[path]`,
        // which is safe but loses the more specific signal.
        let out = redact("/Users/sam/.config/ghp_secret123");
        assert!(!out.contains("ghp_secret123"), "token survived: {out}");
    }

    #[test]
    fn a_home_directory_is_replaced() {
        assert_eq!(
            redact("updated checkout /Users/sam/code/x"),
            "updated checkout [path]"
        );
        assert_eq!(redact("/home/sam/code/x"), "[path]");
        assert_eq!(redact(r"C:\Users\sam\code"), "[path]");
    }

    #[test]
    fn a_windows_verbatim_path_is_replaced() {
        // `canonicalize` returns these on Windows, and CLAUDE.md records
        // six Windows-only failures from path handling. A verbatim
        // prefix carries the same username as any other home path.
        assert_eq!(redact(r"\\?\C:\Users\sam\code"), "[path]");
    }

    #[test]
    fn a_repository_name_is_kept() {
        // Deliberate: the audit trail names the repository and number.
        // If this ever starts failing, the module comment is the thing
        // to read before "fixing" it.
        assert_eq!(redact("acme/widgets#42 merged"), "acme/widgets#42 merged");
    }

    #[test]
    fn text_with_nothing_to_redact_is_unchanged() {
        assert_eq!(redact("poll finished in 412ms"), "poll finished in 412ms");
    }
}
