//! Silently-broken agent configuration, swept across every repository
//! (#1217).
//!
//! `install.rs`'s header records the measurement this module exists for.
//! `claude` was started with a deliberately corrupt `settings.json`: it
//! "started normally, answered normally, and the hooks **never ran**. No
//! error, no warning, no line on stderr."
//!
//! Headstate already detected exactly one instance of that class, for
//! one file, as a side effect of asking a different question -- the
//! install dialog reads `~/.claude/settings.json` before it writes to
//! it. Across ~38 checkouts nothing asked at all. A repository whose
//! `.claude/settings.local.json` lost a brace has every rule in that
//! file inert, forever, with no symptom anyone would connect to it.
//!
//! # Findings are CHECKS, never opinions
//!
//! The load-bearing rule, and the reason this module is small. Every
//! [`Finding`] carries a [`Finding::proof`] that is a sentence the
//! producing code already wrote: serde's own parse error with its line
//! and column, or an import resolver's own message. Nothing here
//! inspects a config and forms a view about it.
//!
//! That is not squeamishness. A health page that mixes "this file
//! failed to parse at line 4 column 12" with "this config looks
//! unusual" trains its reader to skim both, and an ignored health page
//! is worse than no health page because it looks like coverage. So
//! there is no heuristic here, no style judgement, and no threshold:
//! the only things reported are a parser that refused and a resolver
//! that could not resolve.
//!
//! [`ScopeRefusal::detail`] already satisfies this, which is why
//! [`settings::effective_in`] is CALLED rather than reimplemented. A
//! second settings parser in this tree would be a second source of
//! truth about someone else's file format, and wrong the moment the
//! first one changed.
//!
//! # A check that could not run is not a check that passed
//!
//! #1042's rule, arriving here for the third time. A repository whose
//! `.claude/` directory could not be listed has an unknown number of
//! problems in it, not zero. It reports [`Verdict::Unknown`], and
//! [`Verdict::rank`] orders Unknown AWAY from the passes rather than
//! beside them -- sorting a repository we could not look at next to
//! thirty we cleared is how "we did not check" becomes "it is fine".
//!
//! # The finding names the SCOPE
//!
//! A repository where only the local scope failed is not "unhealthy".
//! The remedy is one specific file, and naming it is the difference
//! between a page that fixes something and a red dot.
//!
//! So a finding carries the [`Origin`] whose file refused AND the
//! tracked keys that became [`ResolvedKey::undecidable`] because of it
//! -- which is the consequence the user actually feels. `settings.rs`
//! was built to preserve that per-scope distinction; flattening three
//! scopes into one indicator here would discard it one layer up.
//!
//! # What is deliberately NOT checked
//!
//! No PATH check for a hook binary, no MCP stdio command check, no
//! cross-scope shadowing. Those need #1215 and #1216 and are not
//! half-built the way parse failures and import problems already are.
//! Shipping a guess at them would violate the first rule above on the
//! first screen.
//!
//! # What it costs, measured
//!
//! Over this machine's real `~/code`: **39 repositories, 2.0 s to find
//! them and 5.6 s to sweep them cold**, 0.9 s warm once the OS page
//! cache is hot.
//!
//! The two halves were timed apart rather than guessed at, because they
//! are not remotely comparable:
//!
//! ```text
//! settings check (3 file reads x 39 repos)     10 ms
//! CLAUDE.md walk (directory tree x 39 repos)  4737 ms
//! ```
//!
//! So the settings half -- the one this ticket is actually about -- is
//! free, and essentially all the cost is `claudemd::scan_repo` walking
//! directory trees. That is a real number and not a small one, which is
//! why the command is `async`, why the panel is collapsed by default,
//! and why its query does not refetch on window focus. It is NOT
//! quadratic: nothing compares repositories to each other, so the cost
//! is linear in repositories and in the files each walk visits.
//!
//! Read-only. Nothing here writes, repairs, or offers to.

use serde::{Deserialize, Serialize};
use std::path::Path;

