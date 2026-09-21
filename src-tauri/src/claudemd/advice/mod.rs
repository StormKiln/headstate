//! Advice about a repository's CLAUDE.md files: one finding model, one
//! run, one brief.
//!
//! # This module is opinion; `confighealth` is not
//!
//! `claude/confighealth.rs:16-30` states that module's rule: "Findings
//! are CHECKS, never opinions … no heuristic, no style judgement". A text
//! match for `make test` is a heuristic by construction, and so is every
//! producer that will register here. So advice is a SIBLING of
//! `confighealth` that borrows its shape -- a carried [`Check`], a ranked
//! [`Severity`], verbatim evidence -- and adds a third severity,
//! [`Severity::Advice`], that `confighealth` must never grow. Mixing the
//! two would teach a reader to skim both, and an ignored health page is
//! worse than none because it looks like coverage.
//!
//! # One model, one command, one panel
//!
//! Every producer emits [`Finding`]s of this one type, runs under the one
//! `claude_md_advice` command, and renders in the one
//! `ClaudeMdAdvicePanel`. A producer never ships its own panel. This is
//! the seam the `epic` skill warns about: #1038 built a table and #1039
//! built its host, both green, nothing mounted. The seed producer
//! ([`imports`]) computes nothing new -- it re-states the `ImportNode`
//! problems the scan already carries -- so the panel renders a real row
//! before any other producer exists.
//!
//! # A producer that fails is reported, never dropped
//!
//! [`run`] turns a producer's `Err` into [`CheckRun::Unknown`] and the
//! other producers' findings stand (#1044: partial is not nothing). No
//! `tokio::time::timeout` sits anywhere on this path, because it drops the
//! future and everything it retrieved. A producer that needs a bound
//! bounds itself and reports what it covered.
//!
//! [`Report::checks`] is derived from [`Check::ALL`], so a check that has
//! no producer registered appears as Unknown rather than being silently
//! absent; a unit test below asserts `ALL` against the enum's own source
//! block (the "derived, not enumerated" rule in `invariants.rs`).
//!
//! # There is no Pending
//!
//! Pending is the absence of a [`Report`]: the query is in flight and the
//! panel shows a skeleton. `CheckRun` has exactly two states, ran and
//! Unknown, because a third that nothing moves out of is #1042.
//!
//! # What the [`Context`] carries
//!
//! `definitions` and `conn` are `Option`: `claude_md_advice` builds the
//! inventory and opens the store once per run, and [`report_in`] exists
//! for the callers that have neither (tests, and a run over a bare
//! checkout). The rot and skills producers read `definitions`; the
//! transcripts producer reads `conn`, and the gaps producer reads the
//! edited-directory signal through it. A producer that needs one and
//! finds `None` returns `Err`, which is reported as Unknown with that
//! reason rather than as a clean pass; one for which the missing input
//! is only a weaker answer says so in a finding.
//!
//! `home` is `Option` for the same reason `claude_md_effective` tolerates
//! a missing home: the repository scan is still a real answer. The scan's
//! own `unreadable` entry records the missing global scope, and a
//! producer that needs the home directory says so in its `Err`.
//!
//! Read-only. Nothing here writes to a CLAUDE.md or a skill; the brief is
//! text for an agent, and its last line is the read-only policy.

pub mod brief;
pub mod cache;
pub mod gaps;
pub mod imports;
pub mod placement;
pub mod rot;
pub mod shape;
pub mod skills;
pub mod toolchain;
pub mod transcripts;

use serde::{Deserialize, Serialize};
use std::path::Path;

use super::{EffectiveScan, Scope};
use crate::claude::definitions::Inventory;

