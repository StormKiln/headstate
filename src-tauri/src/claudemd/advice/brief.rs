//! The brief: markdown an agent can be handed for one finding, and the
//! combined document for a whole report.
//!
//! Headstate never edits a CLAUDE.md. The brief is the hand-off, in the
//! shape `packages::markdown` already renders for outdated dependencies:
//! what was found, where, what was measured, and what to change -- ending
//! with the `assess.rs` idiom that keeps the agent read-only until the
//! user has seen a diff.
//!
//! # Paths are printed as the finding carries them
//!
//! Absolute, because `ClaudeFile::path` is absolute and the global scope's
//! file lives outside the repository, where a relative path would have
//! nothing to be relative to. An agent given an absolute path can open it
//! from any working directory.
//!
//! # The match on [`Check`] has no wildcard arm
//!
//! `rustc` refuses a missing arm, so the defect a guard can catch is a
//! `_ =>` that would let a new variant compile with a generic suggestion.
//! `invariants.rs` scans this file, with comment lines stripped, and fails
//! on one. Every producer therefore writes its own suggestion.

use super::{Check, CheckRun, Finding, Locator, Report, Subject};

/// The brief for one finding.
///
/// ```markdown
/// ## <one-line finding>
/// Subject: `src-tauri/CLAUDE.md`, section `## Platform`
/// Evidence: `src-tauri/CLAUDE.md:38` — 4 of 4 bullets name paths under `src-tauri/`
/// Suggested change: <what to move, add or delete, naming the target file>
/// Change only the file named above. Show me the diff and let me decide.
/// ```
pub fn render(f: &Finding) -> String {
    let mut out = format!("## {}\n", f.finding);
    out.push_str(&format!("Subject: {}\n", subject(&f.subject)));
    for e in &f.evidence {
        out.push_str(&format!("Evidence: {} — {}\n", locator(&e.at), e.measured));
    }
    out.push_str(&format!("Suggested change: {}\n", suggestion(f)));
    out.push_str("Change only the file named above. Show me the diff and let me decide.\n");
    out
}

fn subject(s: &Subject) -> String {
    match s {
        Subject::ClaudeMd {
            path,
            section: Some(section),
            ..
        } => format!("`{path}`, section `{section}`"),
        Subject::ClaudeMd { path, .. } => format!("`{path}`"),
        Subject::Directory { path } => format!("directory `{path}`"),
        Subject::Skill { path, name } => format!("skill `{name}` (`{path}`)"),
    }
}

/// A locator as the brief prints it.
///
/// A line is printed ONLY when the producer recorded one. `path:0` or a
/// guessed line would send the agent to the wrong place with a confident
/// number, which is the one thing this document must never do.
fn locator(l: &Locator) -> String {
    match l {
        Locator::File {
            path,
            line: Some(line),
        } => format!("`{path}:{line}`"),
        Locator::File { path, line: None } => format!("`{path}`"),
        Locator::Session {
            session_id,
            record: Some(record),
        } => format!("session `{session_id}` record {record}"),
        Locator::Session {
            session_id,
            record: None,
        } => format!("session `{session_id}`"),
    }
}

/// What to change, per check.
///
/// No wildcard arm, and `invariants.rs` checks that: a producer added
/// without a suggestion here must not compile, because a brief whose
/// suggestion is generic is a brief that tells the agent nothing.
fn suggestion(f: &Finding) -> String {
    match f.check {
        Check::Imports => format!(
            "Edit the `@` import line named in the evidence, in `{}`, so it names a file \
             that exists and is readable, or delete the line. Do not create a file to satisfy it.",
            f.subject.path()
        ),
        Check::Toolchain => super::toolchain::suggestion(f),
    }
}

