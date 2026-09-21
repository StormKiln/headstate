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

use super::{Check, Context, Evidence, Finding, Locator, Producer, Severity, Subject};
use crate::claudemd::{ImportNode, Scope};

pub struct Imports;

impl Producer for Imports {
    fn check(&self) -> Check {
        Check::Imports
    }

    fn run(&self, cx: &Context) -> Result<Vec<Finding>, String> {
        let mut out = Vec::new();
        for f in &cx.scan.repo.files {
            walk(&f.path, Scope::Repo, &f.path, &f.imports, &mut out);
        }
        for s in &cx.scan.extra {
            walk(
                &s.file.path,
                s.scope,
                &s.file.path,
                &s.file.imports,
                &mut out,
            );
        }
        Ok(out)
    }
}

/// Walk one import tree.
///
/// `subject` is the CLAUDE.md that loads the tree and `importer` is the
/// file that wrote the import being looked at: the CLAUDE.md itself at
/// the top, and the resolved import one level up below that. A broken
/// node has no children, so `importer` always has a resolved path.
fn walk(subject: &str, scope: Scope, importer: &str, nodes: &[ImportNode], out: &mut Vec<Finding>) {
    for n in nodes {
        if let Some(problem) = &n.problem {
            out.push(Finding::new(
                Check::Imports,
                Severity::Problem,
                Subject::ClaudeMd {
                    path: subject.to_string(),
                    scope,
                    section: None,
                },
                vec![Evidence {
                    at: Locator::File {
                        path: importer.to_string(),
                        line: None,
                    },
                    measured: format!("`{}`: {problem}", n.raw),
                }],
                format!("`{}` in `{importer}` does not resolve: {problem}", n.raw),
            ));
        }
        if let Some(path) = &n.path {
            walk(subject, scope, path, &n.children, out);
        }
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
        let cx = Context {
            repo,
            home: None,
            scan: &scan,
            definitions: None,
            conn: None,
        };
        super::super::run(&cx)
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

        assert_eq!(report.findings.len(), 1, "{report:?}");
        let f = &report.findings[0];
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

        assert!(report.findings.is_empty(), "{report:?}");
        assert_eq!(report.checks[0].run, CheckRun::Ran { findings: 0 });
        assert!(report.brief.contains("nothing found"), "{}", report.brief);
    }

    /// A problem two levels down names the file that WROTE the import,
    /// which is where the edit goes, and the CLAUDE.md as the subject.
    #[test]
    fn a_nested_problem_names_the_importing_file() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "@./a.md\n").unwrap();
        fs::write(t.path().join("a.md"), "@./CLAUDE.md\n").unwrap();

        let report = run_over(t.path());

        assert_eq!(report.findings.len(), 1, "{report:?}");
        let f = &report.findings[0];
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
