//! Which version of `gh`, `glab`, `git`, `docker` and `claude` is installed.
//!
//! Headstate detected whether these EXIST and never which version, so a
//! too-old tool failed at the point of use with that tool's own error
//! rather than an actionable message. The app relies on modern flags --
//! `git worktree`, `--porcelain=v2`, `for-each-ref`, `gh auth token` --
//! and on an old binary those produce parse failures or empty output
//! that callers read as absent data. Absent is not zero, so a version
//! that cannot answer must not look like an answer of none.
//!
//! # Three states, not two
//!
//! `claude/install.rs:341` already models the shape this needs and says
//! why: "`CannotTell` is the state that must not render as
//! `NotInstalled`". The same rule holds one tool over. A version we
//! could not READ is not a version that is too old, and neither is a
//! missing binary -- three remedies, three states.
//!
//! # Reporting, never blocking
//!
//! Nothing here stops the app starting. A too-old `gh` gets a sentence
//! naming the version found and the version needed; the app then
//! degrades exactly as it did before. Refusing to launch over a tool the
//! user may not need would be worse than the confusing 401 this
//! replaces.

use serde::{Deserialize, Serialize};

/// A tool's version, or why we do not have one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ToolVersion {
    /// Found, parsed, and at or above the minimum.
    Ok { found: String },
    /// Found and parsed, but below what the app needs.
    ///
    /// Carries BOTH numbers: "2.30 is older than the 2.41 this needs" is
    /// actionable and "your git is too old" is not.
    TooOld { found: String, required: String },
    /// The binary is not on PATH or in any fallback directory.
    NotFound,
    /// It ran and we could not make sense of the output, or it could not
    /// be run at all.
    ///
    /// DISTINCT from `TooOld` and from `NotFound`, which is the whole
    /// point: the remedy for "install a newer one" and "we could not
    /// tell" are different, and rendering the second as the first sends
    /// a user to upgrade something that may be fine.
    CannotTell { detail: String },
}

/// One tool's answer, with what it is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolReport {
    pub name: &'static str,
    /// The resolved path, when we found one. Useful precisely when the
    /// answer is surprising -- a `gh` from a package manager the user
    /// forgot about.
    pub path: Option<String>,
    pub version: ToolVersion,
    /// What stops working without it, in a sentence.
    ///
    /// The tools are not equal: no `docker` costs one page, no `git`
    /// costs the worktree and branch views entirely.
    pub matters: &'static str,
}

/// The minimum each tool must be, and the flag that needs it.
///
/// One table, with the reason per entry, so a bump is a decision rather
/// than a number someone raised.
const MINIMUMS: &[(&str, &str, &str)] = &[
    // `gh auth token` landed in 2.0 and is the only `gh` call this app
    // makes (`auth.rs:348`).
    (
        "gh",
        "2.0.0",
        "`gh auth token`, the only way this app reads your token",
    ),
    // `%(ahead-behind:)` needs 2.41 (#967). Below it the branch view
    // reports no counts, which used to render as "0 commits not on the
    // default branch" beside a delete checkbox.
    (
        "git",
        "2.41.0",
        "`for-each-ref --format=%(ahead-behind:)`, which the branch view counts with",
    ),
    // `docker system df --format json` -- the Docker page reads nothing
    // else.
    ("docker", "20.10.0", "`docker system df --format json`"),
];

/// Pull a dotted version out of a tool's `--version` line.
///
/// Each of the four prints a different shape:
///
/// ```text
/// gh version 2.101.0 (2026-09-15)
/// git version 2.50.1 (Apple Git-155)
/// Docker version 29.8.1, build 4a63305d74
/// 2.1.277 (Claude Code)
/// ```
///
/// So this finds the FIRST dotted number rather than matching a fixed
/// prefix per tool: the prefixes are what change between releases, and a
/// parser keyed on them would break on a rename it has no reason to
/// care about.
pub fn parse_version(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut dots = 0;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            if bytes[i] == b'.' {
                // A trailing dot is punctuation, not part of the number.
                if i + 1 >= bytes.len() || !bytes[i + 1].is_ascii_digit() {
                    break;
                }
                dots += 1;
            }
            i += 1;
        }
        if dots >= 1 {
            return Some(line[start..i].to_string());
        }
    }
    None
}

