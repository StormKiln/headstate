//! The seed producer: one [`Severity::Problem`] finding per import the
//! scan already knows is broken.
//!
//! Computes nothing new. `claudemd::imports::resolve_tree` records an
//! `ImportNode::problem` for a file it could not find, a cycle, and a file
//! it could not read, and the page shows each as a red pill with no
//! remedy. This producer re-states each one as a finding with a brief, so
//! the panel renders a real row -- and the seam is proven end to end --
//! before any producer that measures something exists.
//!
//! # Evidence carries no line
//!
//! `ImportNode` records the raw import and where it resolved, never the
//! line it was on. The evidence is therefore `Locator::File { line: None }`
//! plus the raw import text, and the brief prints the path alone. Inventing
//! a line here would be the confident wrong number this codebase refuses.
//!
//! # A partial scan does not make this check Unknown
//!
//! Every finding here is the resolver's own verdict about a file that did
//! read. What the walk could not read is already stated above the file
//! list by `PartialScanNotice`, and the findings that exist are real
//! whatever else was unreadable.
//!
//! # Two rules from the memory docs, beyond what the scan carries
//!
//! The content-shape research (`docs/superpowers/specs/
//! 2026-09-21-claude-md-practices-research.md`) hands this producer two
//! loading facts it can check from the tree alone:
//!
//! - **Depth.** "Imported files can recursively import other files, with
//!   a maximum depth of four hops." A file five hops from the CLAUDE.md
//!   is resolved by the scan and counted in the tree, and is not loaded
//!   by Claude Code. That is a [`Severity::Problem`] with the chain
//!   listed one path per hop. cclint says five; the docs win.
//! - **External.** "An import in a project-level memory file is external
//!   when its path resolves outside your working directory", and Claude
//!   Code shows an approval dialog before loading it, disabling the
//!   import if declined. A [`Severity::Advice`] for a repo-scope file,
//!   never for `~/.claude/CLAUDE.md`, whose imports are outside every
//!   repository by construction.

use super::{Check, Context, Evidence, Finding, Locator, Producer, Severity, Subject};
use crate::claudemd::{ImportNode, Scope};
use std::path::Path;

/// The depth Claude Code follows imports to. Memory docs: "a maximum
/// depth of four hops".
const MAX_HOPS: usize = 4;

pub struct Imports;

impl Producer for Imports {
    fn check(&self) -> Check {
        Check::Imports
    }

    fn run(&self, cx: &Context) -> Result<Vec<Finding>, String> {
        let mut out = Vec::new();
        for f in &cx.scan.repo.files {
            let mut chain = vec![f.path.clone()];
            walk(
                &f.path,
                Scope::Repo,
                &f.imports,
                1,
                &mut chain,
                cx.repo,
                &mut out,
            );
        }
        for s in &cx.scan.extra {
            let mut chain = vec![s.file.path.clone()];
            walk(
                &s.file.path,
                s.scope,
                &s.file.imports,
                1,
                &mut chain,
                cx.repo,
                &mut out,
            );
        }
        Ok(out)
    }
}

/// Walk one import tree.
///
/// `subject` is the CLAUDE.md that loads the tree. `chain` is the path
/// walked so far, the CLAUDE.md first; its last entry is the file that
/// wrote the import being looked at, which is where an edit goes. A
/// broken node has no children, so the chain's last entry always has a
/// resolved path. `hop` is how many imports deep `nodes` sit: the
/// CLAUDE.md's own imports are hop 1.
fn walk(
    subject: &str,
    scope: Scope,
    nodes: &[ImportNode],
    hop: usize,
    chain: &mut Vec<String>,
    repo: &Path,
    out: &mut Vec<Finding>,
) {
    let importer = chain.last().cloned().unwrap_or_default();
    let subject_of = || Subject::ClaudeMd {
        path: subject.to_string(),
        scope,
        section: None,
    };
    for n in nodes {
        if hop > MAX_HOPS {
            // Not loaded, whatever else is true of it: a broken import
            // five hops down is moot, and reporting it would send the
            // reader to fix a file Claude Code never reads.
            let mut evidence: Vec<Evidence> = chain
                .iter()
                .enumerate()
                .map(|(i, p)| Evidence {
                    at: Locator::File {
                        path: p.clone(),
                        line: None,
                    },
                    measured: if i == 0 {
                        "the CLAUDE.md".to_string()
                    } else {
                        format!("hop {i}")
                    },
                })
                .collect();
            evidence.push(Evidence {
                at: Locator::File {
                    path: n.path.clone().unwrap_or_else(|| n.raw.clone()),
                    line: None,
                },
                measured: format!("hop {hop}: not loaded"),
            });
            out.push(Finding::new(
                Check::Imports,
                Severity::Problem,
                subject_of(),
                evidence,
                format!(
                    "`{}` in `{importer}` is {hop} hops from `{subject}`; Claude Code follows \
                     imports to a maximum depth of four",
                    n.raw
                ),
            ));
            continue;
        }
        if let Some(problem) = &n.problem {
            out.push(Finding::new(
                Check::Imports,
                Severity::Problem,
                subject_of(),
                vec![Evidence {
                    at: Locator::File {
                        path: importer.clone(),
                        line: None,
                    },
                    measured: format!("`{}`: {problem}", n.raw),
                }],
                format!("`{}` in `{importer}` does not resolve: {problem}", n.raw),
            ));
        }
        if let Some(path) = &n.path {
            if n.problem.is_none() && scope != Scope::Global && outside(path, repo) {
                out.push(Finding::new(
                    Check::Imports,
                    Severity::Advice,
                    subject_of(),
                    vec![Evidence {
                        at: Locator::File {
                            path: importer.clone(),
                            line: None,
                        },
                        measured: format!("`{}` resolves to `{path}`", n.raw),
                    }],
                    format!(
                        "`{}` in `{importer}` resolves outside the repository; it loads only \
                         after the external-imports approval",
                        n.raw
                    ),
                ));
            }
            chain.push(path.clone());
            walk(subject, scope, &n.children, hop + 1, chain, repo, out);
            chain.pop();
        }
    }
}