use super::settings::{effective_in, Origin};

/// What a repository's sweep concluded.
///
/// Three states, because two would lie -- the same shape
/// [`super::install::Status`] takes and for the same reason. The one
/// that must never collapse into another is [`Verdict::Unknown`]: it is
/// not a pass with a caveat, it is the absence of an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    /// Every check ran, and every check was clean.
    ///
    /// The only state that may be rendered as reassurance, because it is
    /// the only one where something was actually established.
    Pass,
    /// At least one check ran and FAILED, with a proof.
    Problem,
    /// At least one check could not run, and nothing that did run failed.
    ///
    /// Not a pass. The `.claude` directory behind a permission wall may
    /// hold a settings file that has been silently inert for months.
    Unknown,
}

impl Verdict {
    /// Sort order for the sweep, worst first.
    ///
    /// Problem, then Unknown, then Pass. The gap that matters is the one
    /// between Unknown and Pass: an Unknown sorted among the passes reads
    /// as "checked, fine", which is the exact claim this module refuses
    /// to make. Ordering it above every pass keeps "we could not look" in
    /// the part of the list a user reads.
    pub fn rank(self) -> u8 {
        match self {
            Verdict::Problem => 0,
            Verdict::Unknown => 1,
            Verdict::Pass => 2,
        }
    }
}

/// Which check produced a finding.
///
/// Carried rather than inferred from the proof's wording: the remedy
/// differs (an editor for a parse error, permissions for a wall) and a
/// frontend that pattern-matched a sentence would break the first time
/// one was reworded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Check {
    /// A `settings.json` scope Claude Code would read and cannot parse.
    ///
    /// The founding case. Claude Code ignores such a file silently, so
    /// every rule and every hook in it is already doing nothing.
    SettingsParse,
    /// A CLAUDE.md `@import` that is broken or circular.
    ClaudeMdImport,
    /// A definition file under `~/.claude` that exists and could not be
    /// read.
    Definition,
    /// A directory the sweep could not list.
    ///
    /// The only check whose finding is [`Severity::Unknown`]: it hides an
    /// unknown number of the others.
    Unreadable,
}

/// Whether a finding is something that IS wrong, or something we could
/// not determine.
///
/// Separate from [`Verdict`], which is the repository's roll-up. A
/// repository can hold both kinds at once, and flattening them would
/// make the roll-up unable to distinguish "one file is broken and the
/// rest are clean" from "one file is broken and we could not see the
/// rest".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    /// Proven broken. The proof says how.
    Problem,
    /// Could not be checked. The proof says why not.
    Unknown,
}

/// One thing found, and the evidence for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub check: Check,
    pub severity: Severity,
    /// The file or directory involved, absolute, so the UI can reveal it.
    pub path: String,
    /// Which settings scope refused, when the finding is about one.
    ///
    /// `None` for checks that have no scope -- an import problem belongs
    /// to a CLAUDE.md, not to one of the three settings files. A
    /// frontend must not invent a scope for those.
    pub scope: Option<Origin>,
    /// The producing code's OWN sentence, verbatim.
    ///
    /// For a settings parse failure this is
    /// [`super::settings::ScopeRefusal::detail`], which is serde's error
    /// with its line and column wrapped in the refusal's explanation.
    /// Never paraphrased and never summarised: the line number is the
    /// only actionable thing in it, and a UI that rewrote this sentence
    /// would be inventing a claim it cannot support.
    pub proof: String,
    /// The tracked settings keys that became undecidable because of this
    /// refusal, in [`super::settings::KEYS`] order.
    ///
    /// The consequence the user actually feels, and the reason naming
    /// the scope is not pedantry: an unreadable `settings.local.json`
    /// means Headstate cannot say which `model` or which `permissions`
    /// are in force, and this is the list of what went dark.
    ///
    /// Empty for a scope refusal that changed no answer -- an unreadable
    /// USER file cannot override a key a local file already decided, and
    /// `effective_in` is what knows that. Empty is not a reason to hide
    /// the finding: the file is still inert.
    pub undecidable_keys: Vec<String>,
}