/// Which producer made a finding. One variant per producer.
///
/// Carried rather than inferred from the finding's wording, for
/// `confighealth::Check`'s reason: a frontend that pattern-matched a
/// sentence would break the first time one was reworded.
///
/// `ALL` is asserted against this enum's source block by
/// `all_lists_every_variant_exactly_once`, so a variant added without an
/// `ALL` entry fails a test rather than vanishing from every report's
/// coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Check {
    /// A CLAUDE.md `@import` that is broken, circular or unreadable. The
    /// seed producer: it re-states `ImportNode::problem`.
    Imports,
    /// A build system on disk whose build, test, lint, format, run or
    /// deploy command no loaded CLAUDE.md names; and a code-style line a
    /// formatter config already decides.
    Toolchain,
    /// What recurred in the sessions recorded under the repository -- a
    /// corrected command, a user correction, a denial, a repeated search
    /// or error -- and whether a CLAUDE.md on the path already states it.
    Transcripts,
    /// A directory with a toolchain, tests or role of its own and no
    /// CLAUDE.md between it and the root.
    Gaps,
    /// A section whose named paths all fall under one subdirectory,
    /// content duplicated across the files one session loads, or an
    /// all-caps rule in a file that loads lazily.
    Placement,
    /// A reference in a CLAUDE.md that resolves to nothing, or a line
    /// past a file's end.
    Rot,
    /// Skills beside the CLAUDE.md files: frontmatter over a documented
    /// limit, a CLAUDE.md naming a skill no scope holds, a procedure a
    /// skill already holds, and cost.
    Skills,
    /// Content shape: the rules from published guidance that a
    /// deterministic check can enforce over one file or one launch set.
    Shape,
}

impl Check {
    /// Every check, in the order a report lists them.
    pub const ALL: &'static [Check] = &[
        Check::Imports,
        Check::Toolchain,
        Check::Transcripts,
        Check::Gaps,
        Check::Placement,
        Check::Rot,
        Check::Skills,
        Check::Shape,
    ];

    /// The check's name as the brief prints it.
    pub fn name(self) -> &'static str {
        match self {
            Check::Imports => "imports",
            Check::Toolchain => "toolchain",
            Check::Transcripts => "transcripts",
            Check::Gaps => "gaps",
            Check::Placement => "placement",
            Check::Rot => "rot",
            Check::Skills => "skills",
            Check::Shape => "shape",
        }
    }
}

/// How sure a finding is, ranked worst first like `confighealth::Verdict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    /// Proven broken. The evidence says how.
    Problem,
    /// A heuristic fired. The evidence says what it measured; the reader
    /// decides whether the rule applies.
    Advice,
    /// Could not be decided. The evidence says why not.
    Unknown,
}

impl Severity {
    /// Sort order, worst first: Problem, then Advice, then Unknown.
    ///
    /// Unknown ranks ABOVE nothing and never among clean: an Unknown
    /// sorted after the findings would read as the least important
    /// thing on the list, when it is the one thing the list cannot
    /// vouch for.
    pub fn rank(self) -> u8 {
        match self {
            Severity::Problem => 0,
            Severity::Advice => 1,
            Severity::Unknown => 2,
        }
    }
}

/// What a finding is about.
///
/// An enum because the skills producer's subject is not a CLAUDE.md and
/// the gaps producer's subject is a directory that has no file yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Subject {
    /// A CLAUDE.md, and the section of it when the finding is about one.
    #[serde(rename_all = "camelCase")]
    ClaudeMd {
        /// Absolute, as `ClaudeFile::path` is, so the panel can match it
        /// against the file the page is showing.
        path: String,
        scope: Scope,
        /// The heading as written, e.g. `## Platform`, when the finding is
        /// about one section rather than the whole file.
        section: Option<String>,
    },
    /// A directory, for a finding about a file that does not exist yet.
    #[serde(rename_all = "camelCase")]
    Directory { path: String },
    /// A skill's `SKILL.md`, by path and by the name it is invoked with.
    #[serde(rename_all = "camelCase")]
    Skill { path: String, name: String },
}

impl Subject {
    /// The path the subject names, whatever its kind.
    pub fn path(&self) -> &str {
        match self {
            Subject::ClaudeMd { path, .. }
            | Subject::Directory { path }
            | Subject::Skill { path, .. } => path,
        }
    }
}

