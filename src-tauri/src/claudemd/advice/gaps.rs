//! Directories that have a toolchain, tests or role of their own and no
//! CLAUDE.md between them and the root.
//!
//! # The rule, cited
//!
//! Anthropic's memory docs (`code.claude.com/docs/en/memory`, "How
//! CLAUDE.md files load"): files in the directory hierarchy above the
//! working directory load at launch; "files in subdirectories load on
//! demand when Claude reads files in those directories". A convention
//! that lives only in the root file is paid for by every session; one in
//! `crates/foo/CLAUDE.md` is loaded exactly when a session touches
//! `crates/foo/`. A directory with its own build, tests or release
//! process and no CLAUDE.md anywhere between it and the root has no place
//! for that knowledge to live where it costs nothing until needed.
//!
//! # What is measured, and where
//!
//! Every fact comes from [`DirFacts`], which `claudemd::scan_repo` records
//! inside the loop that finds the CLAUDE.md files. There is no second
//! walk: #1236 halved that walk's cost once, and this producer adds one
//! file read per `Cargo.toml` or `package.json` it passes and nothing
//! else. `packages::detect::ecosystems` was the intended source of the
//! manifest signal and was rejected after reading and measuring it: per
//! directory it lists the directory twice more (`has_project_file`,
//! `has_xcode_spm`) and walks three levels under it for Terraform locks.
//! Measured on this checkout (120 directories): the walk with facts
//! recorded takes 9.7 ms, and calling `ecosystems()` once per listed
//! directory adds 12.1 ms on top -- more than the whole walk, to learn
//! ten ecosystem labels that ten entry names already gave. The
//! manifest's NAME is what a finding prints, and the walk has the name in
//! hand.
//!
//! # Candidate, strong, weak
//!
//! A candidate is a directory, not the root, with at least one signal and
//! no CLAUDE.md on the path from it up to (excluding) the root. The root
//! file covers nothing below it by itself, or every repository with one
//! root file would pass. A directory the walk could not list, or anything
//! under one, is never a candidate: it is an Unknown finding instead.
//!
//! Strong signals: a manifest of its own; membership in a workspace;
//! [`TEST_FILE_THRESHOLD`] or more `*.test.*`/`*.spec.*` files directly
//! in it; or being one of the directories sessions edit heavily, when the
//! transcripts producer supplies that list. Weak signals: a well-known
//! role name (`docs`, `scripts`, `crates/*`, …) or a `tests/`,
//! `__tests__/` or `spec/` entry. A directory with only weak signals is a
//! weak candidate, rendered as a suggestion: its finding says "role name
//! only", and its brief says to add a file only if there is something
//! true only there.
//!
//! # The test-file threshold is 10
//!
//! Measured on this checkout, direct `*.test.*` counts per directory:
//! `src/components` 105, `src/lib` 58, `src/api` 33, `scripts` 14,
//! `src/components/stats` 10, `src` 6, `src/store` 5, `src/fixtures` 1.
//! Below ten, every directory is one whose tests are incidental to a
//! parent that a session works in for the parent's sake; from ten up,
//! each is a place a session works in for its own sake, and
//! `src/components/stats` at exactly ten is a feature directory with a
//! suite of its own. A count below the threshold is not a signal at all,
//! rather than a weak one: one test file next to a fixture is not a
//! reason to suggest a file.
//!
//! # Suppressed and downgraded, by the nearest ancestor file
//!
//! For a candidate the nearest ancestor CLAUDE.md is the root's, by
//! construction. If it has a heading naming the directory
//! (`text::sections_naming`), the finding is suppressed: the parent has
//! given it a section, and whether that section should move down is the
//! placement producer's question, not this one's. If it has a heading
//! naming an ancestor of the directory, or a path reference to the
//! directory or something under it (`text::paths_naming`), a strong
//! candidate is downgraded to weak with the line as evidence, and a weak
//! one is dropped: a role name the parent already points at is nothing to
//! say. Both helpers live in `text.rs` and are shared with the placement
//! producer.
//!
//! # Grouping
//!
//! Findings are keyed by parent and signal set, so twelve workspace
//! members under `packages/` are one finding with the member list in its
//! evidence, not twelve. Three or more siblings group; a member with a
//! distinct signal set (one that also holds two hundred tests) stays its
//! own row. Measured on this checkout: 120 directories listed, six
//! CLAUDE.md files, ten manifests, two candidates
//! (`crates/headstate-stepup/` strong, `docs/` weak). That ratio is the
//! target.
//!
//! # The edited signal
//!
//! [`gaps`] takes `edited: &[PathBuf]`, the directories sessions edit
//! heavily. The transcripts producer computes it as `edited_dirs` from
//! its `cwd`-bearing records and tool-use paths; this producer never
//! reads a transcript itself. The [`Producer`] impl passes an empty
//! list today, and integration wires the two together. A path in the
//! list is matched exactly against the directory, absolute or relative
//! to the root.
//!
//! # Deliberately out of scope
//!
//! `.claude/rules/` and `AGENTS.md` both change what "covered" means --
//! path-scoped rules load on read, and `AGENTS.md` is read only where
//! none of the three CLAUDE.md files exists -- and neither is consulted
//! here. A follow-up. Go's `_test.go` and Python's `test_*.py` are not
//! counted as test files: the pattern is the design's `*.test.*`, and a
//! wider one is a measured change, not a default.