/// One repository's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoHealth {
    /// Directory name, for the row.
    pub name: String,
    /// Absolute path to the checkout.
    pub path: String,
    pub verdict: Verdict,
    /// Worst first: problems, then unknowns.
    pub findings: Vec<Finding>,
}

/// The whole sweep.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sweep {
    /// Every repository, ranked worst first.
    pub repos: Vec<RepoHealth>,
    /// Scan roots the repository walk itself could not read, with why.
    ///
    /// Carried rather than dropped for #1025's reason: this is the
    /// shortfall in the CENSUS, not in any one repository. It is what
    /// makes "38 repositories are clean" honest or not, and a page that
    /// omitted it would say "all clear" about a machine it half looked
    /// at.
    pub unreadable_roots: Vec<String>,
    /// Findings that belong to the MACHINE rather than to any
    /// repository.
    ///
    /// `~/.claude/skills` is loaded into every session on this machine,
    /// so a directory of it behind a permission wall is one fact about
    /// the machine -- not 38 facts about 38 checkouts. Kept apart for
    /// the reason `claudemd::Scope` exists: attributing a machine-wide
    /// file to one project is its own wrong answer.
    #[serde(default)]
    pub user_findings: Vec<Finding>,
}

impl Sweep {
    /// Repositories whose every check ran and passed.
    pub fn passed(&self) -> usize {
        self.count(Verdict::Pass)
    }

    /// Repositories with at least one proven problem.
    pub fn problems(&self) -> usize {
        self.count(Verdict::Problem)
    }

    /// Repositories where at least one check could not run.
    ///
    /// Reported as its own number rather than folded into either of the
    /// others. Added to the passes it would overstate coverage; added to
    /// the problems it would cry wolf.
    pub fn unknown(&self) -> usize {
        self.count(Verdict::Unknown)
    }

    fn count(&self, v: Verdict) -> usize {
        self.repos.iter().filter(|r| r.verdict == v).count()
    }

    /// Whether the census itself came back short.
    pub fn is_partial(&self) -> bool {
        !self.unreadable_roots.is_empty()
    }
}

/// Check one repository.
///
/// `home` and `repo` are both PARAMETERS, following `effective_in`'s
/// rule exactly: `$HOME` is process-global state and a test that changed
/// it would race every other test in the binary.
pub fn check_repo(home: &Path, repo: &Path) -> RepoHealth {
    let mut findings = Vec::new();

    settings_findings(home, repo, &mut findings);
    claude_md_findings(repo, &mut findings);

    // Worst first WITHIN the repository, for the same reason the sweep
    // ranks repositories: a proven breakage and a directory we could not
    // list are different claims, and the proven one is the one to read.
    findings.sort_by_key(|f| match f.severity {
        Severity::Problem => 0u8,
        Severity::Unknown => 1,
    });

    let verdict = if findings.iter().any(|f| f.severity == Severity::Problem) {
        Verdict::Problem
    } else if findings.is_empty() {
        Verdict::Pass
    } else {
        // Findings exist and none is a Problem, so every one of them is
        // an Unknown. The repository was not cleared; it was not read.
        Verdict::Unknown
    };

    RepoHealth {
        name: repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| repo.display().to_string()),
        path: repo.display().to_string(),
        verdict,
        findings,
    }
}