/// Where a piece of evidence is.
///
/// A session locator carries a record index, never user text: the wire
/// carries keys, counts and ids, and quoted text stays behind a click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Locator {
    /// A file, and a line in it when the producer knows one.
    ///
    /// `line: None` is "no line is known", and the brief prints the path
    /// alone. A producer must never invent a line to fill this in.
    #[serde(rename_all = "camelCase")]
    File { path: String, line: Option<u32> },
    /// A transcript, and the record index in it when known.
    #[serde(rename_all = "camelCase")]
    Session {
        session_id: String,
        record: Option<u64>,
    },
}

/// One measurement behind a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub at: Locator,
    /// The producer's own measurement, verbatim: "4 of 4 bullets name
    /// paths under `src-tauri/`". Never a judgement.
    pub measured: String,
}

/// One thing a producer found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub check: Check,
    pub severity: Severity,
    pub subject: Subject,
    pub evidence: Vec<Evidence>,
    /// One sentence, a fact. Rendered as the row.
    pub finding: String,
    /// Markdown for an agent, rendered by [`brief::render`] at
    /// construction so it can never disagree with the fields above.
    pub brief: String,
    /// The producer's own rule, when its check has more than one and the
    /// brief's suggestion differs per rule (`shape` has ten). Set at
    /// construction by [`Finding::with_rule`] and read only by
    /// [`brief::render`]. Never on the wire: the panel keys on `check`,
    /// and a rule id is an internal.
    #[serde(skip)]
    pub rule: Option<&'static str>,
}

impl Finding {
    /// Build a finding and render its brief.
    ///
    /// The constructor producers use (with [`Finding::with_rule`] for a
    /// producer that has more than one rule): a `Finding` built by hand
    /// could carry a `brief` that names a different subject than its
    /// `subject` field, and the panel copies the brief without reading
    /// it.
    pub fn new(
        check: Check,
        severity: Severity,
        subject: Subject,
        evidence: Vec<Evidence>,
        finding: String,
    ) -> Finding {
        Self::build(check, None, severity, subject, evidence, finding)
    }

    /// [`Finding::new`] carrying the producer's rule id, so the brief's
    /// suggestion is the rule's rather than the check's.
    pub fn with_rule(
        check: Check,
        rule: &'static str,
        severity: Severity,
        subject: Subject,
        evidence: Vec<Evidence>,
        finding: String,
    ) -> Finding {
        Self::build(check, Some(rule), severity, subject, evidence, finding)
    }

    fn build(
        check: Check,
        rule: Option<&'static str>,
        severity: Severity,
        subject: Subject,
        evidence: Vec<Evidence>,
        finding: String,
    ) -> Finding {
        let mut out = Finding {
            check,
            severity,
            subject,
            evidence,
            finding,
            brief: String::new(),
            rule,
        };
        out.brief = brief::render(&out);
        out
    }
}

/// Whether a check ran.
///
/// Two states and no third: "not yet" is the absence of the whole
/// [`Report`], never a variant here (#1042).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum CheckRun {
    /// The producer ran to completion and emitted this many findings.
    #[serde(rename_all = "camelCase")]
    Ran { findings: usize },
    /// The producer could not run, in its own words.
    #[serde(rename_all = "camelCase")]
    Unknown { reason: String },
}

/// One check's coverage in a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckCoverage {
    pub check: Check,
    pub run: CheckRun,
}

/// Everything one run of the producers found, and which checks ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// The repository the run was over, as the caller named it.
    pub repo: String,
    /// In [`Severity::rank`] order, stable within a rank. The frontend
    /// never re-sorts.
    pub findings: Vec<Finding>,
    /// One entry per [`Check::ALL`], in that order.
    pub checks: Vec<CheckCoverage>,
    /// Every finding's brief plus a `_Could not check: {reason}_` line
    /// per Unknown check, rendered by [`brief::render_report`].
    pub brief: String,
}