/// Compare two dotted versions numerically.
///
/// Numerically, NOT lexically: "2.101.0" is newer than "2.9.0" and a
/// string compare says the opposite. That is the bug this function
/// exists to not have.
///
/// A component that is not a number makes the comparison undecidable,
/// which the caller reports as `CannotTell` rather than guessing.
pub fn at_least(found: &str, required: &str) -> Option<bool> {
    let parse =
        |v: &str| -> Option<Vec<u64>> { v.split('.').map(|p| p.parse::<u64>().ok()).collect() };
    let (f, r) = (parse(found)?, parse(required)?);
    for i in 0..f.len().max(r.len()) {
        // A missing component is zero: "2.41" and "2.41.0" are one
        // version.
        let (a, b) = (
            f.get(i).copied().unwrap_or(0),
            r.get(i).copied().unwrap_or(0),
        );
        if a != b {
            return Some(a > b);
        }
    }
    Some(true)
}

/// Judge one tool's `--version` output against its minimum.
///
/// Split from the subprocess call so the judgement is testable without
/// spawning anything -- the shape `auth.rs`'s own `parse` functions use.
pub fn judge(output: Option<&str>, required: &str) -> ToolVersion {
    let Some(line) = output else {
        return ToolVersion::NotFound;
    };
    let Some(found) = parse_version(line) else {
        return ToolVersion::CannotTell {
            detail: format!("could not read a version out of {line:?}"),
        };
    };
    match at_least(&found, required) {
        Some(true) => ToolVersion::Ok { found },
        Some(false) => ToolVersion::TooOld {
            found,
            required: required.to_string(),
        },
        // A version with a non-numeric component. Reported rather than
        // assumed either way: guessing "probably fine" hides a real
        // problem and guessing "too old" sends the user to upgrade
        // something that may be current.
        None => ToolVersion::CannotTell {
            detail: format!("could not compare {found:?} with {required:?}"),
        },
    }
}