/// The settings half: three scopes, through the one parser.
fn settings_findings(home: &Path, repo: &Path, out: &mut Vec<Finding>) {
    let eff = effective_in(home, repo);

    for refusal in &eff.unreadable {
        // WHICH keys THIS scope put in doubt -- not every key some scope
        // did.
        //
        // The distinction only shows up when two scopes refuse at once,
        // and getting it wrong is constraint 3's own defect one level
        // down: an unreadable USER file would claim credit for a key
        // only the LOCAL file could have overridden, and the reader
        // would fix the wrong file.
        //
        // So the same precedence rule `effective_in` applies at :177 is
        // applied per refusal: a key is in doubt because of THIS scope
        // only when this scope outranks the best READABLE contribution,
        // or when no readable scope carried the key at all.
        //
        // `contributions` is what that rule is read from, NOT `winner`:
        // `effective_in` sets `winner` to `None` for exactly the keys
        // that are undecidable, so matching on it here would take the
        // "no readable scope carried it" arm every time and attribute
        // every key to every refusal -- the bug this comment exists to
        // prevent a return to. `contributions` still lists the scopes
        // that DID carry the key, in precedence order.
        //
        // `undecidable` is still required, so this never widens
        // `effective_in`'s answer -- it only attributes it.
        let undecidable_keys: Vec<String> = eff
            .keys
            .iter()
            .filter(|k| {
                k.undecidable
                    && match k.contributions.last().map(|c| c.origin) {
                        Some(best_readable) => refusal.origin > best_readable,
                        None => true,
                    }
            })
            .map(|k| k.key.clone())
            .collect();

        out.push(Finding {
            check: Check::SettingsParse,
            // A scope that exists and will not parse is PROVEN inert:
            // Claude Code ignores it silently, measured. That is a
            // problem, not an unknown -- the thing we could not do is
            // read it, but the thing we DO know is that neither can
            // Claude Code.
            severity: Severity::Problem,
            path: refusal.path.clone(),
            scope: Some(refusal.origin),
            proof: refusal.detail.clone(),
            undecidable_keys,
        });
    }
}

/// The CLAUDE.md half: broken and circular imports, plus what the walk
/// could not read.
fn claude_md_findings(repo: &Path, out: &mut Vec<Finding>) {
    let scan = crate::claudemd::scan_repo(repo);

    for dir in &scan.unreadable_dirs {
        out.push(Finding {
            check: Check::Unreadable,
            severity: Severity::Unknown,
            path: dir.clone(),
            scope: None,
            proof: dir.clone(),
            undecidable_keys: Vec::new(),
        });
    }
    for file in &scan.unreadable_files {
        out.push(Finding {
            check: Check::Unreadable,
            severity: Severity::Unknown,
            path: file.clone(),
            scope: None,
            proof: file.clone(),
            undecidable_keys: Vec::new(),
        });
    }

    for file in &scan.files {
        collect_import_problems(&file.path, &file.imports, out);
    }
}

/// Walk one file's import tree, reporting every node that carries a
/// `problem`.
///
/// `ImportNode::problem` is the resolver's own sentence -- "not found",
/// "circular import" -- which is a fact about the file rather than a
/// reading of it, so it qualifies as a proof under this module's rule.
fn collect_import_problems(
    owner: &str,
    nodes: &[crate::claudemd::imports::ImportNode],
    out: &mut Vec<Finding>,
) {
    for node in nodes {
        if let Some(problem) = &node.problem {
            out.push(Finding {
                check: Check::ClaudeMdImport,
                // `unreadable` is the resolver's OWN distinction between
                // "this file is not there" and "this file is there and we
                // could not read it" (#972). It maps exactly onto
                // Problem-vs-Unknown here, so it is read rather than
                // re-derived from the message text.
                severity: if node.unreadable {
                    Severity::Unknown
                } else {
                    Severity::Problem
                },
                // The importING file, because that is where the broken
                // line is and therefore where the edit goes. The
                // unresolved target is in the proof.
                path: owner.to_string(),
                scope: None,
                proof: format!("{owner} imports {}: {problem}", node.raw),
                undecidable_keys: Vec::new(),
            });
        }
        collect_import_problems(owner, &node.children, out);
    }
}