impl Report {
    /// Whether any check could not run.
    pub fn is_partial(&self) -> bool {
        self.checks
            .iter()
            .any(|c| matches!(c.run, CheckRun::Unknown { .. }))
    }

    /// How many checks ran to completion.
    pub fn ran(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| matches!(c.run, CheckRun::Ran { .. }))
            .count()
    }
}

/// Why a caller is asking, and therefore whether a cached report may be
/// served (#1293).
///
/// A parameter rather than a heuristic inside the command, because the
/// two callers want opposite things and neither can be guessed from the
/// arguments: opening a repository wants an answer NOW and will accept a
/// previous run, while pressing Refresh wants the producers to run. A
/// command that decided for itself would make Refresh a no-op on a
/// repository whose inputs have not moved -- which is exactly when a
/// user presses it, because the last run said something they want
/// re-checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Serve the stored report when the tracked inputs still match, else
    /// run the producers. The default, and what a repository selection
    /// uses.
    #[default]
    Cached,
    /// Run the producers regardless, and replace what is stored. What
    /// Refresh uses, and what the second half of a two-phase read uses
    /// after a `Cached` call came back not [`Freshness::Fresh`].
    Fresh,
}

/// Where a served report came from, and whether it can be called current.
///
/// THREE states that must never collapse into two (#846, #1042, #1044).
/// The distinction that matters is not "cached vs not" -- it is whether
/// the app can HONESTLY claim the report describes the repository as it
/// is now:
///
/// - [`Freshness::Fresh`] -- the producers ran during this call, or the
///   stored report's fingerprint was recomputed in full and matched. In
///   both cases every tracked input was read.
/// - [`Freshness::Cached`] -- the stored report is being served and a
///   run has NOT happened. `stale` says whether the fingerprint
///   disagreed, which is what tells a caller a refresh is worth making.
/// - [`Freshness::Unverified`] -- a tracked input could not be read, so
///   the fingerprint is not a statement about currency in either
///   direction. This is NOT `Fresh` with a footnote and NOT `Cached`
///   with a shrug: a matching digest here proves nothing, because the
///   digest omitted something both times.
///
/// The third state is reachable from both sides, which is why it is a
/// variant rather than a flag on `Cached`: a run that JUST happened can
/// also be unverified, if a CLAUDE.md the scan listed could not be read
/// when the fingerprint was taken. Calling that run "fresh" would be the
/// same lie one open later.
///
/// "From cache, refreshing" -- the epic's second state -- is the caller
/// holding a [`Freshness::Cached`] result while a [`Mode::Fresh`] call
/// is in flight. It is deliberately NOT a variant here: a single
/// synchronous call cannot be both the cached answer and the running
/// one, and a backend variant saying "a refresh is happening" would be a
/// claim about a future this call cannot observe. What the backend owes
/// the caller is the fact that it served cache and whether that cache
/// is stale, and those are both here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Freshness {
    /// Computed during this call, or the cache's fingerprint was
    /// verified against every tracked input and matched.
    #[serde(rename_all = "camelCase")]
    Fresh {
        /// Whether the producers ran during this call, as opposed to the
        /// stored report being verified current. Both are fresh; only
        /// one of them cost the transcript read.
        recomputed: bool,
    },
    /// Served from the store without running the producers.
    #[serde(rename_all = "camelCase")]
    Cached {
        /// Whether a tracked input has changed since the report was
        /// computed. `true` means the report is a PREVIOUS answer and a
        /// [`Mode::Fresh`] call would produce a different one.
        stale: bool,
    },
    /// Currency could not be established, in the fingerprint's own
    /// words. Says nothing about whether the report is right.
    #[serde(rename_all = "camelCase")]
    Unverified {
        reason: String,
        /// Whether the producers ran during this call. An unverified run
        /// that JUST happened is the best available answer and still not
        /// a current one.
        recomputed: bool,
    },
}