/// Whether a resolved import lies outside the repository.
///
/// Both sides canonicalised, so a `../` in the import and a symlinked
/// repository compare as the paths they really are. A path that cannot
/// be canonicalised is not called external: possibly wrong, so
/// suppressed.
fn outside(path: &str, repo: &Path) -> bool {
    match (Path::new(path).canonicalize(), repo.canonicalize()) {
        (Ok(p), Ok(r)) => !p.starts_with(&r),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claudemd::advice::{CheckRun, Report};
    use crate::claudemd::scan_effective_opt;
    use std::fs;

    fn run_over(repo: &std::path::Path) -> Report {
        let scan = scan_effective_opt(repo, None);
        // An empty inventory, so a producer that needs one runs and the
        // report these tests inspect is not partial for a reason that
        // has nothing to do with imports.
        let inv = crate::claude::definitions::Inventory::default();
        let cx = Context {
            repo,
            home: None,
            scan: &scan,
            definitions: Some(&inv),
            conn: None,
        };
        // The whole run, narrowed to this check: the tests below assert
        // on partiality and the "nothing found" line, and a sibling
        // producer that needs a store or an inventory this context does
        // not carry would make both about the sibling, not about imports.
        let mut report = super::super::run(&cx);
        report.findings.retain(|f| f.check == Check::Imports);
        report.checks.retain(|c| c.check == Check::Imports);
        report.brief = crate::claudemd::advice::brief::render_report(&report);
        report
    }

    /// This producer's findings alone. The report runs every producer,
    /// and the shape producer emits its launch-set row for any repository
    /// with a root CLAUDE.md.
    fn imports_findings(report: &Report) -> Vec<&Finding> {
        report
            .findings
            .iter()
            .filter(|f| f.check == Check::Imports)
            .collect()
    }

    /// The memory docs: "a maximum depth of four hops". The fifth hop is
    /// resolved by the scan and never loaded by Claude Code, so it is a
    /// Problem with the whole chain in evidence; a chain of exactly four
    /// is not.
    #[test]
    fn a_chain_deeper_than_four_hops_is_a_problem_with_the_chain_listed() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "@./a.md\n").unwrap();
        fs::write(t.path().join("a.md"), "@./b.md\n").unwrap();
        fs::write(t.path().join("b.md"), "@./c.md\n").unwrap();
        fs::write(t.path().join("c.md"), "@./d.md\n").unwrap();
        fs::write(t.path().join("d.md"), "leaf at hop four").unwrap();

        let four = run_over(t.path());
        assert!(
            imports_findings(&four).is_empty(),
            "four hops is within the limit: {four:?}"
        );

        // One more hop.
        fs::write(t.path().join("d.md"), "@./e.md\n").unwrap();
        fs::write(t.path().join("e.md"), "leaf at hop five").unwrap();

        let five = run_over(t.path());
        let found = imports_findings(&five);
        assert_eq!(found.len(), 1, "{five:?}");
        let f = found[0];
        assert_eq!(f.severity, Severity::Problem);
        assert!(f.finding.contains("5 hops"), "{}", f.finding);
        assert!(f.finding.contains("maximum depth of four"), "{}", f.finding);
        // The chain, one path per hop: the CLAUDE.md, a, b, c, d, then e.
        assert_eq!(f.evidence.len(), 6, "{:?}", f.evidence);
        assert_eq!(f.evidence[0].measured, "the CLAUDE.md");
        assert!(at_path(&f.evidence[0]).ends_with("CLAUDE.md"));
        for (i, e) in f.evidence.iter().enumerate().skip(1).take(4) {
            assert_eq!(e.measured, format!("hop {i}"), "{e:?}");
        }
        assert_eq!(f.evidence[5].measured, "hop 5: not loaded");
        assert!(at_path(&f.evidence[5]).ends_with("e.md"));
    }

    /// The memory docs: an import in a project-level file that resolves
    /// outside the working directory loads only after an approval
    /// dialog. Advice, not a Problem: it does load once approved.
    #[test]
    fn an_import_outside_the_repository_is_advice_naming_the_approval() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        let elsewhere = t.path().join("elsewhere");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        let shared = elsewhere.join("shared.md");
        fs::write(&shared, "shared rules").unwrap();
        fs::write(
            repo.join("CLAUDE.md"),
            format!("@{}\n@./inside.md\n", shared.to_string_lossy()),
        )
        .unwrap();
        fs::write(repo.join("inside.md"), "inside").unwrap();

        let report = run_over(&repo);
        let found = imports_findings(&report);
        assert_eq!(found.len(), 1, "only the external one: {report:?}");
        let f = found[0];
        assert_eq!(f.severity, Severity::Advice);
        assert!(
            f.finding.contains("outside the repository"),
            "{}",
            f.finding
        );
        assert!(
            f.finding.contains("external-imports approval"),
            "{}",
            f.finding
        );
        assert!(!f.finding.contains("inside.md"));
    }

    fn at_path(e: &Evidence) -> String {
        match &e.at {
            Locator::File { path, .. } => path.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// The founding case: a CLAUDE.md importing a file that does not
    /// exist produces one Problem finding whose brief names the file and
    /// the import, and no line.
    #[test]
    fn a_broken_import_is_a_problem_finding_with_a_brief() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("CLAUDE.md");
        fs::write(&file, "# rules\n@./missing.md\n").unwrap();

        let report = run_over(t.path());

        let found = imports_findings(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        let f = found[0];
        assert_eq!(f.check, Check::Imports);
        assert_eq!(f.severity, Severity::Problem);
        assert_eq!(f.subject.path(), file.to_string_lossy());
        assert!(f.finding.contains("@./missing.md"), "{}", f.finding);
        assert!(f.finding.contains("file not found"), "{}", f.finding);
        assert!(f.brief.contains(&*file.to_string_lossy()), "{}", f.brief);
        // No line: `ImportNode` does not record one.
        assert_eq!(
            f.evidence[0].at,
            Locator::File {
                path: file.to_string_lossy().to_string(),
                line: None
            }
        );
        assert!(
            !f.brief.contains(&format!("{}:", file.to_string_lossy())),
            "{}",
            f.brief
        );
        assert_eq!(
            report.checks[0].run,
            CheckRun::Ran { findings: 1 },
            "{report:?}"
        );
        assert!(!report.is_partial());
    }

    /// A tree that resolves cleanly is not a finding, and the report says
    /// the check ran and found nothing.
    #[test]
    fn a_resolving_import_is_not_a_finding() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "@./shared.md\n").unwrap();
        fs::write(t.path().join("shared.md"), "shared").unwrap();

        let report = run_over(t.path());

        assert!(imports_findings(&report).is_empty(), "{report:?}");
        assert_eq!(report.checks[0].run, CheckRun::Ran { findings: 0 });
    }

    /// A problem two levels down names the file that WROTE the import,
    /// which is where the edit goes, and the CLAUDE.md as the subject.
    #[test]
    fn a_nested_problem_names_the_importing_file() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "@./a.md\n").unwrap();
        fs::write(t.path().join("a.md"), "@./CLAUDE.md\n").unwrap();

        let report = run_over(t.path());

        let found = imports_findings(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        let f = found[0];
        assert!(f.finding.contains("circular import"), "{}", f.finding);
        // Compared by suffix: the resolver keeps the `./` the import was
        // written with, and the property under test is WHICH file, not
        // how its path is spelled.
        match &f.evidence[0].at {
            Locator::File { path, line: None } => assert!(
                path.ends_with("a.md") && !path.ends_with("CLAUDE.md"),
                "the evidence is the file that wrote the import: {path}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(
            f.subject.path().ends_with("CLAUDE.md"),
            "the subject is the CLAUDE.md that loads the tree: {:?}",
            f.subject
        );
    }
}