use super::{Check, Context, Evidence, Finding, Locator, Producer, Severity, Subject};
use crate::claudemd::{text, DirFacts, Scan};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Direct `*.test.*`/`*.spec.*` files a directory needs for that alone
/// to be a strong signal. See the module docs for how it was measured.
pub const TEST_FILE_THRESHOLD: usize = 10;

/// Siblings that share a parent and a signal set collapse into one
/// finding from this many up.
const GROUP_AT: usize = 3;

/// What siblings must share to group: their parent, their signal set and
/// what the root file said about them.
type GroupKey = (String, Vec<String>, Option<Mention>);

pub struct Gaps;

impl Producer for Gaps {
    fn check(&self) -> Check {
        Check::Gaps
    }

    fn run(&self, cx: &Context) -> Result<Vec<Finding>, String> {
        // Empty until integration hands over the transcripts producer's
        // `edited_dirs`; see the module docs.
        gaps(&cx.scan.repo, &[])
    }
}

/// How sure a candidate is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    /// A manifest, a workspace membership, a test suite or the edited
    /// signal.
    Strong,
    /// A role name or a `tests/` entry only, or a strong candidate the
    /// root file already refers to.
    Weak,
}

/// Something the root CLAUDE.md says about a candidate that lowers it.
/// Ordered so it can sit in a grouping key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mention {
    pub file: PathBuf,
    pub line: usize,
    /// "has a `## packages` section for its parent" or
    /// "references `docs/mobile-*.md`".
    pub what: String,
}

/// One candidate directory, before grouping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// Absolute.
    pub dir: PathBuf,
    /// Relative to the root, with `/` separators, no trailing slash.
    pub rel: String,
    pub strength: Strength,
    /// The signals as the finding prints them: "own `Cargo.toml`,
    /// `Cargo.lock`", "12 test files", "role name `docs`".
    pub signals: Vec<String>,
    /// True when every signal is weak: the finding then says "role name
    /// only".
    pub role_only: bool,
    /// Set when the root file's mention downgraded a strong candidate.
    pub downgraded_by: Option<Mention>,
}