/// The combined document for a report.
///
/// Every finding's brief in the report's own order, then one
/// `_Could not check: {reason}_` line per check that could not run, the
/// shape `packages::markdown::render` uses for a tool that failed. "Nothing
/// found" is printed only when every check ran: an empty list under a
/// failed check would read as good news, which is the worst available
/// answer.
pub fn render_report(r: &Report) -> String {
    let mut out = format!("# CLAUDE.md advice for `{}`\n", r.repo);

    for f in &r.findings {
        out.push('\n');
        out.push_str(&f.brief);
    }

    for c in &r.checks {
        if let CheckRun::Unknown { reason } = &c.run {
            out.push_str(&format!(
                "\n## {}\n\n_Could not check: {reason}_\n",
                c.check.name()
            ));
        }
    }

    if r.findings.is_empty() && !r.is_partial() {
        out.push_str(&format!(
            "\n{} check{} ran; nothing found.\n",
            r.ran(),
            if r.ran() == 1 { "" } else { "s" }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claudemd::advice::{CheckCoverage, Evidence, Severity};
    use crate::claudemd::Scope;

    const FILE: &str = "/home/octocat/hello-world/src-tauri/CLAUDE.md";

    /// A fixture per variant. A `match` with no wildcard, so adding a
    /// variant fails to compile here until it has a fixture.
    fn fixture(check: Check) -> Finding {
        match check {
            Check::Imports => Finding::new(
                Check::Imports,
                Severity::Problem,
                Subject::ClaudeMd {
                    path: FILE.into(),
                    scope: Scope::Repo,
                    section: None,
                },
                vec![Evidence {
                    at: Locator::File {
                        path: FILE.into(),
                        line: None,
                    },
                    measured: "`@./missing.md`: file not found".into(),
                }],
                "`@./missing.md` in the file does not resolve: file not found".into(),
            ),
            Check::Toolchain => Finding::new(
                Check::Toolchain,
                Severity::Advice,
                Subject::ClaudeMd {
                    path: FILE.into(),
                    scope: Scope::Repo,
                    section: None,
                },
                vec![Evidence {
                    at: Locator::File {
                        path: "/home/octocat/hello-world/Makefile".into(),
                        line: Some(118),
                    },
                    measured: "target `build`".into(),
                }],
                "make (Makefile at root) offers `build`, `test`; none of the 3 files read names \
                 `make build`"
                    .into(),
            ),
        }
    }

    /// Every check's brief names the file it is about, so an agent handed
    /// the brief alone knows which file to open.
    #[test]
    fn every_check_variant_renders_a_brief_that_names_its_subject() {
        for check in Check::ALL {
            let f = fixture(*check);
            let brief = render(&f);
            assert!(
                brief.contains(f.subject.path()),
                "{check:?}'s brief does not name its subject:\n{brief}"
            );
            assert!(brief.starts_with(&format!("## {}\n", f.finding)));
            assert!(
                brief.ends_with(
                    "Change only the file named above. Show me the diff and let me decide.\n"
                ),
                "{brief}"
            );
            assert!(brief.contains("Suggested change: "), "{brief}");
            // The brief IS the field: a producer cannot construct a
            // finding whose brief disagrees with it.
            assert_eq!(f.brief, brief);
        }
    }

    /// A locator with no line prints no line. `ImportNode` records none,
    /// and a guessed `:0` would send the agent to a confident wrong place.
    #[test]
    fn the_brief_never_claims_a_line_it_does_not_have() {
        let without = fixture(Check::Imports);
        let brief = render(&without);
        assert!(
            brief.contains(&format!("Evidence: `{FILE}` —")),
            "the path stands alone: {brief}"
        );
        assert!(
            !brief.contains(&format!("{FILE}:")),
            "no line was recorded, so none may be printed: {brief}"
        );

        // And the positive: a line that WAS recorded is printed as
        // `path:line`, the form an editor opens.
        let mut with = without.clone();
        with.evidence[0].at = Locator::File {
            path: FILE.into(),
            line: Some(38),
        };
        assert!(render(&with).contains(&format!("`{FILE}:38`")));
    }

    #[test]
    fn a_section_and_a_skill_subject_are_named_as_such() {
        let mut f = fixture(Check::Imports);
        f.subject = Subject::ClaudeMd {
            path: FILE.into(),
            scope: Scope::Repo,
            section: Some("## Platform".into()),
        };
        assert!(render(&f).contains(&format!("Subject: `{FILE}`, section `## Platform`")));

        f.subject = Subject::Skill {
            path: "/home/octocat/hello-world/.claude/skills/verify/SKILL.md".into(),
            name: "verify".into(),
        };
        assert!(render(&f).contains("Subject: skill `verify` (`/home/octocat"));

        f.subject = Subject::Directory {
            path: "/home/octocat/hello-world/crates".into(),
        };
        assert!(render(&f).contains("Subject: directory `/home/octocat/hello-world/crates`"));

        f.evidence[0].at = Locator::Session {
            session_id: "s1".into(),
            record: Some(9),
        };
        assert!(render(&f).contains("Evidence: session `s1` record 9 —"));
    }

    fn report(findings: Vec<Finding>, checks: Vec<CheckCoverage>) -> Report {
        let mut r = Report {
            repo: "/home/octocat/hello-world".into(),
            findings,
            checks,
            brief: String::new(),
        };
        r.brief = render_report(&r);
        r
    }

    /// A check that could not run is stated, and "nothing found" is not.
    #[test]
    fn the_report_states_an_unknown_check_and_withholds_nothing_found() {
        let r = report(
            vec![],
            vec![CheckCoverage {
                check: Check::Imports,
                run: CheckRun::Unknown {
                    reason: "the repository could not be listed".into(),
                },
            }],
        );
        assert!(r
            .brief
            .contains("_Could not check: the repository could not be listed_"));
        assert!(!r.brief.contains("nothing found"), "{}", r.brief);
    }

    /// Only a run where every check completed may say nothing was found.
    #[test]
    fn the_report_says_nothing_found_only_when_every_check_ran() {
        let r = report(
            vec![],
            vec![CheckCoverage {
                check: Check::Imports,
                run: CheckRun::Ran { findings: 0 },
            }],
        );
        assert!(
            r.brief.contains("1 check ran; nothing found."),
            "{}",
            r.brief
        );
        assert!(!r.brief.contains("Could not check"));
    }

    /// The combined document carries every brief in the report's order.
    #[test]
    fn the_report_concatenates_briefs_in_order() {
        let mut second = fixture(Check::Imports);
        second.finding = "the second finding".into();
        second.brief = render(&second);
        let r = report(
            vec![fixture(Check::Imports), second],
            vec![CheckCoverage {
                check: Check::Imports,
                run: CheckRun::Ran { findings: 2 },
            }],
        );
        let first_at = r.brief.find("## `@./missing.md`").unwrap();
        let second_at = r.brief.find("## the second finding").unwrap();
        assert!(first_at < second_at, "{}", r.brief);
        assert!(r
            .brief
            .starts_with("# CLAUDE.md advice for `/home/octocat/hello-world`\n"));
    }
}