/// A [`Report`] and the honest account of where it came from.
///
/// A struct rather than widening `Report`, because `Report` is what a
/// run PRODUCES and this is what a call SERVES: a report stored in
/// January and read in March is the same report and a different
/// freshness. Keeping them apart is also why the cached payload holds a
/// bare `Report` -- the freshness is recomputed on every read from the
/// fingerprint, never replayed from the row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdviceResult {
    pub report: Report,
    pub freshness: Freshness,
    /// RFC 3339, when the PRODUCERS ran -- not when this call answered.
    /// For a cached result this is older than now, which is the whole
    /// point of showing it.
    pub computed_at: String,
}

/// What every producer may read. Built once per run, so the CLAUDE.md
/// walk happens once rather than once per producer (#1246 is what a
/// second walk costs).
pub struct Context<'a> {
    pub repo: &'a Path,
    /// `None` when no home directory is set. The scan already records
    /// the missing global scope; a producer that needs the home returns
    /// `Err` and is reported Unknown.
    pub home: Option<&'a Path>,
    pub scan: &'a EffectiveScan,
    /// The definitions inventory, once the skills producer wires it. See
    /// the module docs for why this is `Option` today.
    pub definitions: Option<&'a Inventory>,
    /// Headstate's own store, once the transcripts producer wires it.
    pub conn: Option<&'a rusqlite::Connection>,
}

/// One producer: one [`Check`], one run.
///
/// `Sync` so a `&dyn Producer` can sit in the [`PRODUCERS`] static.
pub trait Producer: Sync {
    fn check(&self) -> Check;
    /// `Err` becomes [`CheckRun::Unknown`] with the string as its reason.
    /// The reason is shown to the reader verbatim, so it states a fact
    /// and stops: no "so that…" tail.
    fn run(&self, cx: &Context) -> Result<Vec<Finding>, String>;
}

/// Every registered producer, in run order.
pub static PRODUCERS: &[&dyn Producer] = &[
    &imports::Imports,
    &toolchain::Coverage,
    &transcripts::Transcripts,
    &gaps::Gaps,
    &placement::Placement,
    &rot::Rot,
    &skills::Skills,
    &shape::Shape,
];

/// Run every registered producer and assemble the report.
pub fn run(cx: &Context) -> Report {
    run_with(cx, PRODUCERS)
}

/// [`run`] over an explicit producer list, so a test can register a
/// producer that fails.
fn run_with(cx: &Context, producers: &[&dyn Producer]) -> Report {
    let mut findings = Vec::new();
    let mut runs: Vec<(Check, CheckRun)> = Vec::new();

    for p in producers {
        let check = p.check();
        // An `Err` is a coverage fact, not a rejection. The other
        // producers' findings stand beside it.
        let run = match p.run(cx) {
            Ok(found) => {
                let n = found.len();
                findings.extend(found);
                CheckRun::Ran { findings: n }
            }
            Err(reason) => CheckRun::Unknown { reason },
        };
        runs.push((check, run));
    }

    // Stable, so two findings of one severity keep the order their
    // producer emitted them in, which is the order the reader can
    // predict.
    findings.sort_by_key(|f| f.severity.rank());

    // Derived from `ALL`, never from the producer list: a check whose
    // producer was forgotten is reported as unknown rather than absent.
    let checks = Check::ALL
        .iter()
        .map(|&check| CheckCoverage {
            check,
            run: runs
                .iter()
                .find(|(c, _)| *c == check)
                .map(|(_, r)| r.clone())
                .unwrap_or(CheckRun::Unknown {
                    reason: "no producer is registered for this check".to_string(),
                }),
        })
        .collect();

    let mut report = Report {
        repo: cx.repo.to_string_lossy().to_string(),
        findings,
        checks,
        brief: String::new(),
    };
    report.brief = brief::render_report(&report);
    report
}