/// The machine-wide half: definitions under `~/.claude` that could not be
/// read.
///
/// Separate from the per-repository sweep because it is not per
/// repository -- `~/.claude/skills` is loaded into every session on the
/// machine, and attributing it to one checkout would be a wrong answer
/// repeated 38 times.
pub fn user_findings(home: &Path) -> Vec<Finding> {
    let mut out = Vec::new();

    let defs = crate::claude::definitions::scan_in(
        &home.join(".claude"),
        &crate::claude::definitions::Source::User,
    );
    for entry in &defs.unreadable {
        out.push(Finding {
            check: Check::Definition,
            // A directory behind a permission wall hides an unknown
            // number of definitions (`definitions.rs`'s own words), so it
            // is Unknown rather than Problem.
            severity: Severity::Unknown,
            path: entry.clone(),
            scope: None,
            proof: entry.clone(),
            undecidable_keys: Vec::new(),
        });
    }
    out
}

/// Sweep a set of repositories.
///
/// `repos` is supplied rather than discovered here, so the walk that
/// finds them can report its own shortfall separately -- and so this is
/// testable against a temp directory without a git checkout in it.
///
/// Linear in the number of repositories and, within each, linear in the
/// files its CLAUDE.md walk visits. Nothing here compares repositories
/// to each other, which is the shape that would make a 38-repository
/// sweep quadratic.
pub fn sweep_in(home: &Path, repos: &[(String, String)], unreadable_roots: Vec<String>) -> Sweep {
    let mut out = Sweep {
        repos: Vec::with_capacity(repos.len()),
        unreadable_roots,
        // Filled by the caller, which is the only layer that knows the
        // home directory is the SAME one it swept the repositories with.
        user_findings: Vec::new(),
    };
    for (_name, path) in repos {
        out.repos.push(check_repo(home, Path::new(path)));
    }
    rank(&mut out.repos);
    out
}

