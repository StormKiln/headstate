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

use super::{Check, CheckRun, Finding, Locator, Report, Severity, Subject};

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
    // A Note is an observation (#1339): it says what was measured and
    // recommends nothing, so it carries no suggestion and asks for no
    // diff. The last line still keeps an agent handed it read-only.
    if f.severity == Severity::Note {
        out.push_str("Observation only: nothing to change. Do not edit any file for this.\n");
        return out;
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
pub(super) fn locator(l: &Locator) -> String {
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
            "Edit the `@` import line named in the evidence, in `{}`, so it names a readable \
             file inside the repository within four hops of the CLAUDE.md, or delete the line. \
             Do not create a file to satisfy it.",
            f.subject.path()
        ),
        Check::Toolchain => super::toolchain::suggestion(f),
        // Two shapes of finding reach this: an Unknown is a transcript
        // that could not be read, and the remedy is not an edit; an
        // Advice is a candidate rule: one line, in the file the sessions
        // load, stating what they had to learn. The counts and the
        // "already written" hits are Notes (#1339) and never get here.
        Check::Transcripts => match (&f.severity, &f.subject) {
            (Severity::Unknown, _) => "No edit. Make the transcript named in the evidence \
                 readable, or leave it: the findings above stand without it, as floors."
                .to_string(),
            (_, Subject::Directory { path }) => format!(
                "If the evidence shows a rule the sessions had to learn, create \
                 `{path}/CLAUDE.md` holding that one line: the command to run, the path to \
                 read first, or the call not to make."
            ),
            (_, subject) => format!(
                "If the evidence shows a rule the sessions had to learn, add one line to \
                 `{}` stating it: the command to run, the path to read first, or the call not \
                 to make.",
                subject.path()
            ),
        },
        Check::Gaps => gaps_suggestion(f),
        Check::Placement => super::placement::suggestion(f),
        Check::Rot => super::rot::suggestion(f),
        Check::Skills => match &f.subject {
            Subject::Skill { path, .. } => format!(
                "Edit `{path}` at the line the evidence names: shorten or respell a value that \
                 exceeds or misspells what the named surface documents, quote a description \
                 YAML would read as more than one value, and move body text a reference file \
                 could hold into one linked from SKILL.md. A cost figure alone needs no edit. \
                 Do not rename the directory."
            ),
            Subject::ClaudeMd { path, .. } => format!(
                "In `{path}`, correct or delete a reference to a skill that was not found, or \
                 add that skill under `.claude/skills/<name>/SKILL.md`; move a procedure into \
                 the skill the evidence names, leaving one line in `{path}` that names the skill."
            ),
            Subject::Directory { path } => format!(
                "Nothing to edit for a count under `{path}`; where the evidence names a \
                 permission error, make that directory readable and run the check again."
            ),
        },
        Check::Shape => super::shape::suggestion(f),
    }
}

/// The gaps producer's suggestion, by the shape of its finding.
///
/// Five shapes, told apart by what `gaps.rs` writes into the finding and
/// pinned there by `the_brief_suggestion_follows_the_finding_shape`: an
/// Unknown opening "session-edit signal unavailable" is a store that
/// could not be queried; any other Unknown is a directory that could
/// not be listed; a sentence opening with a count is a group whose
/// members the evidence lists; "role name only" is a weak candidate;
/// anything else is one strong directory. A helper rather than a nested
/// `match` because the wildcard guard in `invariants.rs` reads every arm
/// between the `Check` match's braces.
fn gaps_suggestion(f: &Finding) -> String {
    let dir = f.subject.path();
    if f.severity == Severity::Unknown && f.finding.starts_with("session-edit signal unavailable") {
        return "No edit. The directories above were judged without the session-edit signal, \
                on their manifests, tests and roles alone; a directory sessions edit heavily \
                may be missing from them. Make Headstate's store readable and run the advice \
                again."
            .to_string();
    }
    if f.severity == Severity::Unknown && f.finding.contains("whether a path-scoped rule covers") {
        return "No edit. Make the rules named in the evidence readable and run the advice \
                again; until then no CLAUDE.md or rule is suggested for the directories listed."
            .to_string();
    }
    if f.severity == Severity::Unknown {
        return format!(
            "Make `{dir}` listable, or add it to the CLAUDE.md walk's skip list if it holds no \
             source, then run the advice again. Nothing under it has been assessed."
        );
    }
    let grouped = f.finding.chars().next().is_some_and(|c| c.is_ascii_digit());
    let body = if grouped {
        format!(
            "Add `{dir}/CLAUDE.md` covering the conventions shared by the members the evidence \
             lists: how they are built and tested, what they are for and which side depends \
             on them, and the rules from the root file that apply here with a different \
             twist. Keep it to what is true only here; a per-member file is for a member that \
             carried a signal of its own."
        )
    } else if f.finding.contains("role name only") {
        format!(
            "Add `{dir}/CLAUDE.md` only if a convention is true only here: what the directory \
             is for, and any rule from the root file that applies here with a different \
             twist. A role name alone is not a gap, so leave it if there is nothing to say."
        )
    } else {
        format!(
            "Add `{dir}/CLAUDE.md` covering: how it is built and tested on its own (the evidence \
             lists what it has); what it is for and which side depends on it; the rules from the \
             root file that apply here with a different twist. Keep it to what is true only here."
        )
    };
    match gaps_rule_offer(f, grouped) {
        Some(offer) => format!("{body} {offer}"),
        None => body,
    }
}