/// Scan the repository and run every producer over it.
///
/// `home` is a parameter for `confighealth::check_repo`'s reason: a test
/// that changes `$HOME` races every other test in the binary. `None` is
/// tolerated the way `claude_md_effective` tolerates it -- the repo scan
/// is still a real answer, and the scan records the scope it could not
/// look for.
///
/// `definitions` is built by the command, once, for the same reason the
/// scan is: the inventory walks the user, project and plugin roots. A
/// producer that needs it and gets `None` reports itself Unknown, so a
/// skill reference is then "could not check", never "missing".
pub fn report_in(repo: &Path, home: Option<&Path>, definitions: Option<&Inventory>) -> Report {
    let scan = super::scan_effective_opt(repo, home);
    let cx = Context {
        repo,
        home,
        scan: &scan,
        definitions,
        conn: None,
    };
    run(&cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_scan() -> EffectiveScan {
        EffectiveScan::default()
    }

    fn context<'a>(repo: &'a Path, scan: &'a EffectiveScan) -> Context<'a> {
        Context {
            repo,
            home: None,
            scan,
            definitions: None,
            conn: None,
        }
    }

    /// A producer that always fails, to prove an `Err` is coverage
    /// rather than a rejection.
    struct Failing;
    impl Producer for Failing {
        fn check(&self) -> Check {
            Check::Imports
        }
        fn run(&self, _cx: &Context) -> Result<Vec<Finding>, String> {
            Err("the transcripts directory could not be listed: Permission denied".into())
        }
    }

    /// A producer that emits findings out of rank order, to prove the
    /// report sorts and the sort is stable.
    struct Emits(Vec<Finding>);
    impl Producer for Emits {
        fn check(&self) -> Check {
            Check::Imports
        }
        fn run(&self, _cx: &Context) -> Result<Vec<Finding>, String> {
            Ok(self.0.clone())
        }
    }

    fn finding(severity: Severity, sentence: &str) -> Finding {
        Finding::new(
            Check::Imports,
            severity,
            Subject::ClaudeMd {
                path: "/home/octocat/hello-world/CLAUDE.md".into(),
                scope: Scope::Repo,
                section: None,
            },
            vec![Evidence {
                at: Locator::File {
                    path: "/home/octocat/hello-world/CLAUDE.md".into(),
                    line: None,
                },
                measured: "`@./missing.md`: file not found".into(),
            }],
            sentence.into(),
        )
    }

    /// The "derived, not enumerated" rule from `invariants.rs`, applied
    /// to `Check::ALL`: the variant count is read from this file's own
    /// source block, so a variant added without an `ALL` entry cannot be
    /// silently absent from every report's coverage.
    #[test]
    fn all_lists_every_variant_exactly_once() {
        let src = include_str!("mod.rs").replace("\r\n", "\n");
        let (_, after) = src
            .split_once("pub enum Check {")
            .expect("the Check enum's source block");
        let (block, _) = after
            .split_once("\n}")
            .expect("the enum's closing brace at column 0");
        let variants: Vec<&str> = block
            .lines()
            .map(str::trim)
            // Doc comments and attributes are not variants.
            .filter(|l| !l.starts_with("//") && !l.starts_with("#["))
            .filter(|l| {
                l.ends_with(',') && l.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            })
            .collect();
        assert!(
            !variants.is_empty(),
            "the scan found no variants, so its idea of the enum's shape is stale"
        );
        assert_eq!(
            Check::ALL.len(),
            variants.len(),
            "Check::ALL has {} entries but the enum declares {}: {variants:?}",
            Check::ALL.len(),
            variants.len()
        );
        for (i, a) in Check::ALL.iter().enumerate() {
            assert!(
                !Check::ALL[..i].contains(a),
                "{a:?} is listed twice in Check::ALL"
            );
        }
    }

    /// Every check has a producer registered, so a report never carries
    /// the "no producer is registered" placeholder in production.
    #[test]
    fn every_check_has_a_registered_producer() {
        for check in Check::ALL {
            assert!(
                PRODUCERS.iter().any(|p| p.check() == *check),
                "{check:?} has no producer in PRODUCERS"
            );
        }
    }

    /// Partial is not nothing (#1044). A producer's `Err` is one
    /// check's coverage, and the other producers' findings stand.
    #[test]
    fn a_failing_producer_is_reported_unknown_not_dropped() {
        let scan = fixture_scan();
        let repo = Path::new("/home/octocat/hello-world");
        let cx = context(repo, &scan);
        let kept = finding(Severity::Problem, "a real finding from another producer");
        let producers: [&dyn Producer; 2] = [&Failing, &Emits(vec![kept.clone()])];

        let report = run_with(&cx, &producers);

        // The failure is reported, in the producer's own words.
        let unknown = report
            .checks
            .iter()
            .find(|c| matches!(c.run, CheckRun::Unknown { .. }))
            .expect("the failing producer is reported as unknown");
        match &unknown.run {
            CheckRun::Unknown { reason } => assert!(reason.contains("Permission denied")),
            other => panic!("{other:?}"),
        }
        assert!(report.is_partial());
        // And the other producer's finding was not thrown away with it.
        assert_eq!(report.findings, vec![kept], "{report:?}");
        assert!(
            report
                .brief
                .contains("_Could not check: the transcripts directory"),
            "the brief states the failure: {}",
            report.brief
        );
    }

    /// `Report.checks` is derived from `Check::ALL`, so every check
    /// appears exactly once whatever the producers did.
    #[test]
    fn the_report_lists_every_check_exactly_once() {
        let scan = fixture_scan();
        let repo = Path::new("/home/octocat/hello-world");
        let cx = context(repo, &scan);

        let report = run(&cx);
        let listed: Vec<Check> = report.checks.iter().map(|c| c.check).collect();
        assert_eq!(listed, Check::ALL.to_vec());

        // The same over an EMPTY producer list: nothing ran, and the
        // report says so per check rather than listing nothing.
        let none = run_with(&cx, &[]);
        assert_eq!(none.checks.len(), Check::ALL.len());
        assert!(none.is_partial(), "an unrun check is not a clean one");
        assert_eq!(none.ran(), 0);
    }

    /// Findings are sorted by `Severity::rank`, and the sort is stable so
    /// findings of one severity keep their producer's order.
    #[test]
    fn findings_are_ranked_worst_first_and_stably() {
        let scan = fixture_scan();
        let repo = Path::new("/home/octocat/hello-world");
        let cx = context(repo, &scan);
        let emitted = vec![
            finding(Severity::Advice, "advice one"),
            finding(Severity::Unknown, "unknown"),
            finding(Severity::Problem, "problem"),
            finding(Severity::Advice, "advice two"),
        ];
        let producers: [&dyn Producer; 1] = [&Emits(emitted)];

        let report = run_with(&cx, &producers);
        let order: Vec<&str> = report.findings.iter().map(|f| f.finding.as_str()).collect();
        assert_eq!(order, ["problem", "advice one", "advice two", "unknown"]);
    }

    /// The wire shape the TypeScript mirror is written against: camelCase
    /// fields and `kind`/`state` tags. A drift here is a runtime `undefined`
    /// in the panel with nothing to say so.
    #[test]
    fn the_wire_shape_is_tagged_and_camel_cased() {
        let f = Finding::new(
            Check::Imports,
            Severity::Problem,
            Subject::Skill {
                path: "/home/octocat/hello-world/.claude/skills/verify/SKILL.md".into(),
                name: "verify".into(),
            },
            vec![Evidence {
                at: Locator::Session {
                    session_id: "abc".into(),
                    record: Some(4),
                },
                measured: "seen in 3 sessions".into(),
            }],
            "a sentence".into(),
        );
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["check"], "imports");
        assert_eq!(json["severity"], "problem");
        assert_eq!(json["subject"]["kind"], "skill");
        assert_eq!(json["evidence"][0]["at"]["kind"], "session");
        assert_eq!(json["evidence"][0]["at"]["sessionId"], "abc");
        let run = serde_json::to_value(CheckRun::Ran { findings: 2 }).unwrap();
        assert_eq!(run["state"], "ran");
        assert_eq!(run["findings"], 2);
    }
}