/// Worst first, then by name so the order is stable across runs.
///
/// A list that reshuffled between two sweeps would make a diff of them
/// unreadable -- the rule `scan_reporting` states for its own report.
fn rank(repos: &mut [RepoHealth]) {
    repos.sort_by(|a, b| {
        a.verdict
            .rank()
            .cmp(&b.verdict.rank())
            // More findings before fewer, within a verdict: a repository
            // with four dead scopes needs attention before one with a
            // single broken import.
            .then(b.findings.len().cmp(&a.findings.len()))
            .then(a.name.cmp(&b.name))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        (tmp, home)
    }

    /// Mandatory test 1: a genuinely malformed `settings.json` produces a
    /// finding whose PROOF carries the real parse location.
    ///
    /// The assertion is on "line" and "column" appearing in the proof,
    /// not on a canned string: the point of routing serde's own message
    /// through is that the location is real. A test that accepted any
    /// sentence would pass against a module that replaced the proof with
    /// "this config looks unusual", which is the exact failure #1217
    /// forbids.
    #[test]
    fn a_malformed_settings_file_proves_where_it_broke() {
        let (_t, home) = fixture();
        let repo = _t.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        // Valid up to line 3, then a bare word where a value belongs. The
        // location is therefore a real place in a real file rather than
        // byte zero.
        write(
            &repo.join(".claude").join("settings.json"),
            "{\n  \"model\": \"opus\",\n  \"env\": nope\n}\n",
        );

        let health = check_repo(&home, &repo);
        assert_eq!(health.verdict, Verdict::Problem);

        let f = health
            .findings
            .iter()
            .find(|f| f.check == Check::SettingsParse)
            .expect("a settings file that will not parse must produce a finding");
        assert_eq!(f.severity, Severity::Problem);
        assert_eq!(f.scope, Some(Origin::Project));
        assert!(
            f.proof.contains("line 3"),
            "the proof must carry the REAL parse location, got: {}",
            f.proof
        );
        assert!(
            f.proof.contains("column"),
            "the proof must carry the parse column, got: {}",
            f.proof
        );
        assert!(
            f.path.ends_with("settings.json"),
            "the finding must name the file to open, got: {}",
            f.path
        );
    }

    /// Mandatory test 2: a repository whose config directory cannot be
    /// read reports UNKNOWN, and the ranking does not place it among the
    /// passes.
    ///
    /// The two clean repositories are named so that one sorts BEFORE
    /// `walled` alphabetically and one after. Without that, a broken
    /// rank could be rescued by the name tie-break and the test would
    /// pass against the collapse it exists to catch -- sabotage-proven,
    /// and the first draft of this test was rescued exactly that way.
    ///
    /// Unix-only: the wall is made with a permission bit, and Windows
    /// does not honour one.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_directory_is_unknown_and_never_sorts_among_the_passes() {
        use std::os::unix::fs::PermissionsExt;

        let (_t, home) = fixture();
        let walled = _t.path().join("walled");
        // A subdirectory of the repository, so the CLAUDE.md walk tries
        // to list it and is refused. The repository root itself stays
        // readable, which is the realistic shape: one subtree behind a
        // wall, not a whole checkout.
        let blocked = walled.join("secrets");
        std::fs::create_dir_all(&blocked).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let clean_a = _t.path().join("aaa-clean");
        let clean_b = _t.path().join("zzz-clean");
        std::fs::create_dir_all(&clean_a).unwrap();
        std::fs::create_dir_all(&clean_b).unwrap();

        let repos = vec![
            ("aaa-clean".to_string(), clean_a.display().to_string()),
            ("zzz-clean".to_string(), clean_b.display().to_string()),
            ("walled".to_string(), walled.display().to_string()),
        ];
        let sweep = sweep_in(&home, &repos, Vec::new());

        // Restore before any assertion, so a failure does not leave an
        // unremovable directory behind for the temp dir's drop.
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();

        let walled_row = sweep
            .repos
            .iter()
            .find(|r| r.name == "walled")
            .expect("the walled repository must still appear");
        assert_eq!(
            walled_row.verdict,
            Verdict::Unknown,
            "a directory we could not list is not a repository we cleared"
        );
        assert!(
            walled_row
                .findings
                .iter()
                .any(|f| f.severity == Severity::Unknown && f.check == Check::Unreadable),
            "the unknown must be carried as a finding with its own reason"
        );

        // The ranking, which is the half that makes the distinction
        // visible. Unknown must come BEFORE every pass -- not merely be
        // labelled differently while sitting between two green rows.
        //
        // Asserted against the VERDICT rank directly as well as against
        // the emitted order. The order alone is not enough: the
        // findings-count tie-break happens to lift a repository with one
        // finding above two with none, so an order-only assertion passes
        // against a `rank()` that ties Unknown with Pass. That is a
        // rescue, not a proof, and it was observed -- so the rank is
        // pinned here too.
        assert!(
            Verdict::Unknown.rank() < Verdict::Pass.rank(),
            "an unknown that ranks equal to a pass will sort among the passes the moment              a tie-break stops rescuing it"
        );
        let walled_at = sweep.repos.iter().position(|r| r.name == "walled").unwrap();
        for (i, r) in sweep.repos.iter().enumerate() {
            if r.verdict == Verdict::Pass {
                assert!(
                    i > walled_at,
                    "{} passed and sorted above an unknown: an unknown ranked among the \
                     passes reads as 'checked, fine'",
                    r.name
                );
            }
        }
        assert_eq!(sweep.unknown(), 1);
        assert_eq!(sweep.passed(), 2);
        assert_eq!(sweep.problems(), 0);
    }

    /// Mandatory test 3: a finding names the specific failing scope AND
    /// the keys that became undecidable -- not a flattened boolean.
    ///
    /// The fixture is the one that makes the distinction bite: the USER
    /// file is readable and sets `model`, and the LOCAL file -- which
    /// outranks it -- is the one that will not parse. So the scope named
    /// must be `Local` and `model` must be listed as undecidable,
    /// because what the broken file would have said about it is unknown.
    #[test]
    fn a_finding_names_the_failing_scope_and_the_keys_it_put_in_doubt() {
        let (_t, home) = fixture();
        let repo = _t.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        write(
            &home.join(".claude").join("settings.json"),
            r#"{"model": "sonnet"}"#,
        );
        write(&repo.join(".claude").join("settings.local.json"), "{ nope");

        let health = check_repo(&home, &repo);
        let f = health
            .findings
            .iter()
            .find(|f| f.check == Check::SettingsParse)
            .expect("the unparseable local scope must produce a finding");

        assert_eq!(
            f.scope,
            Some(Origin::Local),
            "the finding must name WHICH file to fix, since that is the whole remedy"
        );
        assert!(f.path.ends_with("settings.local.json"), "got: {}", f.path);
        assert!(
            f.undecidable_keys.iter().any(|k| k == "model"),
            "a readable user file set `model` and the broken file outranks it, so `model` \
             is undecidable and the finding must say so; got: {:?}",
            f.undecidable_keys
        );
        // And the user scope, which is readable, produced NO finding of
        // its own: flattening three scopes into one repository-level
        // indicator is what this asserts against.
        assert_eq!(
            health
                .findings
                .iter()
                .filter(|f| f.check == Check::SettingsParse)
                .count(),
            1,
            "only the scope that actually failed may be reported"
        );
    }

    /// When TWO scopes refuse, each finding claims only the keys ITS
    /// OWN scope put in doubt.
    ///
    /// The case that exposes constraint 3's defect one level down. The
    /// first draft filtered on `k.undecidable` alone, so both findings
    /// listed every undecidable key -- an unreadable USER file taking
    /// credit for a key only the LOCAL file could have overridden, which
    /// sends the reader to the wrong file. Exactly the flattening the
    /// per-scope model exists to prevent.
    ///
    /// The fixture: PROJECT readably sets `model`, and both USER (which
    /// it outranks) and LOCAL (which outranks it) refuse. So `model` is
    /// undecidable because of LOCAL and NOT because of USER.
    #[test]
    fn two_refusing_scopes_do_not_share_each_others_undecidable_keys() {
        let (_t, home) = fixture();
        let repo = _t.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        write(&home.join(".claude").join("settings.json"), "{ broken");
        write(
            &repo.join(".claude").join("settings.json"),
            r#"{"model": "sonnet"}"#,
        );
        write(
            &repo.join(".claude").join("settings.local.json"),
            "{ also broken",
        );

        let health = check_repo(&home, &repo);
        let find = |o: Origin| {
            health
                .findings
                .iter()
                .find(|f| f.scope == Some(o))
                .unwrap_or_else(|| panic!("{o:?} refused, so it must have a finding"))
        };

        // BOTH scopes are reported: each is a separate inert file with
        // its own remedy.
        assert_eq!(
            health
                .findings
                .iter()
                .filter(|f| f.check == Check::SettingsParse)
                .count(),
            2
        );

        assert!(
            find(Origin::Local)
                .undecidable_keys
                .iter()
                .any(|k| k == "model"),
            "the local file outranks the readable project value, so it is what puts \
             `model` in doubt; got: {:?}",
            find(Origin::Local).undecidable_keys
        );
        assert!(
            !find(Origin::User)
                .undecidable_keys
                .iter()
                .any(|k| k == "model"),
            "an unreadable USER file cannot override a key the PROJECT file already \
             decided, so claiming `model` here would send the reader to the wrong \
             file; got: {:?}",
            find(Origin::User).undecidable_keys
        );
    }

    /// A broken CLAUDE.md import is a finding, and its proof is the
    /// resolver's own sentence rather than a summary.
    #[test]
    fn a_broken_claude_md_import_is_a_finding_with_the_resolvers_own_words() {
        let (_t, home) = fixture();
        let repo = _t.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("CLAUDE.md"), "# Rules\n\n@./missing-file.md\n").unwrap();

        let health = check_repo(&home, &repo);
        let f = health
            .findings
            .iter()
            .find(|f| f.check == Check::ClaudeMdImport)
            .expect("an import that resolves to nothing must be reported");
        assert_eq!(f.severity, Severity::Problem);
        assert_eq!(f.scope, None, "an import belongs to no settings scope");
        assert!(
            f.proof.contains("missing-file.md"),
            "the proof must name the import that failed, got: {}",
            f.proof
        );
        assert_eq!(health.verdict, Verdict::Problem);
    }

    /// A repository with nothing wrong passes, and passing means every
    /// check RAN.
    #[test]
    fn a_clean_repository_passes_with_no_findings() {
        let (_t, home) = fixture();
        let repo = _t.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        write(
            &repo.join(".claude").join("settings.json"),
            r#"{"model": "opus"}"#,
        );
        std::fs::write(repo.join("CLAUDE.md"), "# Rules\n").unwrap();

        let health = check_repo(&home, &repo);
        assert_eq!(health.verdict, Verdict::Pass);
        assert!(health.findings.is_empty(), "{:?}", health.findings);
    }

    /// Verdict ordering, asserted directly rather than only through a
    /// fixture: Unknown sits between Problem and Pass and is equal to
    /// neither.
    #[test]
    fn unknown_ranks_between_problem_and_pass() {
        assert!(Verdict::Problem.rank() < Verdict::Unknown.rank());
        assert!(Verdict::Unknown.rank() < Verdict::Pass.rank());
    }

    /// A MEASURED sweep over this machine's real scan root, so "small to
    /// medium" is a number rather than an assumption.
    ///
    /// `#[ignore]` because it reads the developer's `~/code` and would
    /// fail or mislead in CI, where that directory does not exist. Run
    /// with `cargo test -- --ignored measured_sweep_cost`.
    #[test]
    #[ignore]
    fn measured_sweep_cost_over_this_machines_repositories() {
        let Some(home) = crate::auth::home_dir() else {
            return;
        };
        let root = home.join("code");
        if !root.is_dir() {
            return;
        }
        let dirs = vec![root.display().to_string()];
        let census = std::time::Instant::now();
        let scan = crate::worktrees::scan_dirs_fast_reporting(&dirs);
        let census_ms = census.elapsed().as_millis();
        let repos: Vec<(String, String)> = scan
            .repos
            .iter()
            .map(|r| (r.name.clone(), r.path.clone()))
            .collect();

        // The two halves timed apart, so the cost is attributed rather
        // than guessed at: the settings check is three file reads per
        // repository and the CLAUDE.md check is a directory walk.
        let s_only = std::time::Instant::now();
        for (_n, p) in &repos {
            let mut v = Vec::new();
            settings_findings(&home, Path::new(p), &mut v);
        }
        let settings_ms = s_only.elapsed().as_millis();
        let m_only = std::time::Instant::now();
        for (_n, p) in &repos {
            let mut v = Vec::new();
            claude_md_findings(Path::new(p), &mut v);
        }
        let md_ms = m_only.elapsed().as_millis();
        println!("MEASURED split: settings {settings_ms} ms, claude.md walk {md_ms} ms");

        let t = std::time::Instant::now();
        let sweep = sweep_in(&home, &repos, scan.unreadable);
        let ms = t.elapsed().as_millis();
        println!(
            "MEASURED: census {} repos in {census_ms} ms; sweep in {ms} ms; \
             {} broken, {} unknown, {} clean",
            repos.len(),
            sweep.problems(),
            sweep.unknown(),
            sweep.passed(),
        );
        for r in sweep.repos.iter().filter(|r| r.verdict != Verdict::Pass) {
            println!(
                "  {} [{:?}] {} finding(s)",
                r.name,
                r.verdict,
                r.findings.len()
            );
            for f in &r.findings {
                println!("      {:?} {}", f.check, f.proof);
            }
        }
    }

    /// The census shortfall travels with the sweep (#1025).
    #[test]
    fn a_short_census_is_reported_beside_the_repositories_that_were_read() {
        let (_t, home) = fixture();
        let repo = _t.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let sweep = sweep_in(
            &home,
            &[("repo".to_string(), repo.display().to_string())],
            vec!["/some/root: permission denied".to_string()],
        );
        assert!(sweep.is_partial());
        assert_eq!(sweep.passed(), 1);
        assert_eq!(sweep.unreadable_roots.len(), 1);
    }
}