/// #1352: what a repository with `.claude/rules/` is also offered, in
/// placement's words (#1321): a rule whose `paths:` names the directory,
/// with the lazy-load caveat. A probe that failed says the question could
/// not be checked; no probe (no rules directory) adds nothing.
///
/// Read from the finding alone: placement's `.claude/rules` probe in the
/// evidence, and the directory from the sentence (`` `docs/` has … `` or
/// "… under `packages/` …"). No rule is offered for the root: a
/// root-wide glob scopes nothing (`claudemd::rules::Rule::scopes`).
fn gaps_rule_offer(f: &Finding, grouped: bool) -> Option<String> {
    use super::placement::{
        rule_file, rules_exist, rules_probe, rules_unchecked, RULE_LOADS_LAZILY,
    };
    let rel = if grouped {
        f.finding
            .split_once(" under `")
            .and_then(|(_, r)| r.split_once("/`"))
            .map(|(d, _)| d)
    } else {
        f.finding
            .strip_prefix('`')
            .and_then(|r| r.split_once("/`"))
            .map(|(d, _)| d)
    }
    .filter(|d| !d.is_empty())?;
    let (rules, measured) = rules_probe(f)?;
    Some(if rules_exist(measured) {
        format!(
            "Or put the same in {}, instead of the nested file. {RULE_LOADS_LAZILY}",
            rule_file(rules, Some(rel))
        )
    } else {
        rules_unchecked(rules, measured, "this directory's conventions")
    })
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

    // A Note is not advice, so a report of only Notes has none -- but it
    // did find something, so it may not say "nothing found" either.
    let advice = r
        .findings
        .iter()
        .filter(|f| f.severity != Severity::Note)
        .count();
    if advice == 0 && !r.is_partial() {
        let checks = format!("{} check{}", r.ran(), if r.ran() == 1 { "" } else { "s" });
        if r.findings.is_empty() {
            out.push_str(&format!("\n{checks} ran; nothing found.\n"));
        } else {
            out.push_str(&format!(
                "\n{checks} ran; no advice, only the observations above.\n"
            ));
        }
    }

    // Judgement, attributed, and last: none of it has a mechanical test,
    // so none of it is a finding and none of it counts toward the totals
    // above.
    out.push_str(
        "\n## Guidance\n\nAdvice with no mechanical test, so none of it is a finding:\n\n",
    );
    for (text, source) in super::shape::GUIDANCE {
        out.push_str(&format!("- {text} — {source}\n"));
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
            Check::Transcripts => Finding::new(
                Check::Transcripts,
                Severity::Advice,
                Subject::ClaudeMd {
                    path: FILE.into(),
                    scope: Scope::Repo,
                    section: None,
                },
                vec![Evidence {
                    at: Locator::Session {
                        session_id: "s1".into(),
                        record: Some(12),
                    },
                    measured: "first `yarn lint` failed (eslint: command not found), then \
                               `make lint` succeeded"
                        .into(),
                }],
                "`yarn lint` failed and `make lint` followed it in 2 sessions under \
                 `/home/octocat/hello-world/src-tauri`"
                    .into(),
            ),
            Check::Gaps => Finding::new(
                Check::Gaps,
                Severity::Advice,
                Subject::Directory {
                    path: "/home/octocat/hello-world/crates/octocat-core".into(),
                },
                vec![Evidence {
                    at: Locator::File {
                        path: "/home/octocat/hello-world/crates/octocat-core".into(),
                        line: None,
                    },
                    measured:
                        "own `Cargo.toml`, `Cargo.lock`; no CLAUDE.md between it and the root"
                            .into(),
                }],
                "`crates/octocat-core/` has own `Cargo.toml`, `Cargo.lock` and no CLAUDE.md \
                 between it and the root"
                    .into(),
            ),
            Check::Placement => Finding::new(
                Check::Placement,
                Severity::Advice,
                Subject::ClaudeMd {
                    path: FILE.into(),
                    scope: Scope::Repo,
                    section: Some("## Platform".into()),
                },
                vec![
                    Evidence {
                        at: Locator::File {
                            path: FILE.into(),
                            line: Some(38),
                        },
                        measured: "2 of 2 paths resolve under src-tauri/src/".into(),
                    },
                    Evidence {
                        at: Locator::File {
                            path: "/home/octocat/hello-world/src-tauri/src/CLAUDE.md".into(),
                            line: None,
                        },
                        measured: "does not exist".into(),
                    },
                ],
                "Section \"Platform\" (~40 est. tokens) names only paths under src-tauri/src/: \
                 src-tauri/src/a.rs, src-tauri/src/b.rs"
                    .into(),
            ),
            Check::Rot => Finding::new(
                Check::Rot,
                Severity::Problem,
                Subject::ClaudeMd {
                    path: FILE.into(),
                    scope: Scope::Repo,
                    section: Some("## Platform".into()),
                },
                vec![Evidence {
                    at: Locator::File {
                        path: FILE.into(),
                        line: Some(15),
                    },
                    measured: "resolved against `src`, the repository root and a suffix match over 1200 paths in the working tree: 0 matches".into(),
                }],
                "`src/CLAUDE.md:15` names `src/lib/target.ts`, which does not exist in this repository".into(),
            ),
            Check::Skills => Finding::new(
                Check::Skills,
                Severity::Advice,
                Subject::Skill {
                    path: "/home/octocat/hello-world/.claude/skills/verify/SKILL.md".into(),
                    name: "verify".into(),
                },
                vec![Evidence {
                    at: Locator::File {
                        path: "/home/octocat/hello-world/.claude/skills/verify/SKILL.md".into(),
                        line: Some(2),
                    },
                    measured: "`name:` is 70 characters".into(),
                }],
                "skill `verify`: `name` at line 2 is 70 characters; the Agent Skills spec as \
                 the API enforces it allows at most 64"
                    .into(),
            ),
            Check::Shape => Finding::with_rule(
                Check::Shape,
                crate::claudemd::advice::shape::Rule::LineTarget.id(),
                Severity::Advice,
                Subject::ClaudeMd {
                    path: FILE.into(),
                    scope: Scope::Repo,
                    section: None,
                },
                vec![Evidence {
                    at: Locator::File {
                        path: FILE.into(),
                        line: Some(201),
                    },
                    measured: "312 lines; est. 3,100 tokens".into(),
                }],
                "the file runs to 312 lines; Anthropic's target is under 200".into(),
            ),
        }
    }

    /// The guidance is the brief's last section, attributed per line, and
    /// is never counted as a finding: a report with no findings still
    /// says "nothing found" above it.
    #[test]
    fn the_report_ends_with_attributed_guidance_that_is_not_a_finding() {
        let r = report(
            vec![],
            vec![CheckCoverage {
                check: Check::Imports,
                run: CheckRun::Ran { findings: 0 },
            }],
        );
        let at = r.brief.find("\n## Guidance\n").expect("a guidance section");
        assert!(
            r.brief[..at].contains("nothing found"),
            "nothing found comes before the guidance: {}",
            r.brief
        );
        for (text, source) in crate::claudemd::advice::shape::GUIDANCE {
            assert!(r.brief[at..].contains(text), "missing guidance: {text}");
            assert!(
                r.brief[at..].contains(source),
                "unattributed guidance: {text}"
            );
        }
        assert!(r.findings.is_empty(), "guidance is not a finding");
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

    /// A Note's brief says what was measured and recommends nothing
    /// (#1339): no "Suggested change", and no request for a diff.
    #[test]
    fn a_note_brief_recommends_nothing() {
        let mut f = fixture(Check::Transcripts);
        f.severity = Severity::Note;
        let brief = render(&f);
        assert!(brief.starts_with(&format!("## {}\n", f.finding)), "{brief}");
        assert!(
            brief.contains("Evidence: session `s1` record 12 —"),
            "{brief}"
        );
        assert!(!brief.contains("Suggested change"), "{brief}");
        assert!(!brief.contains("Show me the diff"), "{brief}");
        assert!(
            brief.contains("Observation only: nothing to change."),
            "{brief}"
        );
    }

    /// A report whose only findings are Notes does not say "nothing
    /// found" -- it found something, which is the observations -- and
    /// does not imply advice either.
    #[test]
    fn a_report_of_only_notes_says_there_is_nothing_to_change() {
        let mut f = fixture(Check::Transcripts);
        f.severity = Severity::Note;
        f.brief = render(&f);
        let r = report(
            vec![f],
            vec![CheckCoverage {
                check: Check::Transcripts,
                run: CheckRun::Ran { findings: 1 },
            }],
        );
        assert!(!r.brief.contains("nothing found"), "{}", r.brief);
        assert!(
            r.brief
                .contains("1 check ran; no advice, only the observations above."),
            "{}",
            r.brief
        );
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