/// Run each tool's `--version` and judge it.
///
/// Spawned through the same resolvers the app uses everywhere else --
/// `auth::find_gh`, `auth::git_program`, `auth::find_claude` -- so this
/// reports on the binary the app would actually run, not on whatever is
/// first on an interactive `PATH`. A GUI-launched `.app` does not
/// inherit that PATH, which is the whole reason those resolvers exist.
pub fn report_all() -> Vec<ToolReport> {
    let run =
        |program: Option<std::path::PathBuf>, args: &[&str]| -> (Option<String>, Option<String>) {
            let Some(p) = program else {
                return (None, None);
            };
            let path = p.to_string_lossy().to_string();
            let out = std::process::Command::new(&p).args(args).output().ok();
            let line = out.and_then(|o| {
                // Some tools print the version on stderr. Both are read
                // rather than assuming stdout, because a tool that answers
                // on the other stream would otherwise read as unparseable.
                let text = if o.stdout.is_empty() {
                    o.stderr
                } else {
                    o.stdout
                };
                String::from_utf8(text)
                    .ok()
                    .and_then(|s| s.lines().next().map(str::to_string))
            });
            (Some(path), line)
        };

    let min = |name: &str| {
        MINIMUMS
            .iter()
            .find(|(t, _, _)| *t == name)
            .map(|(_, m, _)| *m)
            .unwrap_or("0.0.0")
    };

    let (gh_path, gh_line) = run(crate::auth::find_gh(), &["--version"]);
    let (glab_path, glab_line) = run(crate::gitlab::auth::find_glab(), &["--version"]);
    let glab_found = glab_path.is_some();
    let (git_path, git_line) = run(
        Some(crate::auth::git_program().to_path_buf()),
        &["--version"],
    );
    let (claude_path, claude_line) = run(crate::auth::find_claude(), &["--version"]);
    let (docker_path, docker_line) = run(crate::docker::find_docker(), &["--version"]);

    vec![
        ToolReport {
            name: "git",
            path: git_path,
            version: judge(git_line.as_deref(), min("git")),
            matters: "Every worktree and branch view. Without it they show nothing, \
                      and a branch scan reports every branch as unmerged.",
        },
        ToolReport {
            name: "gh",
            path: gh_path,
            version: judge(gh_line.as_deref(), min("gh")),
            matters: "GitHub pull requests only. GitLab and local views remain available.",
        },
        ToolReport {
            name: "glab",
            path: glab_path,
            // 1.119.0 is the version exercised by the probe, not a proven
            // minimum. An older release must not be called unusable solely
            // because it is older than our fixture.
            version: match glab_line.as_deref() {
                Some(line) => match parse_version(line) {
                    Some(found) => ToolVersion::Ok { found },
                    None => ToolVersion::CannotTell {
                        detail: "could not read glab's version".to_string(),
                    },
                },
                None if glab_found => ToolVersion::CannotTell {
                    detail: "glab did not report a version".to_string(),
                },
                None => ToolVersion::NotFound,
            },
            matters: "GitLab.com authentication only. GitHub and local views remain available.",
        },
        ToolReport {
            name: "claude",
            path: claude_path,
            // Claude Code has no minimum: this app reads files it
            // leaves behind rather than calling it, so an old one costs
            // nothing. Reported for completeness, never judged.
            version: match claude_line.as_deref() {
                None => ToolVersion::NotFound,
                Some(l) => match parse_version(l) {
                    Some(found) => ToolVersion::Ok { found },
                    None => ToolVersion::CannotTell {
                        detail: format!("could not read a version out of {l:?}"),
                    },
                },
            },
            matters: "The Claude Code pages. Headstate reads the files it leaves \
                      behind rather than running it, so any version works.",
        },
        ToolReport {
            name: "docker",
            path: docker_path,
            version: judge(docker_line.as_deref(), min("docker")),
            // Docker is frequently OFF, unlike git, and that is normal
            // rather than a problem -- `DockerState` makes the same
            // point. Absent here costs one page, not the app.
            matters: "The Docker page only. Absent is ordinary and costs nothing else.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four real formats, captured from the machines this runs on
    /// rather than invented.
    #[test]
    fn every_real_version_format_parses() {
        for (line, want) in [
            ("gh version 2.101.0 (2026-09-15)", "2.101.0"),
            ("git version 2.50.1 (Apple Git-155)", "2.50.1"),
            ("Docker version 29.8.1, build 4a63305d74", "29.8.1"),
            ("2.1.277 (Claude Code)", "2.1.277"),
        ] {
            assert_eq!(parse_version(line).as_deref(), Some(want), "for {line:?}");
        }
    }

    /// The bug this exists to not have: "2.101.0" is NEWER than "2.9.0",
    /// and a string compare says the opposite.
    #[test]
    fn versions_compare_numerically_not_lexically() {
        assert_eq!(at_least("2.101.0", "2.9.0"), Some(true));
        assert_eq!(at_least("2.9.0", "2.101.0"), Some(false));
        assert!("2.101.0" < "2.9.0", "the lexical compare this avoids");
    }

    /// A missing component is zero, so "2.41" and "2.41.0" are one
    /// version rather than two.
    #[test]
    fn a_short_version_is_padded_with_zeros() {
        assert_eq!(at_least("2.41", "2.41.0"), Some(true));
        assert_eq!(at_least("2.41.0", "2.41"), Some(true));
    }

    #[test]
    fn a_new_enough_tool_is_ok() {
        assert_eq!(
            judge(Some("git version 2.50.1"), "2.41.0"),
            ToolVersion::Ok {
                found: "2.50.1".into()
            }
        );
    }

    /// Both numbers, because "2.30 is older than the 2.41 this needs" is
    /// actionable and "your git is too old" is not.
    #[test]
    fn an_old_tool_names_both_versions() {
        assert_eq!(
            judge(Some("git version 2.30.0"), "2.41.0"),
            ToolVersion::TooOld {
                found: "2.30.0".into(),
                required: "2.41.0".into()
            }
        );
    }

    /// The load-bearing distinction (`install.rs:341`): unreadable is
    /// NOT too old, and neither is missing. Three remedies, three
    /// states.
    #[test]
    fn unreadable_output_is_not_too_old() {
        match judge(Some("some future format with no numbers"), "2.41.0") {
            ToolVersion::CannotTell { .. } => {}
            other => panic!("expected CannotTell, got {other:?}"),
        }
    }

    #[test]
    fn an_absent_tool_is_not_found_rather_than_unreadable() {
        assert_eq!(judge(None, "2.41.0"), ToolVersion::NotFound);
    }

    /// A version we cannot compare is reported rather than assumed
    /// either way: guessing "fine" hides a problem, guessing "too old"
    /// sends the user to upgrade something current.
    #[test]
    fn an_uncomparable_version_is_cannot_tell() {
        assert_eq!(at_least("2.x.1", "2.41.0"), None);
        match judge(Some("git version 2.x.1"), "2.41.0") {
            ToolVersion::CannotTell { .. } => {}
            other => panic!("expected CannotTell, got {other:?}"),
        }
    }

    /// Every tool in the table has a reason for its minimum.
    #[test]
    fn every_minimum_names_what_needs_it() {
        for (tool, min, why) in MINIMUMS {
            assert!(
                !why.is_empty(),
                "{tool}'s minimum {min} has no stated reason"
            );
            assert!(
                at_least(min, min) == Some(true),
                "{tool}'s minimum {min} must parse"
            );
        }
    }
}