/// Every candidate, in path order, before grouping.
///
/// `Err` when the root itself could not be listed or its CLAUDE.md
/// exists and could not be read: without the root file, whether any
/// candidate is suppressed is unknowable, and a list that might be wrong
/// in every row is withheld rather than qualified.
pub fn candidates(scan: &Scan, edited: &[PathBuf]) -> Result<Vec<Gap>, String> {
    let root = scan
        .dir_facts
        .iter()
        .find(|f| f.rel.as_os_str().is_empty())
        .ok_or_else(|| "the scan recorded no directories, so nothing was assessed".to_string())?;
    if let Some(why) = &root.unreadable {
        return Err(format!("the repository root could not be listed: {why}"));
    }
    let root_text = if root.has_claude_md {
        let path = root.path.join("CLAUDE.md");
        Some(std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "{} could not be read, so whether it names any directory is unknown: {e}",
                path.display()
            )
        })?)
    } else {
        None
    };

    let covered: Vec<&Path> = scan
        .dir_facts
        .iter()
        .filter(|f| f.has_claude_md && !f.rel.as_os_str().is_empty())
        .map(|f| f.rel.as_path())
        .collect();
    let unreadable: Vec<&Path> = scan
        .dir_facts
        .iter()
        .filter(|f| f.unreadable.is_some())
        .map(|f| f.rel.as_path())
        .collect();
    let edited: Vec<PathBuf> = edited
        .iter()
        .map(|e| {
            if e.is_absolute() {
                e.clone()
            } else {
                root.path.join(e)
            }
        })
        .collect();

    let mut out = Vec::new();
    for f in &scan.dir_facts {
        if f.rel.as_os_str().is_empty() || f.unreadable.is_some() {
            continue;
        }
        // Unknown is not missing: nothing under a directory the walk
        // could not list is assessed, and the Unknown finding for that
        // directory says so.
        if unreadable.iter().any(|u| f.rel.starts_with(u)) {
            continue;
        }
        // The path from the directory up to, excluding, the root. A
        // CLAUDE.md anywhere on it -- the directory's own included --
        // covers it.
        if f.rel
            .ancestors()
            .filter(|a| !a.as_os_str().is_empty())
            .any(|a| covered.contains(&a))
        {
            continue;
        }

        let (signals, strength) = signals_of(f, &root.path, &edited);
        let Some(strength) = strength else {
            continue;
        };
        let rel = f.rel.to_string_lossy().replace('\\', "/");
        let root_file = root.path.join("CLAUDE.md");

        let mut strength = strength;
        let mut downgraded_by = None;
        if let Some(text) = &root_text {
            if !text::sections_naming(text, &rel).is_empty() {
                // A section of its own: the parent has given it a place.
                continue;
            }
            let mention = f
                .rel
                .ancestors()
                .skip(1)
                .filter(|a| !a.as_os_str().is_empty())
                .find_map(|a| {
                    let a = a.to_string_lossy().replace('\\', "/");
                    text::sections_naming(text, &a)
                        .into_iter()
                        .next()
                        .map(|(heading, line)| Mention {
                            file: root_file.clone(),
                            line,
                            what: format!("has a section `{heading}` for its parent `{a}/`"),
                        })
                })
                .or_else(|| {
                    text::paths_naming(text, &rel)
                        .into_iter()
                        .next()
                        .map(|(word, line)| Mention {
                            file: root_file.clone(),
                            line,
                            what: format!("references `{word}`"),
                        })
                });
            if let Some(m) = mention {
                match strength {
                    // A role name the parent already points at is
                    // nothing to say.
                    Strength::Weak => continue,
                    Strength::Strong => {
                        strength = Strength::Weak;
                        downgraded_by = Some(m);
                    }
                }
            }
        }

        out.push(Gap {
            dir: f.path.clone(),
            rel,
            strength,
            role_only: strength == Strength::Weak && downgraded_by.is_none(),
            signals,
            downgraded_by,
        });
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// A directory's signals as the finding prints them, and the strength
/// they add up to. `None` when there is no signal at all.
fn signals_of(f: &DirFacts, root: &Path, edited: &[PathBuf]) -> (Vec<String>, Option<Strength>) {
    let mut strong = Vec::new();
    let mut weak = Vec::new();

    if !f.manifests.is_empty() {
        let files: Vec<String> = f
            .manifests
            .iter()
            .chain(f.markers.iter())
            .map(|m| format!("`{m}`"))
            .collect();
        strong.push(format!("own {}", files.join(", ")));
    }
    if let Some(manifest) = &f.workspace_of {
        let shown = manifest
            .strip_prefix(root)
            .unwrap_or(manifest)
            .to_string_lossy()
            .replace('\\', "/");
        strong.push(format!("workspace membership in `{shown}`"));
    }
    if f.test_files >= TEST_FILE_THRESHOLD {
        strong.push(format!("{} test files", f.test_files));
    }
    if edited.iter().any(|e| e == &f.path) {
        strong.push("session edits".to_string());
    }
    if let Some(role) = &f.role {
        weak.push(format!("role name `{role}`"));
    }
    if f.test_dirs > 0 {
        weak.push("a tests directory".to_string());
    }

    if !strong.is_empty() {
        strong.extend(weak);
        (strong, Some(Strength::Strong))
    } else if !weak.is_empty() {
        (weak, Some(Strength::Weak))
    } else {
        (Vec::new(), None)
    }
}

/// Every finding: candidates grouped by parent and signal set, then one
/// Unknown per directory the walk could not list and per manifest whose
/// members could not be read.
///
/// `edited` is the transcripts producer's `edited_dirs`; see the module
/// docs.
pub fn gaps(scan: &Scan, edited: &[PathBuf]) -> Result<Vec<Finding>, String> {
    let found = candidates(scan, edited)?;
    let mut out = Vec::new();

    // Keyed by parent, signal set and mention, in first-seen order of the
    // sorted candidates so the output order is the path order.
    let mut groups: BTreeMap<GroupKey, Vec<&Gap>> = BTreeMap::new();
    for g in &found {
        let parent = match g.rel.rsplit_once('/') {
            Some((p, _)) => p.to_string(),
            None => String::new(),
        };
        groups
            .entry((parent, g.signals.clone(), g.downgraded_by.clone()))
            .or_default()
            .push(g);
    }
    let mut rows: Vec<Finding> = Vec::new();
    for ((parent, signals, mention), members) in groups {
        if members.len() >= GROUP_AT {
            rows.push(grouped(&parent, &signals, mention.as_ref(), &members));
        } else {
            rows.extend(members.into_iter().map(single));
        }
    }
    // A group's row sorts where its parent does, beside the singles.
    rows.sort_by(|a, b| a.subject.path().cmp(b.subject.path()));
    out.extend(rows);

    for f in scan.dir_facts.iter().filter(|f| f.unreadable.is_some()) {
        let rel = rel_display(&f.rel);
        out.push(Finding::new(
            Check::Gaps,
            Severity::Unknown,
            Subject::Directory {
                path: f.path.to_string_lossy().to_string(),
            },
            vec![Evidence {
                at: Locator::File {
                    path: f.path.to_string_lossy().to_string(),
                    line: None,
                },
                measured: format!(
                    "could not be listed: {}",
                    f.unreadable.as_deref().unwrap_or_default()
                ),
            }],
            format!(
                "`{rel}` could not be listed, so whether it or anything under it needs a \
                 CLAUDE.md is unknown"
            ),
        ));
    }
    for f in scan
        .dir_facts
        .iter()
        .filter(|f| f.workspace_unknown.is_some())
    {
        let rel = rel_display(&f.rel);
        out.push(Finding::new(
            Check::Gaps,
            Severity::Unknown,
            Subject::Directory {
                path: f.path.to_string_lossy().to_string(),
            },
            vec![Evidence {
                at: Locator::File {
                    path: f.path.to_string_lossy().to_string(),
                    line: None,
                },
                measured: f.workspace_unknown.clone().unwrap_or_default(),
            }],
            format!(
                "`{rel}` declares workspace members that could not be read, so which \
                 directories are members is unknown"
            ),
        ));
    }
    Ok(out)
}

/// `docs/` for a directory, `the root` for the root.
fn rel_display(rel: &Path) -> String {
    if rel.as_os_str().is_empty() {
        "the root".to_string()
    } else {
        format!("{}/", rel.to_string_lossy().replace('\\', "/"))
    }
}

fn single(g: &Gap) -> Finding {
    let dir = g.dir.to_string_lossy().to_string();
    // "role name only (`docs`)", not "role name only (role name `docs`)".
    let joined = if g.role_only {
        g.signals
            .iter()
            .map(|s| s.strip_prefix("role name ").unwrap_or(s))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        g.signals.join(", ")
    };
    let mut evidence = vec![Evidence {
        at: Locator::File {
            path: dir.clone(),
            line: None,
        },
        measured: format!(
            "{}; no CLAUDE.md between it and the root",
            g.signals.join("; ")
        ),
    }];
    let mut sentence = if g.role_only {
        format!(
            "`{}/` has a role name only ({joined}) and no CLAUDE.md between it and the root",
            g.rel
        )
    } else {
        format!(
            "`{}/` has {joined} and no CLAUDE.md between it and the root",
            g.rel
        )
    };
    if let Some(m) = &g.downgraded_by {
        sentence.push_str(&format!(", though the root CLAUDE.md {}", m.what));
        evidence.push(mention_evidence(m));
    }
    Finding::new(
        Check::Gaps,
        Severity::Advice,
        Subject::Directory { path: dir },
        evidence,
        sentence,
    )
}

fn grouped(
    parent: &str,
    signals: &[String],
    mention: Option<&Mention>,
    members: &[&Gap],
) -> Finding {
    // The parent's absolute path, from the member's: the parent may be
    // the root, which has no candidate of its own.
    let parent_dir = members[0]
        .dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| members[0].dir.clone());
    let names: Vec<String> = members
        .iter()
        .map(|m| {
            format!(
                "`{}`",
                m.rel.rsplit_once('/').map_or(m.rel.as_str(), |(_, n)| n)
            )
        })
        .collect();
    let joined = signals.join(", ");
    let shown = if parent.is_empty() {
        "the root".to_string()
    } else {
        format!("`{parent}/`")
    };
    let mut evidence = vec![Evidence {
        at: Locator::File {
            path: parent_dir.to_string_lossy().to_string(),
            line: None,
        },
        measured: format!(
            "{} members: {}; each: {}; no CLAUDE.md between them and the root",
            members.len(),
            names.join(", "),
            signals.join("; ")
        ),
    }];
    let mut sentence = format!(
        "{} directories under {shown} each have {joined} and no CLAUDE.md between them and \
         the root",
        members.len()
    );
    if let Some(m) = mention {
        sentence.push_str(&format!(", though the root CLAUDE.md {}", m.what));
        evidence.push(mention_evidence(m));
    }
    Finding::new(
        Check::Gaps,
        Severity::Advice,
        Subject::Directory {
            path: parent_dir.to_string_lossy().to_string(),
        },
        evidence,
        sentence,
    )
}

fn mention_evidence(m: &Mention) -> Evidence {
    Evidence {
        at: Locator::File {
            path: m.file.to_string_lossy().to_string(),
            line: Some(m.line as u32),
        },
        measured: format!("the root CLAUDE.md {}", m.what),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claudemd::advice::{CheckRun, Report};
    use crate::claudemd::{scan_effective_opt, scan_repo};
    use std::fs;

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// The sub-issue's fixture: a workspaces root with three members, a
    /// `tests/` directory holding ten test files, a `docs/` with a role
    /// only, and (on unix) one directory that cannot be listed.
    fn fixture() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        write(
            &r.join("CLAUDE.md"),
            "# hello-world\n\nRules for everyone.\n",
        );
        write(
            &r.join("package.json"),
            r#"{"name":"hello-world","workspaces":["packages/*"]}"#,
        );
        for m in ["octocat-a", "octocat-b", "octocat-c"] {
            write(
                &r.join("packages").join(m).join("package.json"),
                &format!(r#"{{"name":"{m}"}}"#),
            );
        }
        for i in 0..10 {
            write(&r.join("tests").join(format!("t{i}.test.ts")), "test\n");
        }
        write(&r.join("docs").join("README.md"), "docs\n");
        t
    }

    /// Wall one directory, run `f`, and restore the wall before any
    /// assertion so a failure does not leave an unremovable directory
    /// behind for the temp dir's drop.
    #[cfg(unix)]
    fn with_walled<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).unwrap();
        let out = f();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        out
    }

    fn run_over(repo: &Path) -> Report {
        let scan = scan_effective_opt(repo, None);
        let cx = Context {
            repo,
            home: None,
            scan: &scan,
            definitions: None,
            conn: None,
        };
        super::super::run(&cx)
    }

    /// This check's coverage row. Every producer runs, so the row is
    /// found by check rather than by position.
    fn coverage(report: &Report) -> &CheckRun {
        &report
            .checks
            .iter()
            .find(|c| c.check == Check::Gaps)
            .expect("a coverage row for gaps")
            .run
    }

    fn gap_findings(report: &Report) -> Vec<&Finding> {
        report
            .findings
            .iter()
            .filter(|f| f.check == Check::Gaps)
            .collect()
    }

    fn by_subject<'a>(findings: &[&'a Finding], suffix: &str) -> Option<&'a Finding> {
        findings
            .iter()
            .copied()
            .find(|f| Path::new(f.subject.path()).ends_with(suffix))
    }

    /// The founding case. Three members become ONE grouped finding
    /// naming all three; `tests/` is strong on its test count; `docs/`
    /// is weak on its role; nothing is emitted for a member on its own.
    ///
    /// Sabotage-proven: with `GROUP_AT` raised past three the group
    /// splits into three rows and the "no member row" assertion fails;
    /// with the threshold comparison dropped, `tests/` stops being
    /// strong.
    #[test]
    fn the_fixture_yields_one_group_one_strong_and_one_weak() {
        let t = fixture();
        let report = run_over(t.path());
        let found = gap_findings(&report);

        let group = by_subject(&found, "packages").expect("one grouped finding for packages/");
        assert!(
            group
                .finding
                .starts_with("3 directories under `packages/` each have"),
            "{}",
            group.finding
        );
        assert!(group
            .finding
            .contains("workspace membership in `package.json`"));
        for m in ["`octocat-a`", "`octocat-b`", "`octocat-c`"] {
            assert!(
                group.evidence[0].measured.contains(m),
                "{}",
                group.evidence[0].measured
            );
        }
        assert!(
            by_subject(&found, "octocat-a").is_none(),
            "a member must not also be its own row: {found:?}"
        );

        let tests = by_subject(&found, "tests").expect("a strong finding for tests/");
        assert!(tests.finding.contains("10 test files"), "{}", tests.finding);
        assert!(!tests.finding.contains("role name only"));
        assert_eq!(tests.severity, Severity::Advice);

        let docs = by_subject(&found, "docs").expect("a weak finding for docs/");
        assert!(
            docs.finding.contains("role name only (`docs`)"),
            "{}",
            docs.finding
        );
        assert_eq!(
            docs.severity,
            Severity::Advice,
            "weak is advice, never a problem"
        );

        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(*coverage(&report), CheckRun::Ran { findings: 3 });
    }

    /// A `packages/CLAUDE.md` covers every member: the group disappears
    /// and the other findings stand. Sabotage-proven: with the ancestor
    /// check dropped, the group is still emitted.
    #[test]
    fn a_claude_md_in_the_parent_suppresses_the_group() {
        let t = fixture();
        write(&t.path().join("packages").join("CLAUDE.md"), "# packages\n");
        let report = run_over(t.path());
        let found = gap_findings(&report);
        assert!(
            by_subject(&found, "packages").is_none(),
            "a parent file covers its members: {found:?}"
        );
        assert!(by_subject(&found, "octocat-a").is_none());
        assert!(by_subject(&found, "tests").is_some());
        assert!(by_subject(&found, "docs").is_some());
    }

    /// A `## packages` heading in the root names the members' PARENT, so
    /// the group is downgraded -- still listed, weak, with the heading's
    /// line as evidence -- and not gone.
    #[test]
    fn a_root_heading_naming_the_parent_downgrades_the_group_not_removes_it() {
        let t = fixture();
        write(
            &t.path().join("CLAUDE.md"),
            "# hello-world\n\nRules.\n\n## packages\n\nShared conventions.\n",
        );
        let report = run_over(t.path());
        let found = gap_findings(&report);
        let group = by_subject(&found, "packages").expect("downgraded, not gone");
        assert!(
            group
                .finding
                .contains("though the root CLAUDE.md has a section `packages` for its parent"),
            "{}",
            group.finding
        );
        assert_eq!(
            group.evidence[1].at,
            Locator::File {
                path: t.path().join("CLAUDE.md").to_string_lossy().to_string(),
                line: Some(5),
            }
        );
        let scan = scan_repo(t.path());
        let c = candidates(&scan, &[]).unwrap();
        let a = c.iter().find(|g| g.rel == "packages/octocat-a").unwrap();
        assert_eq!(a.strength, Strength::Weak);
        assert!(a.downgraded_by.is_some());
    }

    /// A heading naming the directory ITSELF suppresses it: the parent
    /// has given it a section, and whether that section should move
    /// down is the placement producer's question.
    #[test]
    fn a_root_heading_naming_the_directory_suppresses_it() {
        let t = fixture();
        write(
            &t.path().join("CLAUDE.md"),
            "# hello-world\n\n## tests\n\nRun them with `yarn vitest run`.\n",
        );
        let report = run_over(t.path());
        let found = gap_findings(&report);
        assert!(by_subject(&found, "tests").is_none(), "{found:?}");
        assert!(by_subject(&found, "docs").is_some(), "unaffected");
    }

    /// A path reference in the root downgrades a strong candidate, and
    /// drops a weak one.
    #[test]
    fn a_root_path_reference_downgrades_strong_and_drops_weak() {
        let t = fixture();
        write(
            &t.path().join("CLAUDE.md"),
            "# hello-world\n\nSee `tests/t0.test.ts` for the shape, and `docs/README.md`.\n",
        );
        let report = run_over(t.path());
        let found = gap_findings(&report);
        let tests = by_subject(&found, "tests").expect("downgraded, not gone");
        assert!(
            tests
                .finding
                .contains("though the root CLAUDE.md references `tests/t0.test.ts`"),
            "{}",
            tests.finding
        );
        assert!(
            !tests.finding.contains("role name only"),
            "a downgraded manifest is not a role: {}",
            tests.finding
        );
        assert!(
            by_subject(&found, "docs").is_none(),
            "a role name the root already points at is nothing to say: {found:?}"
        );
    }

    /// The edited signal, absolute or relative, upgrades a weak candidate
    /// and makes a candidate of a directory with no other signal.
    #[test]
    fn the_edited_signal_upgrades_and_creates_candidates() {
        let t = fixture();
        write(&t.path().join("lib").join("util.ts"), "export {};\n");
        let scan = scan_repo(t.path());

        let none = candidates(&scan, &[]).unwrap();
        assert_eq!(
            none.iter().find(|g| g.rel == "docs").unwrap().strength,
            Strength::Weak
        );
        assert!(
            none.iter().all(|g| g.rel != "lib"),
            "no signal, no candidate"
        );

        let edited = [t.path().join("docs"), PathBuf::from("lib")];
        let with = candidates(&scan, &edited).unwrap();
        let docs = with.iter().find(|g| g.rel == "docs").unwrap();
        assert_eq!(docs.strength, Strength::Strong);
        assert!(docs.signals.contains(&"session edits".to_string()));
        assert!(!docs.role_only);
        assert!(
            with.iter()
                .any(|g| g.rel == "lib" && g.strength == Strength::Strong),
            "a relative path is joined to the root: {with:?}"
        );
    }

    /// An unlistable directory is an Unknown finding and no gap, for it
    /// or anything under it.
    #[cfg(unix)]
    #[test]
    fn an_unlistable_directory_is_unknown_and_never_a_gap() {
        let t = fixture();
        let locked = t.path().join("locked");
        let report = with_walled(&locked, || run_over(t.path()));
        let found = gap_findings(&report);
        let unknown = by_subject(&found, "locked").expect("an Unknown for locked/");
        assert_eq!(unknown.severity, Severity::Unknown);
        assert!(
            unknown.finding.contains("could not be listed"),
            "{}",
            unknown.finding
        );
        assert!(unknown.brief.contains("listable"), "{}", unknown.brief);
        assert!(
            !found
                .iter()
                .any(|f| f.severity == Severity::Advice && f.subject.path().contains("locked")),
            "{found:?}"
        );
        // The other findings stand: partial is not nothing.
        assert!(by_subject(&found, "packages").is_some());
    }

    /// The "nor anything under it" half, on a hand-built scan: a directory
    /// under an unlistable one carries a manifest and must still not be a
    /// gap. The walk cannot produce this shape (it never lists under a
    /// wall), so the guard is proven on the facts directly. Sabotage-
    /// proven: with the `unreadable` prefix check dropped, `locked/inner`
    /// becomes a strong finding.
    #[test]
    fn a_directory_under_an_unlistable_one_is_never_a_gap() {
        let root = PathBuf::from("/home/octocat/hello-world");
        let scan = Scan {
            dir_facts: vec![
                DirFacts {
                    path: root.clone(),
                    rel: PathBuf::new(),
                    ..Default::default()
                },
                DirFacts {
                    path: root.join("locked"),
                    rel: PathBuf::from("locked"),
                    unreadable: Some("Permission denied (os error 13)".into()),
                    ..Default::default()
                },
                DirFacts {
                    path: root.join("locked").join("inner"),
                    rel: PathBuf::from("locked").join("inner"),
                    manifests: vec!["Cargo.toml".into()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let found = gaps(&scan, &[]).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].severity, Severity::Unknown);
        assert!(candidates(&scan, &[]).unwrap().is_empty());
    }

    /// A member with a signal of its own stays its own row while its
    /// siblings group.
    #[test]
    fn a_member_with_a_distinct_signal_stays_its_own_row() {
        let t = fixture();
        write(
            &t.path()
                .join("packages")
                .join("octocat-d")
                .join("package.json"),
            r#"{"name":"octocat-d"}"#,
        );
        for i in 0..12 {
            write(
                &t.path()
                    .join("packages")
                    .join("octocat-d")
                    .join(format!("d{i}.test.ts")),
                "t\n",
            );
        }
        let report = run_over(t.path());
        let found = gap_findings(&report);
        let group = by_subject(&found, "packages").expect("the three plain members group");
        assert!(
            group.finding.starts_with("3 directories"),
            "{}",
            group.finding
        );
        assert!(!group.evidence[0].measured.contains("octocat-d"));
        let d = by_subject(&found, "octocat-d").expect("d has its own row");
        assert!(d.finding.contains("12 test files"), "{}", d.finding);
    }

    /// Two siblings do not group: a group of one or two is just rows.
    #[test]
    fn fewer_than_three_siblings_are_separate_rows() {
        let t = fixture();
        fs::remove_dir_all(t.path().join("packages").join("octocat-c")).unwrap();
        let report = run_over(t.path());
        let found = gap_findings(&report);
        assert!(by_subject(&found, "octocat-a").is_some());
        assert!(by_subject(&found, "octocat-b").is_some());
        assert!(by_subject(&found, "packages").is_none(), "{found:?}");
    }

    /// A workspace manifest that will not parse is Unknown for the member
    /// question; the members' own manifests are still a signal.
    #[test]
    fn an_unparseable_workspace_manifest_is_unknown_for_membership() {
        let t = fixture();
        write(&t.path().join("package.json"), "{ not json");
        let report = run_over(t.path());
        let found = gap_findings(&report);
        let unknown = found
            .iter()
            .find(|f| f.severity == Severity::Unknown)
            .expect("the root manifest is Unknown for membership");
        assert!(
            unknown
                .finding
                .contains("workspace members that could not be read"),
            "{}",
            unknown.finding
        );
        assert!(unknown.evidence[0].measured.contains("not valid JSON"));
        let group = by_subject(&found, "packages").expect("still grouped on own package.json");
        assert!(group.finding.contains("own `package.json`"));
        assert!(
            !group.finding.contains("workspace membership"),
            "membership is unknown, not asserted: {}",
            group.finding
        );
    }

    /// A root CLAUDE.md that exists and cannot be read makes the whole
    /// check Unknown: without it, no candidate's suppression is decidable.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_root_claude_md_makes_the_check_unknown() {
        use std::os::unix::fs::PermissionsExt;
        let t = fixture();
        let file = t.path().join("CLAUDE.md");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
        let report = run_over(t.path());
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        match coverage(&report) {
            CheckRun::Unknown { reason } => {
                assert!(reason.contains("could not be read"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
        assert!(gap_findings(&report).is_empty());
    }

    /// A repository with no root CLAUDE.md still has candidates: nothing
    /// covers them, and nothing can suppress them.
    #[test]
    fn a_repository_without_a_root_file_still_has_candidates() {
        let t = fixture();
        fs::remove_file(t.path().join("CLAUDE.md")).unwrap();
        let report = run_over(t.path());
        assert_eq!(gap_findings(&report).len(), 3);
    }

    /// The threshold is a boundary: nine test files is no signal, ten is
    /// strong.
    #[test]
    fn the_test_file_threshold_is_a_boundary() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..TEST_FILE_THRESHOLD - 1 {
            write(&t.path().join("nine").join(format!("t{i}.spec.ts")), "t\n");
        }
        for i in 0..TEST_FILE_THRESHOLD {
            write(&t.path().join("ten").join(format!("t{i}.spec.ts")), "t\n");
        }
        let scan = scan_repo(t.path());
        let c = candidates(&scan, &[]).unwrap();
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].rel, "ten");
        assert_eq!(c[0].strength, Strength::Strong);
    }

    /// A CLAUDE.md anywhere between a directory and the root covers it,
    /// however deep. Sabotage-proven: with the ancestor check dropped,
    /// `src-tauri/plugins/x` is a strong finding.
    #[test]
    fn a_claude_md_anywhere_above_covers_a_nested_directory() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("CLAUDE.md"), "# root\n");
        write(&t.path().join("src-tauri").join("CLAUDE.md"), "# tauri\n");
        write(
            &t.path()
                .join("src-tauri")
                .join("plugins")
                .join("x")
                .join("Cargo.toml"),
            "[package]\nname = \"x\"\n",
        );
        write(
            &t.path().join("crates").join("y").join("Cargo.toml"),
            "[package]\nname = \"y\"\n",
        );
        let scan = scan_repo(t.path());
        let c = candidates(&scan, &[]).unwrap();
        let rels: Vec<&str> = c.iter().map(|g| g.rel.as_str()).collect();
        assert_eq!(rels, ["crates/y"], "{c:?}");
        assert!(c[0].signals.contains(&"role name `crates/*`".to_string()));
    }

    /// The brief's suggestion follows the finding's shape: the contract
    /// `brief::gaps_suggestion` keys on.
    #[test]
    fn the_brief_suggestion_follows_the_finding_shape() {
        let t = fixture();
        let report = run_over(t.path());
        let found = gap_findings(&report);
        let group = by_subject(&found, "packages").unwrap();
        assert!(
            group.brief.contains("shared by the members"),
            "{}",
            group.brief
        );
        assert!(
            group.brief.contains(&format!(
                "Add `{}/CLAUDE.md`",
                t.path().join("packages").to_string_lossy()
            )),
            "{}",
            group.brief
        );
        let docs = by_subject(&found, "docs").unwrap();
        assert!(
            docs.brief
                .contains("only if a convention is true only here"),
            "{}",
            docs.brief
        );
        let tests = by_subject(&found, "tests").unwrap();
        assert!(
            tests
                .brief
                .contains("how it is built and tested on its own"),
            "{}",
            tests.brief
        );
        assert!(tests.brief.contains("Keep it to what is true only here."));
    }

    /// The Cargo side of membership, on this repository's own shape: a
    /// `[workspace]` under `src-mobile/` lists `plugins/*`, and the
    /// members are covered by `src-mobile/CLAUDE.md`; a sibling crate
    /// with a lock of its own is the strong candidate.
    #[test]
    fn cargo_workspace_members_are_recorded_and_covered_by_their_parent() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("CLAUDE.md"), "# root\n");
        write(&t.path().join("src-mobile").join("CLAUDE.md"), "# mobile\n");
        write(
            &t.path().join("src-mobile").join("Cargo.toml"),
            "[workspace]\nmembers = [\"plugins/*\"]\n",
        );
        for p in ["keys", "notify", "refresh"] {
            write(
                &t.path()
                    .join("src-mobile")
                    .join("plugins")
                    .join(p)
                    .join("Cargo.toml"),
                "[package]\n",
            );
        }
        let stepup = t.path().join("crates").join("octocat-stepup");
        write(&stepup.join("Cargo.toml"), "[package]\n");
        write(&stepup.join("Cargo.lock"), "");
        write(&stepup.join("deny.toml"), "");

        let scan = scan_repo(t.path());
        let keys = scan
            .dir_facts
            .iter()
            .find(|f| f.rel == Path::new("src-mobile").join("plugins").join("keys"))
            .unwrap();
        assert_eq!(
            keys.workspace_of,
            Some(t.path().join("src-mobile").join("Cargo.toml"))
        );

        let c = candidates(&scan, &[]).unwrap();
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].rel, "crates/octocat-stepup");
        assert_eq!(
            c[0].signals[0],
            "own `Cargo.toml`, `Cargo.lock`, `deny.toml`"
        );
    }
}
