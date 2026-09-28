//! Rot: what a CLAUDE.md names that no longer exists.
//!
//! A CLAUDE.md is prose that points at code, and the code moves. Every
//! reference `refs::extract` finds is resolved against the repository,
//! and a reference that resolves to nothing is a [`Verdict::Missing`]
//! finding at [`Severity::Problem`]. Measured on this repository's own
//! three files before this producer existed: 16 path references (4 only
//! by unique suffix), 2 `path:line`, 2 `make` targets, 2 `yarn` (one a
//! script, one a binary), 4 skill names, 18 qualified symbols, 10 issue
//! numbers; zero certain rot. The check exists for the day
//! `src/lib/target.ts` is renamed, which `src/CLAUDE.md` names three
//! times.
//!
//! # What each kind resolves against
//!
//! - **path**: the CLAUDE.md's own directory, then the repository root,
//!   then a unique suffix match over the tree, walked with the same
//!   [`SKIP`] list `scan_repo` uses. A nested CLAUDE.md describes its
//!   own directory, so when any suffix match lies under that directory
//!   only those count: measured, a nested e2e file's `helpers/` matched
//!   7 paths across sibling apps and exactly one under its own (#1319).
//!   With none under it, the whole tree counts; for the root file the
//!   two are the same. Two or more matches left are then settled by the
//!   reference's own line, or its list item when it sits in one (#1372):
//!   measured, "c specs and `src/testing`" matched 4 paths, and the
//!   prose word `c` is a segment of only `apps/c/src/testing`. Only
//!   prose words count -- another code span is another reference -- and
//!   only a candidate's segments before the shared suffix; a word every
//!   candidate has settles nothing. When the words hit exactly one
//!   candidate it resolves, and any finding it goes on to produce names
//!   the word in its evidence. None or several is `Unknown`
//!   ("ambiguous"), never a guess. A path that lies UNDER a pruned
//!   directory is `Unknown` naming the prune, never `Missing`: the walk
//!   did not enter it, and #1299 is what one prune reading as absence
//!   costs. Neither this walk nor `scan_repo` prunes `.claude` for
//!   holding agent worktrees -- `SKIP` already prunes the `worktrees`
//!   child by name, and the parent holds the rules and skills a
//!   CLAUDE.md names. A single-segment token that resolves to no file
//!   is then looked up in the `package.json` dependency tables before
//!   it is called missing: `chart.js` is a declared dependency, not a
//!   file, and shape cannot separate the two, since plenty of real
//!   files are named `something.js` (#1300). A declared dependency
//!   resolves and is silent. A manifest that EXISTS and could not be
//!   read makes it `Unknown`, because "not declared" was never
//!   established. A token that no readable manifest declares and no
//!   file matches is still `Missing`: whether it is a typo'd package
//!   name or a deleted file, nothing of that name exists here, so the
//!   verdict is not the confident wrong number -- only its label would
//!   be, and the reader is pointed at the same line either way.
//!
//!   Nothing outside the repository root is ever stat'd. Each anchored
//!   candidate has its `.` and `..` folded lexically first, and one that
//!   climbs out of the root is not tried. A `../`-prefixed token that
//!   does not resolve from the file's own directory is relative to a
//!   base the line does not state -- measured, `../../fixtures` in a
//!   nested CLAUDE.md is an import specifier written from a spec file
//!   two levels down -- and a suffix match can never hit a `..`
//!   segment, so before #1317 it was always Missing. Now, stripped of
//!   its leading `../` segments, a remainder that matches exactly one
//!   path under the file's own directory resolves; anything else is
//!   `Unknown` ("relative to an unstated base"), never `Missing`.
//!
//!   The walk reads the working tree on disk, not git, so an absent
//!   path is asked about before it is called missing: a file git
//!   ignores is expected to be absent from a checkout. Measured,
//!   `tools/stack/.env` is a secrets file each developer creates, and
//!   before #1318 it passed on a laptop that had one and was Missing on
//!   a fresh clone. One `git check-ignore --no-index --stdin` answers
//!   every path candidate in the run. An ignored path with a template
//!   beside it (`.example`, `.sample`, `.template`, `.dist`) resolves
//!   and is silent; one with no template is
//!   [`Verdict::IgnoredWithoutTemplate`] at [`Severity::Advice`]. A
//!   repository with no `.git` has no ignore rules, so nothing in it is
//!   ignored and the verdict stays `Missing`. With a `.git`, a git that
//!   cannot run or that fails makes the path `Unknown`: it never said
//!   "not ignored".
//! - **`path:line`**: the path as above, then the file's line count. A
//!   line past the end is [`Verdict::LinePastEof`] at
//!   [`Severity::Advice`]. A line WITHIN the file is silent, even when
//!   the sentence it cited has moved: `claude/sessions.rs:444` in the
//!   root file no longer holds the sentence it cites, but the file has
//!   2829 lines, and the check cannot know what the author meant to
//!   point at. A content fingerprint would catch that; it needs a stored
//!   hash and a rule for when the hash is stale, and is out of scope.
//! - **make target**: `packages::scripts::targets` over the file's
//!   directory, then the root. A makefile that `include`s another or
//!   carries a `%` pattern rule can define a target the parser cannot
//!   see, so a miss against such a file is `Unknown`, not `Missing`. A
//!   makefile that could not be read, to list its targets or to look for
//!   an `include`, is `Unknown` too, naming the file and the io error
//!   (#1411).
//! - **yarn/npm script**: `packages::scripts::scripts`, then
//!   `node_modules/.bin/<x>`, which resolves as a binary and is not a
//!   finding (`yarn vitest run` is this). A yarn CLI verb such as
//!   `install` is neither a script nor a binary and is never a finding.
//! - **cargo**: counted, never resolved. A cargo subcommand does not rot.
//! - **skill**: `Kind::Skill` names across every scope in the
//!   definitions inventory. With no inventory in the [`Context`] a skill
//!   reference is `Unknown` "no definitions inventory", never `Missing`:
//!   "we did not look" must not read as "it is not there" (#1050). A
//!   `/slash-command` is a skill invocation and is extracted as one;
//!   before #1300 `PATH_TOKEN` matched it, the resolver joined it onto
//!   an anchor -- where a leading `/` discards the anchor -- and probed
//!   the real filesystem root, so `/stacked-prs` was reported missing
//!   from a repository it was never looked for in.
//! - **symbol**: a whole-word grep of the last segment over `src/`,
//!   `src-tauri/src/`, `src-mobile/src/` AND the CLAUDE.md's own
//!   directory, files with a source or config extension only, `SKIP`
//!   directories skipped, one pass for every symbol in the run. The own
//!   directory is there for the same reason a path is anchored on it:
//!   measured, `.github/CLAUDE.md` names `GITHUB_REF_NAME`, which lives
//!   in `.github/workflows/release.yml` and nowhere under the three
//!   source roots, and the three-root grep called it missing. Zero
//!   files searched is `Unknown`, not "0 hits". Bare words are not
//!   extracted at all: measured, `main` hits 100 files and `false` 355.
//! - **`#NNNN`**: counted, never resolved. `claudemd` never talks to
//!   GitHub, and an issue number never resolves to nothing anyway: it
//!   closes, it does not vanish.
//! - **a placeholder path**: counted, never resolved. Measured on this
//!   checkout, `scripts/CLAUDE.md` states a naming convention as
//!   "`check-foo.py` has `check-foo.test.py`"; neither is a file and
//!   neither is rot. A path with a segment of `foo`, `bar`, `baz`, `qux`
//!   or `example` names a shape, not a file, and a `Missing` verdict on
//!   it would be the confident wrong number (qualify, or suppress).
//!
//! - **an absolute path**: one under the repository root is a
//!   repository path with the root spelled out, and resolves from the
//!   root alone. One anywhere else -- `/api/v1` is a URL route the
//!   dev-server proxy backs, `/etc/hosts` a host file -- is not a
//!   repository path: counted, never resolved, and never stat'd. Before
//!   #1316 the resolver joined it onto an anchor, where a leading `/`
//!   discards the anchor, and probed the host filesystem root; the
//!   suffix match then looked for `//api/v1`, which nothing can end
//!   with, so the verdict was a guaranteed Missing.
//!
//! A bare `lint-rust` in prose is not extracted, so it is not checked;
//! `refs.rs` says why. Commit SHAs and tags (`v5.20.0`) are not checked.
//!
//! # Unknown is a verdict, not a pass
//!
//! A path that could not be stat'd, a suffix search whose walk could not
//! list a directory or pruned the subtree the path lies under, a
//! manifest that exists and could not be read, an ambiguous suffix, a
//! `../` path relative to an unstated base, an absent path git could not
//! be asked about, a skill with no inventory: each is a
//! [`Severity::Unknown`] finding with the reason, and the file gets ONE
//! [`Severity::Note`] summary, "N references checked; K could not be
//! checked (…)", only when K > 0. So a run that checked 0 of 41 reads
//! differently from a clean one (absent is not zero, #846). It is a Note
//! because it counts the Unknown rows and recommends nothing: as Advice
//! it restated each of them a second time (#1372). The whole
//! check is `Err`, and so `CheckRun::Unknown`, only when the scan read no
//! CLAUDE.md at all AND could not list a directory: the file it did not
//! find may be behind that wall.
//!
//! # Two rules handed over by the content-shape research
//!
//! The smell catalogue (arXiv:2606.15828) names *blind references*,
//! "reference external documents … without explaining when that resource
//! becomes relevant", at 16 % prevalence. Here that is a prose line
//! naming an existing document path with "see", "read" or "consult" that
//! is neither an `@` import nor conditioned by "when", "if", "for",
//! "before", "on demand" or "as needed" ("when relevant" and "if needed"
//! by their first word, #1375). Both remedies are real and the brief
//! offers both, the condition first: a condition keeps the document
//! lazy, while an `@` import loads it in every session, and the brief
//! prices that -- "`@docs/a.md` would load ~N est. tokens in every
//! session" -- with [`tokens::estimate`] over the document as injected
//! plus what it imports in turn ("at least" when one of those could not
//! be read). Measured, a repository that ratchets its instruction-token
//! budget down in CI would have failed that check on the import the
//! brief offered. A document that cannot be read has no figure, never
//! 0. "topic → path" (or `->`) with text before the arrow is a condition
//! too: it is the index form of "read this when the topic comes up". So
//! is a short label before a colon that names a document list ("Docs:",
//! "Reference:", "Further reading:", "Docs, read on demand:"): at most
//! four words, no code span. "See `docs/gate.md`" inside the paragraph
//! about that gate stays a finding; the rule stays narrow, and the price
//! lets the reader decide. A
//! file under `.claude/rules/` is never blind: it loads itself, at launch
//! or when a file its `paths:` matches is read, and the `@` import the
//! brief would offer defeats that scoping. One line naming several
//! documents is ONE finding with each document as its own evidence,
//! not a row per path (#1320).
//! *Dated facts* are a line with an absolute date or a version number
//! AND "as of", "before", "after" or "until": the statement was true on
//! a date, and the brief quotes the line rather than judging it. Both are
//! [`Severity::Advice`]: the research asserts them, it did not measure an
//! effect.
//!
//! # Scope
//!
//! Repository files and a `CLAUDE.local.md` are checked; the global
//! `~/.claude/CLAUDE.md` is not, because its references are not about
//! this repository and every path in it would be reported missing from
//! every repository on the machine. Read-only, like everything under
//! `claudemd`.

use super::{Check, Context, Evidence, Finding, Locator, Producer, Severity, Subject};
use crate::claude::definitions::{Inventory, Kind};
use crate::claudemd::refs::{self, Ref, RefKind, Runner};
use crate::claudemd::{imports, text, tokens, Scope, SKIP};
use crate::packages::scripts::{self, Manifest, Target};
use regex::Regex;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub struct Rot;

/// What resolving one reference decided, when it decided against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Resolves to nothing. Certain.
    Missing,
    /// The file exists and has fewer lines than the reference cites.
    LinePastEof { lines: u64 },
    /// Absent, and git ignores it, so a checkout is expected not to have
    /// it; but no template sits beside it to create it from.
    IgnoredWithoutTemplate,
    /// Could not be decided, with why.
    Unknown(String),
}

/// One reference with a verdict against it, and what was measured to
/// reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rotten {
    pub r: Ref,
    pub verdict: Verdict,
    /// The measurement, verbatim: "Makefile: 14 targets, none named
    /// `nope`".
    pub measured: String,
}

/// A rule from the content-shape research, fired on one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// Document paths named with "see"/"read"/"consult" on one line, not
    /// imported and not conditioned. One per line, in line order.
    BlindReference { line: usize, paths: Vec<Blind> },
    /// A date or version beside "as of"/"before"/"after"/"until".
    DatedFact { line: usize, quoted: String },
}

/// One document a blind line names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blind {
    pub path: String,
    /// How an ambiguous suffix was settled, when it was (#1372).
    pub settled: Option<String>,
    /// What an `@` import of it would load, when that could be weighed.
    /// `None` is "could not be read", never zero (#1375).
    pub cost: Option<Cost>,
}

/// The estimated tokens an `@` import of one document would load: the
/// document and everything it imports in turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    pub tokens: u64,
    /// Something beneath it could not be read, so `tokens` is a floor.
    pub floor: bool,
}

/// One file's result.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileRot {
    pub findings: Vec<Rotten>,
    /// References resolved to an answer: fine, `Missing` or
    /// `LinePastEof`.
    pub refs_checked: usize,
    /// Why each `Unknown` could not be checked, deduplicated.
    pub unchecked: Vec<String>,
    /// `cargo` commands, issue numbers, placeholder paths and absolute
    /// paths outside the repository: counted, never resolved.
    pub unresolvable: usize,
    pub shape: Vec<Shape>,
}

/// Phrases shared by the finding sentences and [`suggestion`], so the
/// brief's remedy is chosen by the same constant the sentence was built
/// from and a rewording cannot split them.
const MISSING: &str = ", which does not exist in this repository";
const SKILL_MISSING: &str = ", and no skill of that name was found in any scope";
const PAST_EOF: &str = "; the file has ";
const IGNORED: &str = ", which git ignores, and no template (`.example`, `.sample`, `.template` or `.dist`) sits beside it";
const UNKNOWN: &str = ", which could not be checked: ";
const SUMMARY: &str = " references checked; ";
const BLIND: &str = " by name; not imported, no condition";
const COST: &str = " est. tokens in every session";
const DATED: &str = " states a dated fact: ";

impl Producer for Rot {
    fn check(&self) -> Check {
        Check::Rot
    }

    fn run(&self, cx: &Context) -> Result<Vec<Finding>, String> {
        let files = files_in_scope(cx);
        if files.is_empty() && !cx.scan.repo.unreadable_dirs.is_empty() {
            return Err(format!(
                "no CLAUDE.md was read and {} could not be listed: {}",
                count(
                    cx.scan.repo.unreadable_dirs.len(),
                    "directory",
                    "directories"
                ),
                cx.scan.repo.unreadable_dirs.join("; ")
            ));
        }

        let mut out = Vec::new();
        for (path, scope, result) in analyse(cx) {
            match result {
                Ok((text, rot)) => out.extend(findings_for(cx.repo, &path, scope, &text, &rot)),
                Err(why) => out.push(Finding::new(
                    Check::Rot,
                    Severity::Unknown,
                    Subject::ClaudeMd {
                        path: path.clone(),
                        scope,
                        section: None,
                    },
                    vec![Evidence {
                        at: Locator::File {
                            path: path.clone(),
                            line: None,
                        },
                        measured: why.clone(),
                    }],
                    format!("`{}` could not be read: {why}", display(cx.repo, &path)),
                )),
            }
        }
        Ok(out)
    }
}

/// The CLAUDE.md files this check reads: the repository's and a
/// `CLAUDE.local.md`, never the global one (see the module docs).
fn files_in_scope(cx: &Context) -> Vec<(String, Scope)> {
    let mut out: Vec<(String, Scope)> = cx
        .scan
        .repo
        .files
        .iter()
        .map(|f| (f.path.clone(), Scope::Repo))
        .collect();
    out.extend(
        cx.scan
            .extra
            .iter()
            .filter(|s| s.scope == Scope::Local)
            .map(|s| (s.file.path.clone(), s.scope)),
    );
    out
}

/// One file's path, scope, and either its text with its result or why
/// it could not be read.
type Analysed = (String, Scope, Result<(String, FileRot), String>);

/// Every file in scope, checked. `Err` per file is a file the scan read
/// a moment ago and this pass could not.
///
/// Public to the test module so a measurement over a real checkout can
/// print counts per file; the producer reads it through [`Producer::run`].
pub fn analyse(cx: &Context) -> Vec<Analysed> {
    let files = files_in_scope(cx);
    let mut texts: Vec<(String, Scope, Result<String, String>)> = Vec::new();
    for (path, scope) in files {
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string());
        texts.push((path, scope, text));
    }

    // Every symbol in the run, so the source tree is read once rather
    // than once per symbol.
    let mut symbols: BTreeSet<String> = BTreeSet::new();
    for (_, _, t) in &texts {
        if let Ok(t) = t {
            for r in refs::extract(t) {
                if let RefKind::Symbol { last } = r.kind {
                    symbols.insert(last);
                }
            }
        }
    }
    let dirs: Vec<PathBuf> = texts
        .iter()
        .filter_map(|(p, _, _)| Path::new(p).parent().map(Path::to_path_buf))
        .collect();
    let mut resolver = Resolver::new(cx.repo, cx.definitions, &symbols, &dirs);
    for (p, _, t) in &texts {
        if let Ok(t) = t {
            resolver.plan(Path::new(p).parent().unwrap_or(cx.repo), t);
        }
    }

    texts
        .into_iter()
        .map(|(path, scope, text)| {
            let result = text.map(|t| {
                let rot = check_file(cx.repo, Path::new(&path), &t, &mut resolver);
                (t, rot)
            });
            (path, scope, result)
        })
        .collect()
}

/// Check one CLAUDE.md's text. `file` is its absolute path; its directory
/// anchors relative references.
pub fn check_file(repo: &Path, file: &Path, text: &str, res: &mut Resolver) -> FileRot {
    let dir = file.parent().unwrap_or(repo).to_path_buf();
    let mut out = FileRot::default();
    let mut unchecked: BTreeSet<String> = BTreeSet::new();
    // Path references that resolved, for the blind-reference rule.
    let mut resolved_paths: Vec<Named> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    for r in refs::extract(text) {
        // Where a path reference resolved to, when it is a file, and the
        // word that settled it when its suffix was ambiguous.
        let mut landed: Option<PathBuf> = None;
        let mut settled: Option<String> = None;
        let words = item_words(&lines, r.line);
        let outcome = match &r.kind {
            RefKind::Cargo | RefKind::Issue { .. } => {
                out.unresolvable += 1;
                continue;
            }
            RefKind::Path { path } | RefKind::PathLine { path, .. }
                if is_placeholder(path) || is_outside_absolute(repo, path) =>
            {
                out.unresolvable += 1;
                continue;
            }
            RefKind::Path { path } => res.path(&dir, path, &words).map(|to| match to {
                Resolved::File(p) => landed = Some(p),
                Resolved::Settled { path, by } => {
                    landed = Some(path);
                    settled = Some(by);
                }
                Resolved::Package | Resolved::Ignored => {}
            }),
            RefKind::PathLine { path, line } => res.path_line(&dir, path, *line, &words),
            RefKind::MakeTarget { name } => res.make_target(&dir, name),
            RefKind::Script { runner, name } => res.script(&dir, *runner, name),
            RefKind::Skill { name } => res.skill(name),
            RefKind::Symbol { last } => res.symbol(last),
        };
        match outcome {
            Ok(()) => {
                out.refs_checked += 1;
                // A `.claude/rules` file loads itself, so naming it is
                // never blind (#1320).
                let rule = landed.as_deref().is_some_and(|p| is_rule_file(repo, p));
                if let RefKind::Path { path } = &r.kind {
                    if !rule {
                        resolved_paths.push(Named {
                            line: r.line,
                            path: path.clone(),
                            file: landed,
                            settled,
                        });
                    }
                }
            }
            Err((verdict @ Verdict::Unknown(_), measured)) => {
                if let Verdict::Unknown(why) = &verdict {
                    unchecked.insert(why.clone());
                }
                out.findings.push(Rotten {
                    r,
                    verdict,
                    measured,
                });
            }
            Err((verdict, measured)) => {
                out.refs_checked += 1;
                out.findings.push(Rotten {
                    r,
                    verdict,
                    measured,
                });
            }
        }
    }
    out.unchecked = unchecked.into_iter().collect();
    out.shape = shape_rules(text, &resolved_paths);
    out
}

/// Segments that name a shape rather than a file. See the module docs.
const PLACEHOLDERS: &[&str] = &["foo", "bar", "baz", "qux", "example"];

/// Whether a path is a naming-convention example rather than a file.
fn is_placeholder(path: &str) -> bool {
    path.split(['/', '.', '-', '_'])
        .any(|seg| PLACEHOLDERS.contains(&seg.to_ascii_lowercase().as_str()))
}

/// Whether a token is an absolute path that does not lie under the
/// repository root: `/api/v1` is a URL route, `/etc/hosts` a host file.
/// Neither is a repository path, and resolving one would stat the host
/// filesystem, where the verdict depends on the machine (#1316).
fn is_outside_absolute(repo: &Path, path: &str) -> bool {
    path.starts_with('/') && !Path::new(path).starts_with(repo)
}

/// A resolution that decided against the reference: the verdict and
/// what was measured.
type Refused = (Verdict, String);

fn unknown(why: String) -> Refused {
    (Verdict::Unknown(why.clone()), why)
}

/// A path reference that resolved, for the blind-reference rule.
struct Named {
    line: usize,
    path: String,
    /// Where it landed, when that is a file on disk.
    file: Option<PathBuf>,
    /// How an ambiguous suffix was settled, when it was (#1372).
    settled: Option<String>,
}

/// Whether a line opens a list item: `- `, `* `, `+ `, `1. ` or `1) `.
fn is_list_item(line: &str) -> bool {
    let t = line.trim_start();
    if ["- ", "* ", "+ "].iter().any(|m| t.starts_with(m)) {
        return true;
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && (t[digits..].starts_with(". ") || t[digits..].starts_with(") "))
}

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9_][A-Za-z0-9_-]*").unwrap());

/// The lowercased words of 1-based `line`, widened to its whole list
/// item when it is in one: the item's opening line and its indented
/// continuation lines, up to a blank line or the next item. A line in no
/// list item is itself alone.
///
/// Prose words only: inline code spans are blanked first. Another span
/// on the line is another reference, and its segments say where IT
/// points: "`crates/a/only.rs` and `lib.rs`" does not make `lib.rs` the
/// one under `a`.
fn item_words(lines: &[&str], line: usize) -> BTreeSet<String> {
    let Some(at) = line.checked_sub(1).filter(|i| *i < lines.len()) else {
        return BTreeSet::new();
    };
    let continuation =
        |l: &str| l.starts_with([' ', '\t']) && !l.trim().is_empty() && !is_list_item(l);
    let mut start = at;
    if !is_list_item(lines[at]) {
        while start > 0 && continuation(lines[start]) {
            start -= 1;
        }
        if !is_list_item(lines[start]) {
            start = at;
        }
    }
    let mut end = at;
    if is_list_item(lines[start]) {
        while end + 1 < lines.len() && continuation(lines[end + 1]) {
            end += 1;
        }
    }
    lines[start..=end]
        .iter()
        .map(|l| text::blank_spans(l))
        .flat_map(|l| {
            WORD.find_iter(&l)
                .map(|m| m.as_str().to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The one candidate of an ambiguous suffix match that the words of its
/// line settle, and the words that settled it (#1372).
///
/// Only a candidate's segments BEFORE the suffix are compared: the
/// suffix is what every candidate shares. A word equal to such a segment
/// of every candidate carries nothing and is ignored. The candidates the
/// remaining words hit must be exactly one; none or several is `None`,
/// and the reference stays Unknown, never a guess.
fn settle(clean: &str, matches: &[String], words: &BTreeSet<String>) -> Option<(String, String)> {
    let prefixes: Vec<Vec<String>> = matches
        .iter()
        .map(|m| {
            m.strip_suffix(clean)
                .unwrap_or("")
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_ascii_lowercase)
                .collect()
        })
        .collect();
    let mut hit: BTreeSet<usize> = BTreeSet::new();
    let mut by: Vec<&str> = Vec::new();
    for w in words {
        let hits: Vec<usize> = (0..matches.len())
            .filter(|i| prefixes[*i].iter().any(|s| s == w))
            .collect();
        if hits.is_empty() || hits.len() == matches.len() {
            continue;
        }
        hit.extend(hits);
        by.push(w);
    }
    if hit.len() != 1 {
        return None;
    }
    let one = *hit.iter().next().expect("one, checked above");
    let by = by
        .iter()
        .map(|w| format!("`{w}`"))
        .collect::<Vec<_>>()
        .join(" and ");
    Some((matches[one].clone(), by))
}

/// What a `metadata` call said about a path.
enum Probe {
    Found,
    Absent,
    /// The path could not be stat'd for a reason other than absence,
    /// which is a wall, not an answer.
    Refused(String),
}

/// What the `package.json` manifests said about a bare token.
enum DependencyLookup {
    /// A declared dependency. Not a path, and not a finding.
    Declared,
    /// A manifest was read and does not declare it.
    NotDeclared,
    /// No `package.json` beside the file or at the root.
    NoManifest,
    /// A `package.json` exists and could not be read or parsed, so
    /// "not declared" was never established.
    Unknown(String),
}

/// What a path reference resolved TO.
enum Resolved {
    /// A real file or directory.
    File(PathBuf),
    /// A real file or directory, chosen from an ambiguous suffix match by
    /// a word on the reference's own line or list item (#1372). `by` is
    /// the measurement, naming the word, for any finding the reference
    /// goes on to produce.
    Settled { path: PathBuf, by: String },
    /// Not a file: a package this project declares. It has no lines to
    /// count and no place on disk to cite.
    Package,
    /// A file git ignores, absent here as a checkout expects, with a
    /// template beside it. It has no lines to count either.
    Ignored,
}

/// Suffixes of a committed template for an ignored local file:
/// `.env.example` beside `.env`.
const TEMPLATE_SUFFIXES: &[&str] = &[".example", ".sample", ".template", ".dist"];

/// How long one `git check-ignore` may take before its answer is
/// Unknown. The same bound the worktree scan gives one git call.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Which of `paths`, relative to `repo`, git ignores: ONE process for
/// the lot, fed on stdin. `--no-index` so a tracked file is judged by
/// the patterns too; only absent paths are ever asked about.
///
/// Every failure is `Err`, never an empty set: a git that did not answer
/// did not say "not ignored" (#1050).
///
/// `pub(super)` so the transcripts producer asks git the same way when
/// it tests whether a deleted session directory was ignored (#1335).
pub(super) fn check_ignored(
    git: &Path,
    repo: &Path,
    paths: &[String],
) -> Result<BTreeSet<String>, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(git)
        .arg("-C")
        .arg(repo)
        .args(["check-ignore", "--no-index", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git check-ignore could not run: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "git check-ignore could not run: no stdin".to_string())?;
    let input = paths.join("\n") + "\n";
    // Written from its own thread, so a git that fills its stdout pipe
    // before reading all of stdin cannot deadlock against this one.
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // The receiver is gone on timeout; that is expected.
        let _ = tx.send(child.wait_with_output());
    });
    let output = match rx.recv_timeout(GIT_TIMEOUT) {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("git check-ignore could not run: {e}")),
        Err(_) => {
            return Err(format!(
                "git check-ignore did not respond within {}s",
                GIT_TIMEOUT.as_secs()
            ))
        }
    };
    let _ = writer.join();
    match output.status.code() {
        // 0: some are ignored, and stdout names them. 1: none are.
        Some(0) | Some(1) => Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| l.trim_end_matches('\r').to_string())
            .filter(|l| !l.is_empty())
            .collect()),
        code => Err(format!(
            "git check-ignore exit status {}: {}",
            code.map(|c| c.to_string()).unwrap_or_else(|| "none".into()),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// Whether a token could be an npm package name rather than a path.
///
/// One segment only: a token with a `/` is a path, except the one form
/// `@scope/name` that npm itself uses. `.` and `..` never reach here.
fn package_shaped(token: &str) -> bool {
    let bare = match token.strip_prefix('@') {
        Some(rest) => match rest.split_once('/') {
            Some((scope, name)) if !scope.is_empty() && !name.is_empty() => {
                return !name.contains('/')
            }
            _ => return false,
        },
        None => token,
    };
    !bare.contains('/')
}

fn probe(p: &Path) -> Probe {
    use std::io::ErrorKind;
    match std::fs::metadata(p) {
        Ok(_) => Probe::Found,
        // A file where a directory was expected is absence too:
        // `src/lib.rs/foo` names nothing.
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            Probe::Absent
        }
        Err(e) => Probe::Refused(e.to_string()),
    }
}

/// The repository tree as relative `/`-joined paths, files and
/// directories both, walked with [`SKIP`]. Built once per run, on the
/// first reference that needs a suffix match.
struct Tree {
    paths: Vec<String>,
    unreadable: Vec<String>,
    /// The [`SKIP`] directories the walk pruned, as relative paths.
    ///
    /// A pruned subtree is "we did not look", and #1299 is what happens
    /// when that reads as "it is not there": one prune turned every
    /// `.claude/**` reference in a repository into a confident Missing.
    /// `Tree::pruning` reads this so a reference that LIES UNDER a prune
    /// is `Unknown` with the prune named. Only such a reference: every
    /// healthy repository prunes `node_modules` and `target`, and
    /// letting any prune soften every miss would retire the check.
    pruned: Vec<String>,
}

impl Tree {
    /// The prune `clean` lies under, if any: the reference could only
    /// have resolved inside a subtree the walk did not enter, so the
    /// tree cannot say it is absent.
    fn pruning(&self, clean: &str) -> Option<&str> {
        self.pruned
            .iter()
            .find(|d| clean == d.as_str() || clean.starts_with(&format!("{d}/")))
            .map(|d| d.as_str())
    }
}

fn index_tree(repo: &Path) -> Tree {
    let mut tree = Tree {
        paths: Vec::new(),
        unreadable: Vec::new(),
        pruned: Vec::new(),
    };
    let mut skipped: Vec<PathBuf> = Vec::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                tree.unreadable
                    .push(format!("{} ({e})", display(repo, &dir.to_string_lossy())));
                continue;
            }
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                // SKIP alone. It already carries `worktrees`, so the
                // agent-managed checkouts under `.claude/worktrees` are
                // pruned by their OWN name when the walk reaches them.
                //
                // This used to ALSO prune `.claude` for holding them,
                // and with it every `.claude/rules/*.md` and
                // `.claude/skills/**/SKILL.md` a CLAUDE.md names, which
                // then matched 0 paths and read as Missing (#1299).
                // `.claude` is worth walking. `scan_repo` carried the
                // same parent-check and lost the same subtree.
                if SKIP.contains(&name.as_str()) {
                    skipped.push(e.path());
                    continue;
                }
                stack.push(e.path());
            }
            if let Some(rel) = relative(repo, &e.path()) {
                tree.paths.push(rel);
            }
        }
    }
    tree.pruned = skipped.iter().filter_map(|p| relative(repo, p)).collect();
    tree.paths.sort();
    tree.unreadable.sort();
    tree.pruned.sort();
    tree
}

/// `p` relative to `repo`, `/`-joined whatever the platform separator.
fn relative(repo: &Path, p: &Path) -> Option<String> {
    let rel = p.strip_prefix(repo).ok()?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// `anchor` joined with the `/`-separated `rel`, with `.` and `..`
/// folded lexically, or `None` when the result is not under `repo`.
///
/// Lexical on purpose: it decides what may be stat'd, so it must not
/// stat anything to decide it. `..` is folded here rather than by the
/// filesystem, so no candidate outside the repository is ever probed
/// (#1317).
fn contained(repo: &Path, anchor: &Path, rel: &str) -> Option<PathBuf> {
    let mut out = anchor.to_path_buf();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if !out.pop() {
                    return None;
                }
            }
            s => out.push(s),
        }
    }
    out.starts_with(repo).then_some(out)
}

/// Whether the relative tree path `p` lies under the relative directory
/// `own`; everything lies under the root, which is `""`.
fn under(own: &str, p: &str) -> bool {
    own.is_empty()
        || p.strip_prefix(own)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// A path for a finding sentence: relative to the repository when it is
/// under it, absolute otherwise.
fn display(repo: &Path, path: &str) -> String {
    relative(repo, Path::new(path))
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| path.to_string())
}

/// Every symbol's whole-word hits over the search roots, plus how many
/// files were searched and what could not be listed.
///
/// Shared with the placement check (#1398), which asks WHERE a name is
/// used rather than whether it is, so the hits are the files, not a
/// count.
pub(crate) struct SymbolSearch {
    /// Symbol -> every searched file with a whole-word hit, absolute.
    pub(crate) files: HashMap<String, Vec<PathBuf>>,
    pub(crate) files_searched: usize,
    pub(crate) unreadable: Vec<String>,
    /// The roots walked, as named in a reason.
    pub(crate) roots: Vec<String>,
}

const SOURCE_ROOTS: &[&str] = &["src", "src-tauri/src", "src-mobile/src"];

const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "swift", "kt", "java", "go", "py", "rb", "yml",
    "yaml", "toml", "sh",
];

/// A file larger than this is not read. Measured on this repository, the
/// largest source file is under 1 MiB; a file past this bound is
/// generated or vendored, and skipping it is recorded so a miss stays
/// Unknown.
const SOURCE_FILE_BOUND: u64 = 4 * 1024 * 1024;

/// `dirs` are the directories of the CLAUDE.md files in the run. A root
/// inside another root is dropped, so one tree is walked once.
pub(crate) fn search_symbols(
    repo: &Path,
    names: &BTreeSet<String>,
    dirs: &[PathBuf],
) -> SymbolSearch {
    let mut out = SymbolSearch {
        files: names.iter().map(|n| (n.clone(), Vec::new())).collect(),
        files_searched: 0,
        unreadable: Vec::new(),
        roots: Vec::new(),
    };
    if names.is_empty() {
        return out;
    }
    let mut candidates: Vec<PathBuf> = SOURCE_ROOTS.iter().map(|r| repo.join(r)).collect();
    candidates.extend(dirs.iter().cloned());
    candidates.retain(|p| p.is_dir());
    candidates.sort();
    candidates.dedup();
    let mut roots: Vec<PathBuf> = Vec::new();
    for c in candidates {
        if !roots.iter().any(|r| c.starts_with(r)) {
            roots.push(c);
        }
    }
    out.roots = roots
        .iter()
        .map(|r| {
            relative(repo, r)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "the repository root".to_string())
        })
        .collect();
    let mut stack: Vec<PathBuf> = roots;
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                out.unreadable
                    .push(format!("{} ({e})", display(repo, &dir.to_string_lossy())));
                continue;
            }
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(meta) = e.metadata() else {
                out.unreadable.push(format!(
                    "{} (could not stat)",
                    display(repo, &e.path().to_string_lossy())
                ));
                continue;
            };
            if meta.is_dir() {
                if !SKIP.contains(&name.as_str()) {
                    stack.push(e.path());
                }
                continue;
            }
            let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
            if !SOURCE_EXTENSIONS.contains(&ext) {
                continue;
            }
            if meta.len() > SOURCE_FILE_BOUND {
                out.unreadable.push(format!(
                    "{} ({} bytes, over the {} byte read bound)",
                    display(repo, &e.path().to_string_lossy()),
                    meta.len(),
                    SOURCE_FILE_BOUND
                ));
                continue;
            }
            let text = match std::fs::read_to_string(e.path()) {
                Ok(t) => t,
                Err(err) => {
                    out.unreadable.push(format!(
                        "{} ({err})",
                        display(repo, &e.path().to_string_lossy())
                    ));
                    continue;
                }
            };
            out.files_searched += 1;
            for (sym, files) in out.files.iter_mut() {
                if has_whole_word(&text, sym) {
                    files.push(e.path());
                }
            }
        }
    }
    out.unreadable.sort();
    out
}

/// Whether `word` occurs in `text` with no identifier character on
/// either side.
fn has_whole_word(text: &str, word: &str) -> bool {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    })
}

/// Yarn's own verbs: `yarn install` names no script and no binary, and
/// is never rot. Not `test`, `start` or `build`, which run scripts.
const YARN_VERBS: &[&str] = &[
    "install",
    "add",
    "remove",
    "up",
    "upgrade",
    "upgrade-interactive",
    "dlx",
    "exec",
    "why",
    "workspace",
    "workspaces",
    "init",
    "set",
    "config",
    "cache",
    "info",
    "pack",
    "publish",
    "version",
    "npm",
    "node",
    "bin",
    "constraints",
    "dedupe",
    "explain",
    "link",
    "unlink",
    "patch",
    "patch-commit",
    "rebuild",
    "search",
    "stage",
    "unplug",
    "create",
    "global",
    "outdated",
    "list",
    "check",
    "audit",
    "import",
    "run",
    "plugin",
    "sdks",
];

/// Everything one run resolves against, built lazily and cached so ten
/// references into one tree cost one walk.
pub struct Resolver<'a> {
    repo: &'a Path,
    definitions: Option<&'a Inventory>,
    tree: Option<Tree>,
    makefiles: HashMap<PathBuf, Manifest<Vec<Target>>>,
    packages: HashMap<PathBuf, Manifest<Vec<String>>>,
    dependencies: HashMap<PathBuf, Manifest<Vec<String>>>,
    symbols: SymbolSearch,
    /// The git binary `check-ignore` runs as.
    git: PathBuf,
    /// Every repository-relative candidate the run may ask git about,
    /// planned up front so the first question asks them all at once.
    ignore_plan: BTreeSet<String>,
    /// What git said, per candidate.
    ignored: HashMap<String, bool>,
    /// Why git could not be asked, once it could not.
    ignore_failed: Option<String>,
}

impl<'a> Resolver<'a> {
    /// `dirs` are the directories of the CLAUDE.md files in the run,
    /// which the symbol search covers beside the source roots.
    pub fn new(
        repo: &'a Path,
        definitions: Option<&'a Inventory>,
        symbols: &BTreeSet<String>,
        dirs: &[PathBuf],
    ) -> Self {
        Resolver {
            repo,
            definitions,
            tree: None,
            makefiles: HashMap::new(),
            packages: HashMap::new(),
            dependencies: HashMap::new(),
            symbols: search_symbols(repo, symbols, dirs),
            git: crate::auth::git_program().to_path_buf(),
            ignore_plan: BTreeSet::new(),
            ignored: HashMap::new(),
            ignore_failed: None,
        }
    }

    /// The same resolver with another git binary, so a test can prove
    /// what one that cannot run produces.
    pub fn with_git(mut self, git: &Path) -> Self {
        self.git = git.to_path_buf();
        self
    }

    /// Record every path candidate `text` could ask git about, so a run
    /// over many files spawns one `git check-ignore`, not one per path.
    pub fn plan(&mut self, dir: &Path, text: &str) {
        for r in refs::extract(text) {
            if let RefKind::Path { path } | RefKind::PathLine { path, .. } = &r.kind {
                if !is_placeholder(path) && !is_outside_absolute(self.repo, path) {
                    let c = self.candidates(dir, path);
                    self.ignore_plan.extend(c);
                }
            }
        }
    }

    /// The repository-relative places an anchored path reference could
    /// be: under the file's directory, then under the root; never one
    /// outside the repository.
    fn candidates(&self, dir: &Path, path: &str) -> Vec<String> {
        let (anchors, rel) = if path.starts_with('/') {
            match relative(self.repo, Path::new(path)) {
                Some(rel) => (vec![self.repo.to_path_buf()], rel),
                None => return Vec::new(),
            }
        } else {
            (self.anchors(dir), path.to_string())
        };
        let clean = rel.trim_start_matches("./").trim_end_matches('/');
        let mut out: Vec<String> = Vec::new();
        for a in anchors {
            if let Some(rel) = contained(self.repo, &a, clean).and_then(|c| relative(self.repo, &c))
            {
                if !rel.is_empty() && !out.contains(&rel) {
                    out.push(rel);
                }
            }
        }
        out
    }

    /// The first of `rels` git ignores, if any.
    ///
    /// A repository with no `.git` has no ignore rules, so nothing in it
    /// is ignored: that is established by the stat, not assumed. With a
    /// `.git`, git is asked -- once, for every planned candidate not yet
    /// answered -- and a git that could not answer is `Err`, never "not
    /// ignored".
    fn ignored_among(&mut self, rels: &[String]) -> Result<Option<String>, String> {
        if let Some(why) = &self.ignore_failed {
            return Err(why.clone());
        }
        let need: Vec<&String> = rels
            .iter()
            .filter(|r| !self.ignored.contains_key(*r))
            .collect();
        if !need.is_empty() {
            let mut query: BTreeSet<String> = need.into_iter().cloned().collect();
            query.extend(
                self.ignore_plan
                    .iter()
                    .filter(|r| !self.ignored.contains_key(*r))
                    .cloned(),
            );
            let query: Vec<String> = query.into_iter().collect();
            let answer = match probe(&self.repo.join(".git")) {
                Probe::Absent => Ok(BTreeSet::new()),
                Probe::Refused(e) => Err(format!("`.git` could not be checked: {e}")),
                Probe::Found => check_ignored(&self.git, self.repo, &query),
            };
            match answer {
                Ok(set) => {
                    for q in query {
                        let hit = set.contains(&q);
                        self.ignored.insert(q, hit);
                    }
                }
                Err(why) => {
                    self.ignore_failed = Some(why.clone());
                    return Err(why);
                }
            }
        }
        Ok(rels
            .iter()
            .find(|r| self.ignored.get(*r) == Some(&true))
            .cloned())
    }

    /// An absent path that no manifest declares: `Missing`, unless git
    /// ignores it (#1318). An ignored path is expected to be absent from
    /// a checkout -- `tools/stack/.env` is a secrets file each developer
    /// creates -- so with a template beside it it resolves, and without
    /// one it is Advice. The tree walk reads the disk, not git, so
    /// without this the verdict for such a file depended on the machine.
    fn absent(&mut self, dir: &Path, path: &str, measured: String) -> Result<Resolved, Refused> {
        let candidates = self.candidates(dir, path);
        let rel = match self.ignored_among(&candidates) {
            Ok(None) => return Err((Verdict::Missing, measured)),
            Ok(Some(rel)) => rel,
            Err(why) => {
                return Err(unknown(format!(
                    "`{path}` matches nothing in the working tree, and whether git ignores it could not be established: {why}"
                )))
            }
        };
        for suffix in TEMPLATE_SUFFIXES {
            let Some(t) = contained(self.repo, self.repo, &format!("{rel}{suffix}")) else {
                continue;
            };
            match probe(&t) {
                Probe::Found => return Ok(Resolved::Ignored),
                Probe::Absent => {}
                Probe::Refused(e) => {
                    return Err(unknown(format!(
                        "`{rel}` is ignored by git, and its template `{rel}{suffix}` could not be checked: {e}"
                    )))
                }
            }
        }
        Err((
            Verdict::IgnoredWithoutTemplate,
            format!(
                "`git check-ignore` ignores `{rel}`; no {} beside it",
                TEMPLATE_SUFFIXES
                    .iter()
                    .map(|s| format!("`{rel}{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ))
    }

    fn tree(&mut self) -> &Tree {
        if self.tree.is_none() {
            self.tree = Some(index_tree(self.repo));
        }
        self.tree.as_ref().expect("built just above")
    }

    /// The directories a relative reference is anchored on: the file's
    /// own, then the repository root when that is a different place.
    fn anchors(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out = vec![dir.to_path_buf()];
        if dir != self.repo {
            out.push(self.repo.to_path_buf());
        }
        out
    }

    /// Whether `token` is a package this project declares, and what the
    /// manifests said.
    ///
    /// A CLAUDE.md line reading "Charts: `chart.js` only via the
    /// `libs/ui/…` components" names a DEPENDENCY. `chart.js` ends in a
    /// known extension, so the extractor calls it a path and the
    /// resolver called it missing (#1300). Shape cannot separate the two
    /// -- plenty of real files are named `something.js` -- so the
    /// manifest is the signal: a token that is a declared dependency is
    /// a package reference, and reporting it as a missing file is wrong.
    ///
    /// A scoped `@scope/name` is looked up whole; only the bare token is
    /// ever tried, so `src/chart.js` is a path and stays one.
    fn declared_package(&mut self, dir: &Path, token: &str) -> DependencyLookup {
        let mut unreadable: Vec<String> = Vec::new();
        let mut any_manifest = false;
        for anchor in self.anchors(dir) {
            let manifest = self
                .dependencies
                .entry(anchor.clone())
                .or_insert_with(|| scripts::dependencies(&anchor))
                .clone();
            match manifest {
                Manifest::Absent => {}
                Manifest::Unreadable(why) => unreadable.push(format!(
                    "{why} in `{}`",
                    display(self.repo, &anchor.to_string_lossy())
                )),
                Manifest::Present(names) => {
                    any_manifest = true;
                    if names.iter().any(|n| n == token) {
                        return DependencyLookup::Declared;
                    }
                }
            }
        }
        if !unreadable.is_empty() {
            return DependencyLookup::Unknown(unreadable.join("; "));
        }
        if any_manifest {
            DependencyLookup::NotDeclared
        } else {
            DependencyLookup::NoManifest
        }
    }

    /// A path reference, resolved to where it lives. `words` are the
    /// words of the reference's own line or list item, which may settle
    /// an ambiguous suffix match (#1372).
    fn path(
        &mut self,
        dir: &Path,
        path: &str,
        words: &BTreeSet<String>,
    ) -> Result<Resolved, Refused> {
        // An absolute token names one place. Under the repository it is
        // a repository path with the root spelled out, so it resolves
        // from the root alone and by nothing looser; anywhere else it is
        // not a repository path and is never stat'd (#1316). `check_file`
        // counts the second kind before it gets here; this arm keeps the
        // resolver honest for any other caller.
        let absolute = path.starts_with('/');
        let stripped;
        let path = if absolute {
            match relative(self.repo, Path::new(path)) {
                Some(rel) => {
                    stripped = rel;
                    stripped.as_str()
                }
                None => {
                    return Err(unknown(format!(
                        "`{path}` is an absolute path outside the repository, so it was not checked"
                    )))
                }
            }
        } else {
            path
        };
        let clean = path.trim_start_matches("./").trim_end_matches('/');
        if clean.is_empty() {
            return Ok(Resolved::File(self.repo.to_path_buf()));
        }
        let anchors = if absolute {
            vec![self.repo.to_path_buf()]
        } else {
            self.anchors(dir)
        };
        for anchor in anchors {
            // A candidate that climbs out of the repository is never
            // tried: `repo/../../fixtures` is somewhere on the host, and
            // a verdict read there depends on the machine (#1317).
            let Some(candidate) = contained(self.repo, &anchor, clean) else {
                continue;
            };
            match probe(&candidate) {
                Probe::Found => return Ok(Resolved::File(candidate)),
                Probe::Absent => {}
                Probe::Refused(e) => {
                    return Err(unknown(format!(
                        "`{}` could not be checked: {e}",
                        display(self.repo, &candidate.to_string_lossy())
                    )))
                }
            }
        }
        if clean.split('/').any(|s| s == "..") {
            return self.parent_relative(dir, path, clean);
        }
        let repo = self.repo.to_path_buf();
        // Owned, so the tree borrow ends here: the 0-match arm consults
        // the dependency manifests, which needs `&mut self`.
        let (matches, walked, unreadable, pruned) = {
            let tree = self.tree();
            let matches: Vec<String> = tree
                .paths
                .iter()
                .filter(|p| p.as_str() == clean || (!absolute && p.ends_with(&format!("/{clean}"))))
                .cloned()
                .collect();
            (
                matches,
                tree.paths.len(),
                tree.unreadable.clone(),
                tree.pruning(clean).map(str::to_string),
            )
        };
        // A nested CLAUDE.md describes its own directory (#1319): when
        // any match lies under it, only those are candidates. One
        // resolves; two are still ambiguous. None falls back to the
        // whole tree. For the root file the own directory is the
        // repository, so this changes nothing there.
        let own = relative(&repo, dir).unwrap_or_default();
        let mine: Vec<String> = matches.iter().filter(|m| under(&own, m)).cloned().collect();
        let matches = if mine.is_empty() { matches } else { mine };
        if matches.len() > 1 {
            if let Some((one, by)) = settle(clean, &matches, words) {
                return Ok(Resolved::Settled {
                    path: repo.join(&one),
                    by: format!(
                        "`{clean}` matches {} paths; {by} on the same line or list item is a segment of only `{one}`",
                        matches.len()
                    ),
                });
            }
        }
        match matches.len() {
            1 => Ok(Resolved::File(repo.join(&matches[0]))),
            // Ordered before the Missing arm on purpose: a reference
            // under a pruned subtree was never looked for, and #1299 is
            // the cost of calling that absent.
            0 if pruned.is_some() => {
                let pruned = pruned.expect("matched just above");
                Err(unknown(format!(
                    "`{clean}` lies under `{pruned}`, which the tree walk does not enter, so whether it exists was not checked"
                )))
            }
            // Nothing on disk, and the walk did enter everywhere it
            // would have looked. Before calling it missing, ask the
            // manifests: a bare `chart.js` is a declared dependency far
            // more often than it is a deleted file, and shape cannot
            // tell them apart (#1300).
            0 if unreadable.is_empty() => {
                if package_shaped(clean) {
                    match self.declared_package(dir, clean) {
                        DependencyLookup::Declared => return Ok(Resolved::Package),
                        DependencyLookup::Unknown(why) => {
                            return Err(unknown(format!(
                                "`{clean}` is not a file, and whether it is a declared dependency could not be established: {why}"
                            )))
                        }
                        DependencyLookup::NotDeclared | DependencyLookup::NoManifest => {}
                    }
                }
                let measured = format!(
                    "resolved against `{}`, the repository root and a suffix match over {walked} paths in the working tree: 0 matches",
                    display(&repo, &dir.to_string_lossy()),
                );
                self.absent(dir, path, measured)
            }
            0 => Err(unknown(format!(
                "`{clean}` matches nothing in the readable tree, and {} could not be listed: {}",
                count(unreadable.len(), "directory", "directories"),
                unreadable.join("; ")
            ))),
            n => Err(unknown(format!(
                "`{clean}` is ambiguous: it matches {n} paths ({})",
                matches
                    .iter()
                    .take(4)
                    .map(|m| format!("`{m}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// A `../` path that did not resolve from the file's own directory.
    ///
    /// It is relative to something the line does not state: measured,
    /// `../../fixtures` in a nested CLAUDE.md is an import specifier,
    /// written from a spec file two levels below it. A suffix match can
    /// never hit a `..` segment, so before #1317 it was always Missing.
    /// The check did not establish absence, so it is Unknown -- unless,
    /// stripped of its leading `../` segments, what remains matches
    /// exactly one path under the file's own directory. That one hit is
    /// unambiguous and resolves; two or none are Unknown.
    fn parent_relative(
        &mut self,
        dir: &Path,
        path: &str,
        clean: &str,
    ) -> Result<Resolved, Refused> {
        let own = relative(self.repo, dir).unwrap_or_default();
        let shown = if own.is_empty() {
            "the repository root".to_string()
        } else {
            format!("`{own}`")
        };
        let mut rest = clean;
        while let Some(r) = rest.strip_prefix("../") {
            rest = r;
        }
        let rest = rest.trim_start_matches("./");
        if rest.is_empty() || rest == ".." || rest.split('/').any(|s| s == "..") {
            return Err(unknown(format!(
                "`{path}` is relative to an unstated base: it does not resolve from {shown}"
            )));
        }
        let repo = self.repo.to_path_buf();
        let matches: Vec<String> = self
            .tree()
            .paths
            .iter()
            .filter(|p| under(&own, p) && (p.as_str() == rest || p.ends_with(&format!("/{rest}"))))
            .cloned()
            .collect();
        match matches.len() {
            1 => Ok(Resolved::File(repo.join(&matches[0]))),
            n => Err(unknown(format!(
                "`{path}` is relative to an unstated base: it does not resolve from {shown}, and `{rest}` matches {} under it",
                count(n, "path", "paths")
            ))),
        }
    }

    fn path_line(
        &mut self,
        dir: &Path,
        path: &str,
        line: u32,
        words: &BTreeSet<String>,
    ) -> Result<(), Refused> {
        let (target, settled) = match self.path(dir, path, words)? {
            Resolved::File(p) => (p, None),
            Resolved::Settled { path, by } => (path, Some(by)),
            // `chart.js:12` on a declared dependency: the package is
            // real, and it has no file in this repository to count.
            Resolved::Package => return Ok(()),
            // An ignored local file with a template: absent by design.
            Resolved::Ignored => return Ok(()),
        };
        if target.is_dir() {
            return Err(unknown(format!(
                "`{path}` is a directory, so it has no line {line}"
            )));
        }
        let lines = match std::fs::read(&target) {
            Ok(bytes) => {
                let newlines = bytes.iter().filter(|b| **b == b'\n').count() as u64;
                if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
                    newlines + 1
                } else {
                    newlines
                }
            }
            Err(e) => {
                return Err(unknown(format!(
                    "`{}` could not be read to count its lines: {e}",
                    display(self.repo, &target.to_string_lossy())
                )))
            }
        };
        if u64::from(line) > lines {
            return Err((
                Verdict::LinePastEof { lines },
                format!(
                    "`{}` has {lines} lines{}",
                    display(self.repo, &target.to_string_lossy()),
                    settled.map(|by| format!("; {by}")).unwrap_or_default()
                ),
            ));
        }
        Ok(())
    }

    fn make_target(&mut self, dir: &Path, name: &str) -> Result<(), Refused> {
        let mut seen: Vec<String> = Vec::new();
        let mut total = 0usize;
        for anchor in self.anchors(dir) {
            let manifest = self
                .makefiles
                .entry(anchor.clone())
                .or_insert_with(|| scripts::targets(&anchor))
                .clone();
            match manifest {
                Manifest::Absent => continue,
                Manifest::Unreadable(why) => {
                    return Err(unknown(format!(
                        "`make {name}` could not be checked: {} in `{}`",
                        why,
                        display(self.repo, &anchor.to_string_lossy())
                    )))
                }
                Manifest::Present(targets) => {
                    if targets.iter().any(|t| t.name == name) {
                        return Ok(());
                    }
                    match scripts::makefile_is_open_ended(&anchor) {
                        Ok(None) => {}
                        Ok(Some(why)) => {
                            return Err(unknown(format!(
                                "`make {name}` is not a target the parser can see, and the makefile in `{}` {why}",
                                display(self.repo, &anchor.to_string_lossy())
                            )))
                        }
                        // #1411: a failed read is not "closed".
                        Err(why) => {
                            return Err(unknown(format!(
                                "`make {name}` could not be checked: {why} in `{}`",
                                display(self.repo, &anchor.to_string_lossy())
                            )))
                        }
                    }
                    total += targets.len();
                    let files: BTreeSet<&str> = targets.iter().map(|t| t.file.as_str()).collect();
                    seen.push(if files.is_empty() {
                        "no manifest".to_string()
                    } else {
                        files.into_iter().collect::<Vec<_>>().join("+")
                    });
                }
            }
        }
        if seen.is_empty() {
            return Err((
                Verdict::Missing,
                "no Makefile, GNUmakefile, makefile or justfile beside the file or at the repository root".into(),
            ));
        }
        Err((
            Verdict::Missing,
            format!("{}: {total} targets, none named `{name}`", seen.join(", ")),
        ))
    }

    fn script(&mut self, dir: &Path, runner: Runner, name: &str) -> Result<(), Refused> {
        if runner == Runner::Yarn && (name.starts_with('-') || YARN_VERBS.contains(&name)) {
            return Ok(());
        }
        let mut total = 0usize;
        let mut any_manifest = false;
        for anchor in self.anchors(dir) {
            let manifest = self
                .packages
                .entry(anchor.clone())
                .or_insert_with(|| scripts::scripts(&anchor))
                .clone();
            match manifest {
                Manifest::Absent => {}
                Manifest::Unreadable(why) => {
                    return Err(unknown(format!(
                        "`{}` could not be checked: {why} in `{}`",
                        script_display(runner, name),
                        display(self.repo, &anchor.to_string_lossy())
                    )))
                }
                Manifest::Present(list) => {
                    any_manifest = true;
                    if list.iter().any(|s| s == name) {
                        return Ok(());
                    }
                    total += list.len();
                }
            }
            // A binary, not a script: `yarn vitest run` runs
            // `node_modules/.bin/vitest`. Resolved, and not a finding.
            match probe(&anchor.join("node_modules").join(".bin").join(name)) {
                Probe::Found => return Ok(()),
                Probe::Absent => {}
                Probe::Refused(e) => {
                    return Err(unknown(format!(
                        "`node_modules/.bin/{name}` under `{}` could not be checked: {e}",
                        display(self.repo, &anchor.to_string_lossy())
                    )))
                }
            }
        }
        Err((
            Verdict::Missing,
            if any_manifest {
                format!("package.json: {total} scripts, none named `{name}`; no `node_modules/.bin/{name}`")
            } else {
                format!("no package.json beside the file or at the repository root; no `node_modules/.bin/{name}`")
            },
        ))
    }

    fn skill(&mut self, name: &str) -> Result<(), Refused> {
        let Some(inv) = self.definitions else {
            return Err(unknown(format!(
                "the `{name}` skill could not be checked: no definitions inventory"
            )));
        };
        // `plugin:skill` is how a plugin's skill is invoked; the
        // inventory records the skill's own name.
        let bare = name.rsplit(':').next().unwrap_or(name);
        let skills: Vec<&str> = inv
            .definitions
            .iter()
            .filter(|d| d.kind == Kind::Skill)
            .map(|d| d.name.as_str())
            .collect();
        if skills.iter().any(|s| *s == name || *s == bare) {
            return Ok(());
        }
        if !inv.unreadable.is_empty() {
            return Err(unknown(format!(
                "no `{name}` skill in the {} readable, and {} could not be read: {}",
                count(skills.len(), "skill", "skills"),
                count(inv.unreadable.len(), "scope", "scopes"),
                inv.unreadable
                    .iter()
                    .map(|r| r.detail.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        Err((
            Verdict::Missing,
            format!(
                "{} in the inventory across every scope, none named `{name}`",
                count(skills.len(), "skill", "skills")
            ),
        ))
    }

    fn symbol(&mut self, last: &str) -> Result<(), Refused> {
        let s = &self.symbols;
        let hits = s.files.get(last).map_or(0, Vec::len);
        if hits > 0 {
            return Ok(());
        }
        if s.files_searched == 0 {
            return Err(unknown(format!(
                "`{last}` could not be checked: no source files under `{}`",
                s.roots.join("`, `")
            )));
        }
        if !s.unreadable.is_empty() {
            return Err(unknown(format!(
                "`{last}` has no whole-word hit in {} searched, and {} could not be read: {}",
                count(s.files_searched, "source file", "source files"),
                count(s.unreadable.len(), "entry", "entries"),
                s.unreadable.join("; ")
            )));
        }
        Err((
            Verdict::Missing,
            format!(
                "{} under `{}` searched for the whole word `{last}`: 0 hits",
                count(s.files_searched, "source file", "source files"),
                s.roots.join("`, `")
            ),
        ))
    }
}

fn script_display(runner: Runner, name: &str) -> String {
    match runner {
        Runner::Yarn => format!("yarn {name}"),
        Runner::Npm => format!("npm run {name}"),
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

static CUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(see|read|consult)\b").unwrap());
/// "when relevant" and "if needed" are here by their first word.
static CONDITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(when|if|for|before|on demand|as needed)\b").unwrap());
static TEMPORAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(as of|before|after|until)\b").unwrap());
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:(?:19|20)\d\d-\d\d-\d\d|(?:January|February|March|April|May|June|July|August|September|October|November|December) (?:19|20)\d\d)\b",
    )
    .unwrap()
});
static VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:v\d+(?:\.\d+)+|\d+\.\d+\.\d+)\b").unwrap());

/// Whether `p` is under a `.claude/rules` directory. Such a file loads
/// itself: at launch without `paths:` frontmatter, or when a file its
/// `paths:` matches is read. An `@` import would load it in every
/// session, defeating the scoping, so naming one is never blind.
fn is_rule_file(repo: &Path, p: &Path) -> bool {
    relative(repo, p)
        .is_some_and(|r| r.starts_with(".claude/rules/") || r.contains("/.claude/rules/"))
}

/// Whether the line states a topic before an arrow: "auth roles →
/// `docs/a.md`" says when to read the target, in the index form. An
/// arrow with nothing before it but a list marker states no topic.
fn topic_arrow(line: &str) -> bool {
    ["→", "->"].iter().any(|arrow| {
        line.find(arrow).is_some_and(|at| {
            !line[..at]
                .trim_start_matches(|c: char| c.is_whitespace() || c == '-' || c == '*')
                .trim()
                .is_empty()
        })
    })
}

/// Words that make a short label before a colon a document index:
/// "Docs:", "Reference:", "Further reading:".
const INDEX_WORDS: &[&str] = &[
    "doc",
    "docs",
    "documentation",
    "reference",
    "references",
    "reading",
];

/// Whether the line opens with a document-index label before a colon
/// (#1375). An index says "these are here for when the topic comes up",
/// which is a condition. Narrow on purpose: the label is at most four
/// words, holds no code span, and names a document list; "read this:"
/// is not one.
fn index_label(line: &str) -> bool {
    let Some((before, _)) = line.split_once(':') else {
        return false;
    };
    if before.contains('`') {
        return false;
    }
    let words: Vec<String> = before
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_lowercase)
        .collect();
    words.len() <= 4 && words.iter().any(|w| INDEX_WORDS.contains(&w.as_str()))
}

/// What an `@` import of `file` would load, estimated: the file as
/// injected (block HTML comments stripped, as `tokens::estimate` says)
/// plus everything it imports. `None` when it is not a readable file:
/// the figure is then left out, never written as zero.
fn import_cost(file: &Path) -> Option<Cost> {
    if !file.is_file() {
        return None;
    }
    let body = std::fs::read_to_string(file).ok()?;
    let children = imports::resolve_tree(file, &mut Vec::new());
    Some(Cost {
        tokens: tokens::estimate(&text::strip_block_html_comments(&body))
            + children
                .iter()
                .map(imports::ImportNode::total_tokens)
                .sum::<u64>(),
        floor: children.iter().any(imports::ImportNode::total_partial),
    })
}

/// The evidence clause pricing an import, ending in [`COST`] so the
/// brief can lift it back out.
fn cost_clause(path: &str, cost: Cost) -> String {
    format!(
        "`@{path}` would load {}~{}{COST}",
        if cost.floor { "at least " } else { "" },
        cost.tokens
    )
}

/// Document extensions, and a `docs/` component, for the blind-reference
/// rule.
fn is_document(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md")
        || lower.ends_with(".txt")
        || lower.ends_with(".rst")
        || lower.starts_with("docs/")
        || lower.contains("/docs/")
}

/// The two rules the content-shape research hands to this producer,
/// over prose lines only. `resolved_paths` are path references that
/// resolved: a missing one is already a `Missing` finding.
fn shape_rules(text: &str, resolved_paths: &[Named]) -> Vec<Shape> {
    let normalised = text.replace("\r\n", "\n");
    let mut out = Vec::new();
    for (n, line) in text::prose_lines(&normalised) {
        let conditioned = CONDITION.is_match(line) || topic_arrow(line) || index_label(line);
        let mut blind: Vec<Blind> = Vec::new();
        if CUE.is_match(line) && !conditioned {
            for named in resolved_paths.iter().filter(|p| p.line == n) {
                let path = &named.path;
                let imported = line.contains(&format!("@{path}"));
                if is_document(path) && !imported && !blind.iter().any(|b| &b.path == path) {
                    blind.push(Blind {
                        path: path.clone(),
                        settled: named.settled.clone(),
                        cost: named.file.as_deref().and_then(import_cost),
                    });
                }
            }
        }
        // One finding per line, however many documents it names: three
        // near-identical rows for one line is noise (#1320).
        if !blind.is_empty() {
            out.push(Shape::BlindReference {
                line: n,
                paths: blind,
            });
        }
        if TEMPORAL.is_match(line) && (DATE.is_match(line) || VERSION.is_match(line)) {
            out.push(Shape::DatedFact {
                line: n,
                quoted: clamp(line.trim(), 200),
            });
        }
    }
    out
}

fn clamp(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// The section heading a line falls under, as written.
fn section_at(sections: &[text::Section], line: usize) -> Option<String> {
    sections
        .iter()
        .rfind(|s| s.heading.is_some() && s.line <= line)
        .and_then(|s| {
            s.heading
                .as_deref()
                .map(|h| format!("{} {h}", "#".repeat(usize::from(s.level))))
        })
}

/// One file's [`FileRot`] as findings.
fn findings_for(repo: &Path, path: &str, scope: Scope, text: &str, rot: &FileRot) -> Vec<Finding> {
    let sections = text::sections(text);
    let shown = display(repo, path);
    let subject = |line: usize| Subject::ClaudeMd {
        path: path.to_string(),
        scope,
        section: section_at(&sections, line),
    };
    let at = |line: usize| Locator::File {
        path: path.to_string(),
        line: Some(line as u32),
    };
    let mut out = Vec::new();

    for r in &rot.findings {
        let raw = &r.r.raw;
        let line = r.r.line;
        let (severity, sentence) = match &r.verdict {
            Verdict::Missing => (
                Severity::Problem,
                if matches!(r.r.kind, RefKind::Skill { .. }) {
                    format!("`{shown}:{line}` names `{raw}`{SKILL_MISSING}")
                } else {
                    format!("`{shown}:{line}` names `{raw}`{MISSING}")
                },
            ),
            Verdict::LinePastEof { lines } => (
                Severity::Advice,
                format!("`{shown}:{line}` cites `{raw}`{PAST_EOF}{lines} lines"),
            ),
            Verdict::IgnoredWithoutTemplate => (
                Severity::Advice,
                format!("`{shown}:{line}` names `{raw}`{IGNORED}"),
            ),
            Verdict::Unknown(why) => (
                Severity::Unknown,
                format!("`{shown}:{line}` names `{raw}`{UNKNOWN}{why}"),
            ),
        };
        out.push(Finding::new(
            Check::Rot,
            severity,
            subject(line),
            vec![Evidence {
                at: at(line),
                measured: r.measured.clone(),
            }],
            sentence,
        ));
    }

    for s in &rot.shape {
        let (line, sentence, measured): (usize, String, Vec<String>) = match s {
            Shape::BlindReference { line, paths } => (
                *line,
                format!(
                    "`{shown}:{line}` names {}{BLIND}",
                    paths
                        .iter()
                        .map(|b| format!("`{}`", b.path))
                        .collect::<Vec<_>>()
                        .join(" and ")
                ),
                paths
                    .iter()
                    .map(|b| {
                        let p = &b.path;
                        let mut m = format!(
                            "line {line} names `{p}` with see/read/consult; no `@{p}` import, no when/if/for/before/on demand/as needed, no topic → and no index label on the line"
                        );
                        if let Some(by) = &b.settled {
                            m.push_str(&format!("; {by}"));
                        }
                        // Last, so the brief can lift it back out.
                        if let Some(cost) = b.cost {
                            m.push_str(&format!("; {}", cost_clause(p, cost)));
                        }
                        m
                    })
                    .collect(),
            ),
            Shape::DatedFact { line, quoted } => (
                *line,
                format!("`{shown}:{line}`{DATED}\"{quoted}\""),
                vec!["a date or version number beside as of/before/after/until".to_string()],
            ),
        };
        out.push(Finding::new(
            Check::Rot,
            Severity::Advice,
            subject(line),
            measured
                .into_iter()
                .map(|measured| Evidence {
                    at: at(line),
                    measured,
                })
                .collect(),
            sentence,
        ));
    }

    // One summary per file, ONLY when something could not be checked:
    // a clean file gets no row, and a file that checked 0 of 41 gets a
    // row that says so. A Note (#1372): it counts the Unknown rows above
    // and recommends nothing, so as Advice it restated each one twice.
    if !rot.unchecked.is_empty() {
        let unknown = rot
            .findings
            .iter()
            .filter(|r| matches!(r.verdict, Verdict::Unknown(_)))
            .count();
        out.push(Finding::new(
            Check::Rot,
            Severity::Note,
            Subject::ClaudeMd {
                path: path.to_string(),
                scope,
                section: None,
            },
            vec![Evidence {
                at: Locator::File {
                    path: path.to_string(),
                    line: None,
                },
                measured: format!(
                    "{} found: {} resolved, {unknown} unknown, {} never resolved (`cargo` commands, issue numbers, placeholder paths and absolute paths outside the repository)",
                    count(rot.refs_checked + unknown + rot.unresolvable, "reference", "references"),
                    rot.refs_checked,
                    rot.unresolvable
                ),
            }],
            format!(
                "`{shown}`: {}{SUMMARY}{unknown} could not be checked ({})",
                rot.refs_checked,
                rot.unchecked.join("; ")
            ),
        ));
    }
    out
}

/// The brief's remedy for a rot finding, chosen by the phrase the
/// sentence was built from.
pub(super) fn suggestion(f: &Finding) -> String {
    let file = f.subject.path();
    let line = f
        .evidence
        .first()
        .and_then(|e| match e.at {
            Locator::File { line, .. } => line,
            Locator::Session { .. } => None,
        })
        .map(|l| format!("line {l} of `{file}`"))
        .unwrap_or_else(|| format!("`{file}`"));
    // The per-file summary is a Note (#1372) and never gets here: the
    // brief renders a Note with no suggestion.
    let s = f.finding.as_str();
    if s.contains(SKILL_MISSING) {
        format!(
            "Edit {line}: name a skill that exists in the user, project or plugin scope, or delete \
             the reference. Do not create a skill to satisfy it."
        )
    } else if s.contains(MISSING) {
        format!(
            "Edit {line}: replace the reference with the current name of what it points at, or \
             delete the sentence. Do not create a file, target, script or symbol to satisfy it."
        )
    } else if s.contains(IGNORED) {
        format!(
            "Edit {line}: say that each developer creates this file, or commit a template beside \
             it (`<path>.example`) and name that too. Do not commit the ignored file itself."
        )
    } else if s.contains(PAST_EOF) {
        format!(
            "Edit {line}: cite the line that now holds what the sentence describes, or drop the \
             line number and cite the file alone."
        )
    } else if s.contains(BLIND) {
        // The condition first: it costs nothing per session. The import
        // is priced, per document, from the evidence (#1375); a document
        // that could not be weighed has no figure rather than zero.
        let costs: Vec<&str> = f
            .evidence
            .iter()
            .filter_map(|e| e.measured.rsplit_once("; ").map(|(_, c)| c))
            .filter(|c| c.ends_with(COST))
            .collect();
        let import = if costs.is_empty() {
            ", which loads it in every session".to_string()
        } else {
            format!(": {}", costs.join("; "))
        };
        format!(
            "Either keep it lazy by stating on {line} when to read it (\"when …\", \"before …\", \
             \"if …\", \"on demand\"), or import the document by adding `@<path>` on its own line \
             in `{file}`{import}. One or the other; not both."
        )
    } else if s.contains(DATED) {
        format!(
            "Re-verify the statement on {line}: replace the date or version with what is true \
             now, or delete the line if the constraint has passed."
        )
    } else {
        format!(
            "Nothing to edit in `{file}` for this row: the reference on {line} could not be \
             checked for the reason given. Make that input readable and re-run the check."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::definitions::{scan_scopes, Source};
    use crate::claudemd::advice::{CheckRun, Report};
    use crate::claudemd::scan_effective_opt;
    use std::fs;

    const BODY: &str = "\
# rules

The entry is `src/octo.rs`; the rule is at `src/octo.rs:9` and `src/octo.rs:11`.
The old module `src/gone.rs` moved. Run `make hello`, never `make nope`.
Run `yarn paw`, not `yarn nope`. Use the `tentacle` skill, not the `ink` skill.
`cargo test` runs it; see #123.

```
`src/fenced.rs`
```
";

    /// The sub-issue's fixture: a repository with one of everything.
    fn fixture() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::write(root.join("CLAUDE.md"), BODY).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("octo.rs"), "line\n".repeat(10)).unwrap();
        fs::write(root.join("Makefile"), "hello:\n\techo\n").unwrap();
        fs::write(root.join("package.json"), r#"{"scripts":{"paw":"x"}}"#).unwrap();
        let skill = root.join(".claude").join("skills").join("tentacle");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: tentacle\ndescription: ink\n---\nbody\n",
        )
        .unwrap();
        t
    }

    fn inventory(root: &Path) -> Inventory {
        scan_scopes(&[(
            Source::Project {
                path: root.to_string_lossy().to_string(),
            },
            root.join(".claude"),
        )])
    }

    fn run_over(repo: &Path, definitions: Option<&Inventory>) -> Report {
        let scan = scan_effective_opt(repo, None);
        let cx = Context {
            repo,
            home: None,
            scan: &scan,
            definitions,
            conn: None,
        };
        super::super::run_with(&cx, &[&Rot])
    }

    fn check_one(repo: &Path, definitions: Option<&Inventory>) -> FileRot {
        let file = repo.join("CLAUDE.md");
        let text = fs::read_to_string(&file).unwrap();
        let symbols: BTreeSet<String> = refs::extract(&text)
            .into_iter()
            .filter_map(|r| match r.kind {
                RefKind::Symbol { last } => Some(last),
                _ => None,
            })
            .collect();
        let mut res = Resolver::new(repo, definitions, &symbols, &[repo.to_path_buf()]);
        check_file(repo, &file, &text, &mut res)
    }

    /// [`check_one`] for a nested CLAUDE.md, `rel` from the root.
    fn check_nested(repo: &Path, rel: &str) -> FileRot {
        let file = rel.split('/').fold(repo.to_path_buf(), |p, s| p.join(s));
        let text = fs::read_to_string(&file).unwrap();
        let dir = file.parent().unwrap().to_path_buf();
        let mut res = Resolver::new(repo, None, &BTreeSet::new(), &[dir]);
        check_file(repo, &file, &text, &mut res)
    }

    fn coverage(report: &Report) -> CheckRun {
        report
            .checks
            .iter()
            .find(|c| c.check == Check::Rot)
            .map(|c| c.run.clone())
            .expect("the rot check is listed")
    }

    fn verdicts(rot: &FileRot) -> Vec<(&str, &Verdict)> {
        rot.findings
            .iter()
            .map(|r| (r.r.raw.as_str(), &r.verdict))
            .collect()
    }

    /// The founding case: four Missing, one LinePastEof, and every
    /// checkable reference counted as checked. Twelve references are
    /// extracted: ten resolve to an answer, and `cargo test` and `#123`
    /// are counted as never resolved.
    #[test]
    fn the_fixture_yields_four_missing_and_one_line_past_eof() {
        let t = fixture();
        let inv = inventory(t.path());
        let rot = check_one(t.path(), Some(&inv));

        assert_eq!(
            verdicts(&rot),
            vec![
                ("src/octo.rs:11", &Verdict::LinePastEof { lines: 10 }),
                ("src/gone.rs", &Verdict::Missing),
                ("make nope", &Verdict::Missing),
                ("yarn nope", &Verdict::Missing),
                ("ink", &Verdict::Missing),
            ],
            "{rot:?}"
        );
        assert_eq!(rot.refs_checked, 10, "{rot:?}");
        assert_eq!(rot.unresolvable, 2, "`cargo test` and `#123`");
        assert!(rot.unchecked.is_empty(), "{:?}", rot.unchecked);
        assert!(rot.shape.is_empty(), "{:?}", rot.shape);
    }

    /// The #1300 fixture, built from the real report: a CLAUDE.md naming
    /// an npm dependency, a slash command, and a file that really is
    /// gone.
    fn rot_1300(body: &str, package_json: Option<&str>, skill: Option<&str>) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::write(root.join("CLAUDE.md"), body).unwrap();
        fs::create_dir_all(root.join("libs/ui/src/lib/charts")).unwrap();
        if let Some(json) = package_json {
            fs::write(root.join("package.json"), json).unwrap();
        }
        if let Some(name) = skill {
            let dir = root.join(".claude").join("skills").join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: stack them\n---\nbody\n"),
            )
            .unwrap();
        }
        t
    }

    /// The user's line, verbatim in shape: `chart.js` is a declared
    /// dependency, not a missing file (#1300).
    #[test]
    fn a_declared_npm_dependency_is_not_a_missing_file() {
        let t = rot_1300(
            "Charts: `chart.js` only via the `libs/ui/src/lib/charts/` components (ng2-charts)\n",
            Some(r#"{"dependencies":{"chart.js":"^4.5.1"}}"#),
            None,
        );
        let rot = check_one(t.path(), None);
        assert_eq!(verdicts(&rot), vec![], "{rot:?}");
        assert_eq!(rot.refs_checked, 2, "{rot:?}");
    }

    /// devDependencies count, and so does a scoped name.
    #[test]
    fn a_dev_dependency_and_a_scoped_package_resolve_too() {
        let t = rot_1300(
            "Use `vitest.config.ts` via `@types/node` and `chart.js`.\n",
            Some(
                r#"{"devDependencies":{"chart.js":"^4.5.1","@types/node":"^20"},"dependencies":{}}"#,
            ),
            None,
        );
        let rot = check_one(t.path(), None);
        let missing: Vec<&str> = rot
            .findings
            .iter()
            .filter(|f| f.verdict == Verdict::Missing)
            .map(|f| f.r.raw.as_str())
            .collect();
        assert_eq!(missing, vec!["vitest.config.ts"], "{rot:?}");
    }

    /// No manifest entry and no file: nothing of that name exists here,
    /// so the verdict stays Missing. A typo'd package and a deleted file
    /// send the reader to the same line.
    #[test]
    fn an_undeclared_package_shaped_token_is_still_missing() {
        let t = rot_1300(
            "Charts: `chart.js` only via the `libs/ui/src/lib/charts/` components\n",
            Some(r#"{"dependencies":{"react":"^18"}}"#),
            None,
        );
        let rot = check_one(t.path(), None);
        assert_eq!(
            verdicts(&rot),
            vec![("chart.js", &Verdict::Missing)],
            "{rot:?}"
        );
    }

    /// The required negative: a `.js` file that really is gone is still
    /// Missing, manifest or no manifest.
    #[test]
    fn a_genuinely_missing_js_file_is_still_missing() {
        for manifest in [None, Some(r#"{"dependencies":{"chart.js":"^4.5.1"}}"#)] {
            let t = rot_1300(
                "The entry is `src/gone.js`, and `also-gone.js` beside it.\n",
                manifest,
                None,
            );
            let rot = check_one(t.path(), None);
            assert_eq!(
                verdicts(&rot),
                vec![
                    ("src/gone.js", &Verdict::Missing),
                    ("also-gone.js", &Verdict::Missing),
                ],
                "manifest={manifest:?} {rot:?}"
            );
        }
    }

    /// A `package.json` that exists and will not parse never established
    /// "not declared", so the token is Unknown, not Missing.
    #[test]
    fn an_unreadable_manifest_makes_a_package_shaped_token_unknown() {
        let t = rot_1300("Charts: `chart.js` only.\n", Some("{ not json"), None);
        let rot = check_one(t.path(), None);
        assert_eq!(rot.findings.len(), 1, "{rot:?}");
        match &rot.findings[0].verdict {
            Verdict::Unknown(why) => assert!(
                why.contains("declared dependency") && why.contains("package.json"),
                "{why}"
            ),
            other => panic!("{other:?}"),
        }
    }

    /// `/stacked-prs` is a skill invocation, not a path, and resolves
    /// against the definitions inventory (#1300).
    #[test]
    fn a_slash_command_resolves_against_the_skill_inventory() {
        let t = rot_1300("Run `/stacked-prs` to stack.\n", None, Some("stacked-prs"));
        let inv = inventory(t.path());
        let rot = check_one(t.path(), Some(&inv));
        assert_eq!(verdicts(&rot), vec![], "{rot:?}");
        assert_eq!(rot.refs_checked, 1, "{rot:?}");
    }

    /// With no inventory, "we did not look" must not read as "it is not
    /// there" (#1050): Unknown, never Missing.
    #[test]
    fn a_slash_command_with_no_inventory_is_unknown_not_missing() {
        let t = rot_1300("Run `/stacked-prs` to stack.\n", None, Some("stacked-prs"));
        let rot = check_one(t.path(), None);
        assert_eq!(rot.findings.len(), 1, "{rot:?}");
        match &rot.findings[0].verdict {
            Verdict::Unknown(why) => assert!(why.contains("no definitions inventory"), "{why}"),
            other => panic!("a slash command with no inventory must be Unknown: {other:?}"),
        }
    }

    /// A slash command with an inventory that does not hold it is
    /// Missing as a SKILL, with the skill sentence, not as a file.
    #[test]
    fn an_unbacked_slash_command_is_a_missing_skill_not_a_missing_file() {
        let t = rot_1300("Run `/stacked-prs` to stack.\n", None, Some("other"));
        let inv = inventory(t.path());
        let report = run_over(t.path(), Some(&inv));
        let problems: Vec<&str> = report
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Problem)
            .map(|f| f.finding.as_str())
            .collect();
        assert_eq!(
            problems,
            vec![
                "`CLAUDE.md:1` names `/stacked-prs`, and no skill of that name was found in any scope"
            ]
        );
    }

    /// The same fixture through the producer: severities, sentences and
    /// no summary row, because nothing was Unknown.
    #[test]
    fn missing_is_a_problem_and_past_eof_is_advice_in_its_own_row() {
        let t = fixture();
        let inv = inventory(t.path());
        let report = run_over(t.path(), Some(&inv));

        let problems: Vec<&str> = report
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Problem)
            .map(|f| f.finding.as_str())
            .collect();
        assert_eq!(
            problems,
            vec![
                "`CLAUDE.md:4` names `src/gone.rs`, which does not exist in this repository",
                "`CLAUDE.md:4` names `make nope`, which does not exist in this repository",
                "`CLAUDE.md:5` names `yarn nope`, which does not exist in this repository",
                "`CLAUDE.md:5` names `ink`, and no skill of that name was found in any scope",
            ]
        );
        let advice: Vec<&str> = report
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Advice)
            .map(|f| f.finding.as_str())
            .collect();
        assert_eq!(
            advice,
            vec!["`CLAUDE.md:3` cites `src/octo.rs:11`; the file has 10 lines"]
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.severity == Severity::Unknown),
            "{report:?}"
        );
        assert!(
            !report.brief.contains("could not be checked"),
            "no summary row when everything was checked: {}",
            report.brief
        );
        assert_eq!(coverage(&report), CheckRun::Ran { findings: 5 });
        // Every brief names the file, its line, and a remedy that is
        // not the generic one.
        for f in &report.findings {
            assert!(
                f.brief.contains("Suggested change: Edit line"),
                "{}",
                f.brief
            );
            assert!(f.subject.path().ends_with("CLAUDE.md"), "{:?}", f.subject);
            assert!(
                !f.evidence[0].measured.is_empty(),
                "measured is stated: {f:?}"
            );
        }
        // The section is carried, as written.
        match &report.findings[0].subject {
            Subject::ClaudeMd { section, .. } => assert_eq!(section.as_deref(), Some("# rules")),
            other => panic!("{other:?}"),
        }
    }

    /// The four must-not-report cases, asserted by absence.
    #[test]
    fn a_line_within_eof_cargo_a_fenced_path_and_an_issue_are_not_findings() {
        let t = fixture();
        let inv = inventory(t.path());
        let rot = check_one(t.path(), Some(&inv));
        let raws: Vec<&str> = rot.findings.iter().map(|r| r.r.raw.as_str()).collect();
        assert!(!raws.contains(&"src/octo.rs:9"), "within EOF: {raws:?}");
        assert!(!raws.contains(&"src/octo.rs"), "exists: {raws:?}");
        assert!(!raws.contains(&"cargo test"), "never a finding: {raws:?}");
        assert!(!raws.contains(&"src/fenced.rs"), "inside a fence: {raws:?}");
        assert!(!raws.contains(&"#123"), "counted, not resolved: {raws:?}");
    }

    /// The flip: delete `octo.rs`, and the two negative assertions above
    /// that depend on it can fail. `cargo test`, the fenced path and the
    /// issue number stay silent whatever the tree holds.
    #[test]
    fn deleting_octo_makes_its_references_missing() {
        let t = fixture();
        let inv = inventory(t.path());
        fs::remove_file(t.path().join("src").join("octo.rs")).unwrap();
        let rot = check_one(t.path(), Some(&inv));
        let raws: Vec<&str> = rot.findings.iter().map(|r| r.r.raw.as_str()).collect();
        assert!(raws.contains(&"src/octo.rs"), "{raws:?}");
        assert!(raws.contains(&"src/octo.rs:9"), "{raws:?}");
        assert!(raws.contains(&"src/octo.rs:11"), "{raws:?}");
        assert!(
            rot.findings
                .iter()
                .filter(|r| r.r.raw.starts_with("src/octo.rs"))
                .all(|r| r.verdict == Verdict::Missing),
            "a missing file is Missing, never past-EOF: {rot:?}"
        );
        assert!(!raws.contains(&"cargo test"), "{raws:?}");
        assert!(!raws.contains(&"src/fenced.rs"), "{raws:?}");
        assert!(!raws.contains(&"#123"), "{raws:?}");
        assert_eq!(rot.refs_checked, 10, "a Missing verdict is still a check");
    }

    /// A path that exists under the repository root but not beside the
    /// file resolves by the root; a bare filename resolves by unique
    /// suffix; a filename in two places is ambiguous, which is Unknown.
    #[test]
    fn paths_resolve_by_root_then_unique_suffix_and_two_matches_is_unknown() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("crates").join("a")).unwrap();
        fs::create_dir_all(root.join("crates").join("b")).unwrap();
        fs::write(root.join("crates").join("a").join("lib.rs"), "").unwrap();
        fs::write(root.join("crates").join("b").join("lib.rs"), "").unwrap();
        fs::write(root.join("crates").join("a").join("only.rs"), "").unwrap();
        fs::write(
            root.join("crates").join("CLAUDE.md"),
            "see `crates/a/only.rs`, `only.rs`, `lib.rs` and `a/lib.rs`\n",
        )
        .unwrap();

        let scan = scan_effective_opt(root, None);
        let cx = Context {
            repo: root,
            home: None,
            scan: &scan,
            definitions: None,
            conn: None,
        };
        let (_, _, result) = analyse(&cx).remove(0);
        let (_, rot) = result.unwrap();
        assert_eq!(rot.refs_checked, 3, "{rot:?}");
        assert_eq!(rot.findings.len(), 1, "{rot:?}");
        assert_eq!(rot.findings[0].r.raw, "lib.rs");
        match &rot.findings[0].verdict {
            Verdict::Unknown(why) => assert!(why.contains("ambiguous"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(rot.unchecked.len(), 1);
    }

    /// #1299: a repository with `.claude/worktrees/` must still have its
    /// `.claude/**` content indexed. The prune is for the agent-managed
    /// checkouts one level down, not for their parent, which holds the
    /// rules and skills a CLAUDE.md names. Before the fix every
    /// `.claude/**` reference in such a repository was Missing.
    #[test]
    fn claude_worktrees_prunes_itself_and_not_the_rest_of_dot_claude() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join(".claude").join("rules")).unwrap();
        fs::write(
            root.join(".claude").join("rules").join("unit-testing.md"),
            "rule\n",
        )
        .unwrap();
        let skill = root.join(".claude").join("skills").join("tentacle");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: tentacle\n---\nbody\n").unwrap();
        // The agent-managed checkouts, which must stay out of the index:
        // they are copies, and indexing them makes suffix matches
        // ambiguous.
        let wt = root.join(".claude").join("worktrees").join("wt1");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join("copied.md"), "copy\n").unwrap();

        let tree = index_tree(root);
        let has = |p: &str| tree.paths.iter().any(|x| x == p);
        assert!(
            has(".claude/rules/unit-testing.md"),
            "the rules live one level up from the prune: {:?}",
            tree.paths
        );
        assert!(
            has(".claude/skills/tentacle/SKILL.md"),
            "the skills do too: {:?}",
            tree.paths
        );
        assert!(
            !tree
                .paths
                .iter()
                .any(|p| p.starts_with(".claude/worktrees/")),
            "the agent checkouts stay out: {:?}",
            tree.paths
        );

        // End to end: the references resolve, so nothing is Missing.
        fs::write(
            root.join("CLAUDE.md"),
            "see `.claude/rules/unit-testing.md` and `.claude/skills/tentacle/SKILL.md`\n",
        )
        .unwrap();
        let rot = check_one(root, None);
        assert_eq!(rot.refs_checked, 2, "{rot:?}");
        assert!(rot.findings.is_empty(), "no finding at all: {rot:?}");
    }

    /// #1299's general lesson: a reference that could only live inside a
    /// pruned subtree was never looked for, so it is Unknown naming the
    /// prune, not Missing. A path outside every prune is still Missing --
    /// softening those would retire the check.
    #[test]
    fn a_path_under_a_pruned_directory_is_unknown_and_names_the_prune() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("node_modules").join("left")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            "patched in `node_modules/left/index.js`, unlike `src/gone.rs`\n",
        )
        .unwrap();

        let rot = check_one(root, None);
        let v = verdicts(&rot);
        let pruned = v
            .iter()
            .find(|(raw, _)| *raw == "node_modules/left/index.js")
            .expect("the pruned reference is a finding");
        match pruned.1 {
            Verdict::Unknown(why) => assert!(
                why.contains("node_modules") && why.contains("does not enter"),
                "the prune is named: {why}"
            ),
            other => panic!("never Missing: {other:?}"),
        }
        assert!(
            v.iter()
                .any(|(raw, verd)| *raw == "src/gone.rs" && **verd == Verdict::Missing),
            "a genuine miss outside every prune is still Missing: {v:?}"
        );
    }

    /// A subtree the walk cannot list is Unknown, not Missing, and the
    /// summary names the directory. Unix-only: the wall is a permission
    /// bit; as root the gate drops `cap_dac_override` to make it bite.
    #[cfg(unix)]
    #[test]
    fn a_reference_into_an_unreadable_directory_is_unknown_and_named() {
        use std::os::unix::fs::PermissionsExt;
        let t = fixture();
        let root = t.path();
        let walled = root.join("walled");
        fs::create_dir_all(&walled).unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            "read `walled/secret.rs` and `src/octo.rs`\n",
        )
        .unwrap();
        fs::set_permissions(&walled, fs::Permissions::from_mode(0o000)).unwrap();

        let report = run_over(root, None);
        let rot = check_one(root, None);

        fs::set_permissions(&walled, fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(rot.refs_checked, 1, "{rot:?}");
        assert_eq!(rot.findings.len(), 1, "{rot:?}");
        assert_eq!(rot.findings[0].r.raw, "walled/secret.rs");
        assert!(
            matches!(rot.findings[0].verdict, Verdict::Unknown(_)),
            "never Missing: {:?}",
            rot.findings[0].verdict
        );
        assert!(
            rot.unchecked.iter().any(|u| u.contains("walled")),
            "the directory is named: {:?}",
            rot.unchecked
        );
        // Through the producer: one Unknown row and one summary row.
        let unknown: Vec<&Finding> = report
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Unknown)
            .collect();
        assert_eq!(unknown.len(), 1, "{report:?}");
        assert!(unknown[0].finding.contains("could not be checked"));
        let summary = report
            .findings
            .iter()
            .find(|f| f.finding.contains("references checked;"))
            .expect("a summary row when K > 0");
        // #1372: the summary counts; it recommends nothing, and restating
        // the Unknown row as advice showed it twice.
        assert_eq!(summary.severity, Severity::Note);
        assert!(
            summary.brief.contains("Observation only") && !summary.brief.contains("Suggested"),
            "{}",
            summary.brief
        );
        assert!(
            summary
                .finding
                .contains("1 references checked; 1 could not be checked (")
                && summary.finding.contains("walled"),
            "{}",
            summary.finding
        );
        assert!(!report
            .findings
            .iter()
            .any(|f| f.severity == Severity::Problem));
    }

    /// With no inventory a skill is Unknown, never Missing (#1050: "we
    /// did not look" is not "it is not there"). With one, a skill that
    /// exists is silent and one that does not is Missing.
    #[test]
    fn a_skill_is_unknown_without_an_inventory_and_missing_with_one() {
        let t = fixture();
        let without = check_one(t.path(), None);
        let skills: Vec<(&str, &Verdict)> = verdicts(&without)
            .into_iter()
            .filter(|(raw, _)| *raw == "tentacle" || *raw == "ink")
            .collect();
        assert_eq!(skills.len(), 2, "{without:?}");
        for (raw, v) in &skills {
            match v {
                Verdict::Unknown(why) => {
                    assert!(why.contains("no definitions inventory"), "{raw}: {why}")
                }
                other => panic!("{raw} must be Unknown, not {other:?}"),
            }
        }
        assert!(without
            .unchecked
            .iter()
            .any(|u| u.contains("no definitions inventory")));

        let inv = inventory(t.path());
        let with = check_one(t.path(), Some(&inv));
        let skills: Vec<(&str, &Verdict)> = verdicts(&with)
            .into_iter()
            .filter(|(raw, _)| *raw == "tentacle" || *raw == "ink")
            .collect();
        assert_eq!(skills, vec![("ink", &Verdict::Missing)], "{with:?}");
    }

    /// A scope the inventory could not read makes a missing skill
    /// Unknown: it may be in the scope that was walled.
    #[test]
    fn a_missing_skill_is_unknown_when_a_scope_was_unreadable() {
        let t = fixture();
        let mut inv = inventory(t.path());
        inv.unreadable
            .push(crate::claude::definitions::ScopeRefusal {
                source: Source::User,
                detail: "/home/octocat/.claude/skills (Permission denied)".into(),
            });
        let rot = check_one(t.path(), Some(&inv));
        let ink = rot.findings.iter().find(|r| r.r.raw == "ink").unwrap();
        match &ink.verdict {
            Verdict::Unknown(why) => assert!(why.contains("Permission denied"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    /// `yarn <x>` resolves against `node_modules/.bin/<x>` when it is not
    /// a script, and a yarn verb is never a finding.
    #[test]
    fn a_binary_and_a_yarn_verb_resolve_without_a_finding() {
        let t = fixture();
        let bin = t.path().join("node_modules").join(".bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("vitest"), "").unwrap();
        fs::write(
            t.path().join("CLAUDE.md"),
            "`yarn vitest run`, `yarn install --immutable`, `yarn run paw`, `npm run paw`, `yarn gone`\n",
        )
        .unwrap();
        let rot = check_one(t.path(), None);
        assert_eq!(
            verdicts(&rot),
            vec![("yarn gone", &Verdict::Missing)],
            "{rot:?}"
        );
        assert_eq!(rot.refs_checked, 5);
        assert!(rot.findings[0]
            .measured
            .contains("no `node_modules/.bin/gone`"));
    }

    /// A makefile that includes another file can hold the target where
    /// the parser cannot see it, so a miss is Unknown, not Missing.
    #[test]
    fn a_make_target_missing_from_an_including_makefile_is_unknown() {
        let t = fixture();
        fs::write(
            t.path().join("Makefile"),
            "include rules.mk\nhello:\n\techo\n",
        )
        .unwrap();
        let rot = check_one(t.path(), None);
        let nope = rot
            .findings
            .iter()
            .find(|r| r.r.raw == "make nope")
            .unwrap();
        assert!(
            matches!(&nope.verdict, Verdict::Unknown(why) if why.contains("includes other files")),
            "{:?}",
            nope.verdict
        );
        // And `make hello` still resolves.
        assert!(!rot.findings.iter().any(|r| r.r.raw == "make hello"));
    }

    /// #1411: a makefile that exists and cannot be read makes `make nope`
    /// Unknown, naming the makefile and the error, never Missing.
    #[cfg(unix)]
    #[test]
    fn a_make_target_against_an_unreadable_makefile_is_unknown() {
        use std::os::unix::fs::PermissionsExt;
        let t = fixture();
        let p = t.path().join("Makefile");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o000)).unwrap();
        let blocked = fs::read(&p).is_err();
        let rot = check_one(t.path(), None);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        if !blocked {
            eprintln!("skipped: mode 0o000 did not block the read (running as root?)");
            return;
        }
        let nope = rot
            .findings
            .iter()
            .find(|r| r.r.raw == "make nope")
            .unwrap();
        assert!(
            matches!(&nope.verdict, Verdict::Unknown(why) if why.contains("Makefile")),
            "{:?}",
            nope.verdict
        );
    }

    /// A symbol is a whole-word hit under the search roots; a symbol no
    /// file holds is Missing; a repository with no source file cannot
    /// check symbols at all.
    #[test]
    fn symbols_resolve_by_whole_word_over_the_source_roots() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src").join("lib.rs"),
            "pub fn seeded_for_test() {}\nconst OBSERVED_REMAINING_X: u8 = 0;\n",
        )
        .unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            "`Budget::seeded_for_test`, `OBSERVED_REMAINING`, `gone()`\n",
        )
        .unwrap();
        let rot = check_one(root, None);
        assert_eq!(
            verdicts(&rot),
            vec![
                ("OBSERVED_REMAINING", &Verdict::Missing),
                ("gone()", &Verdict::Missing)
            ],
            "a prefix of a longer identifier is not a whole-word hit: {rot:?}"
        );
        assert!(
            rot.findings[0]
                .measured
                .contains("1 source file under `the repository root`"),
            "{}",
            rot.findings[0].measured
        );

        // No source file at all: Unknown, never "0 hits" over nothing.
        fs::remove_dir_all(root.join("src")).unwrap();
        let rot = check_one(root, None);
        assert!(
            rot.findings.iter().all(
                |r| matches!(&r.verdict, Verdict::Unknown(why) if why.contains("no source files"))
            ),
            "{rot:?}"
        );
        assert_eq!(rot.refs_checked, 0);
    }

    /// A placeholder path states a convention, not a file, and is
    /// counted rather than resolved; the same sentence with a real name
    /// is checked.
    #[test]
    fn a_placeholder_path_is_counted_not_resolved_and_a_real_name_is_checked() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::write(
            root.join("CLAUDE.md"),
            "`check-foo.py` has `check-foo.test.py`; `src/bar/lib.rs:9` too.\n",
        )
        .unwrap();
        let rot = check_one(root, None);
        assert!(rot.findings.is_empty(), "{rot:?}");
        assert_eq!(rot.refs_checked, 0);
        assert_eq!(rot.unresolvable, 3);

        // The flip: a real name in the same sentence is Missing.
        fs::write(
            root.join("CLAUDE.md"),
            "`check-real.py` has `check-real.test.py`\n",
        )
        .unwrap();
        let rot = check_one(root, None);
        assert_eq!(
            verdicts(&rot),
            vec![
                ("check-real.py", &Verdict::Missing),
                ("check-real.test.py", &Verdict::Missing)
            ]
        );
        assert_eq!(rot.unresolvable, 0);
    }

    /// A document named with "see" and no import and no condition is a
    /// blind reference; the brief offers both remedies.
    #[test]
    fn a_blind_reference_is_advice_offering_an_import_or_a_condition() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs").join("style.md"), "style").unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            "See `docs/style.md` for the house style.\nRead `docs/style.md`.\n",
        )
        .unwrap();
        let report = run_over(root, None);
        let blind: Vec<&Finding> = report
            .findings
            .iter()
            .filter(|f| f.finding.contains("by name; not imported, no condition"))
            .collect();
        assert_eq!(blind.len(), 1, "{report:?}");
        assert_eq!(
            blind[0].finding,
            "`CLAUDE.md:2` names `docs/style.md` by name; not imported, no condition"
        );
        assert_eq!(blind[0].severity, Severity::Advice);
        assert!(blind[0].brief.contains("`@<path>`"), "{}", blind[0].brief);
        assert!(
            blind[0].brief.contains("keep it lazy"),
            "{}",
            blind[0].brief
        );
    }

    /// The negatives: an `@` import, a condition, a non-document path and
    /// a missing document are not blind references.
    #[test]
    fn an_import_a_condition_a_source_path_and_a_missing_doc_are_not_blind() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("docs").join("style.md"), "style").unwrap();
        fs::write(root.join("src").join("lib.rs"), "").unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            "Read `docs/style.md`: @docs/style.md\n\
             Read `docs/style.md` when styling.\n\
             Read `docs/style.md` before pushing.\n\
             See `src/lib.rs`.\n\
             See `docs/gone.md`.\n",
        )
        .unwrap();
        let rot = check_one(root, None);
        assert!(rot.shape.is_empty(), "{:?}", rot.shape);
        // The missing document is reported as Missing, once, not as
        // blind.
        assert_eq!(verdicts(&rot), vec![("docs/gone.md", &Verdict::Missing)]);
    }

    /// #1320: a `.claude/rules` file loads itself -- at launch, or when
    /// a file its `paths:` matches is read -- so offering an `@` import
    /// for it is wrong, and naming it is not blind.
    #[test]
    fn a_claude_rules_file_is_never_a_blind_reference() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join(".claude").join("rules")).unwrap();
        fs::write(root.join(".claude").join("rules").join("x.md"), "rule").unwrap();
        fs::write(root.join("CLAUDE.md"), "See `.claude/rules/x.md`.\n").unwrap();
        let rot = check_one(root, None);
        assert!(rot.findings.is_empty(), "{rot:?}");
        assert!(rot.shape.is_empty(), "{:?}", rot.shape);
    }

    /// #1320: "topic → path" states when to read the target, in the
    /// index form; `->` too. An arrow with nothing before it states no
    /// topic and conditions nothing.
    #[test]
    fn a_topic_arrow_is_a_condition() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs").join("a.md"), "a").unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            "- auth roles, seed model → see `docs/a.md`\n\
             - evidence -> read `docs/a.md`\n\
             → see `docs/a.md`\n",
        )
        .unwrap();
        let rot = check_one(root, None);
        let lines: Vec<usize> = rot
            .shape
            .iter()
            .filter_map(|s| match s {
                Shape::BlindReference { line, .. } => Some(*line),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec![3], "{:?}", rot.shape);
    }

    /// #1375: "on demand", "as needed", "when relevant" and "if needed"
    /// are conditions, and so is a document-index label before a colon.
    /// A colon after an ordinary phrase is not an index.
    #[test]
    fn on_demand_as_needed_and_a_document_index_are_conditions() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs").join("a.md"), "a").unwrap();
        fs::write(root.join("docs").join("b.md"), "b").unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            "Docs, read on demand: `docs/a.md`, `docs/b.md`\n\
             Read `docs/a.md` as needed.\n\
             See `docs/a.md` when relevant.\n\
             Read `docs/b.md` if needed.\n\
             - Reference: see `docs/a.md`\n\
             **Further reading:** read `docs/b.md`\n\
             Docs: see `docs/a.md`\n\
             See `docs/a.md`.\n\
             Run the gate, then read this: `docs/b.md`\n",
        )
        .unwrap();
        let rot = check_one(root, None);
        let lines: Vec<usize> = rot
            .shape
            .iter()
            .filter_map(|s| match s {
                Shape::BlindReference { line, .. } => Some(*line),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec![8, 9], "{:?}", rot.shape);
    }

    /// #1375: the brief offers the condition first, and prices the
    /// import with the token estimate of the target and what IT imports.
    #[test]
    fn a_blind_reference_brief_offers_the_condition_first_and_prices_the_import() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let docs = root.join("docs");
        fs::create_dir_all(&docs).unwrap();
        // 400 characters: 100 est. tokens.
        fs::write(docs.join("x.md"), "x".repeat(400)).unwrap();
        // "@y.md\n" is 6 characters (2 tokens), and y.md 40 (10 tokens).
        fs::write(docs.join("z.md"), "@y.md\n").unwrap();
        fs::write(docs.join("y.md"), "y".repeat(40)).unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            "See `docs/x.md`.\nSee `docs/z.md`.\n",
        )
        .unwrap();
        let report = run_over(root, None);
        let blind: Vec<&Finding> = report
            .findings
            .iter()
            .filter(|f| f.finding.contains(BLIND))
            .collect();
        assert_eq!(blind.len(), 2, "{report:?}");
        let x = &blind[0].brief;
        assert!(
            x.contains("`@docs/x.md` would load ~100 est. tokens in every session"),
            "{x}"
        );
        let lazy = x.find("keep it lazy").expect("the condition is offered");
        let import = x.find("`@<path>`").expect("the import is offered");
        assert!(lazy < import, "the condition comes first: {x}");
        assert!(
            blind[1]
                .brief
                .contains("`@docs/z.md` would load ~12 est. tokens in every session"),
            "what the target imports loads too: {}",
            blind[1].brief
        );
    }

    /// #1375: a target that cannot be read for the estimate has no
    /// figure. Never 0: a directory weighs something unmeasured.
    #[test]
    fn a_blind_reference_with_no_readable_target_states_no_figure() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("docs").join("guide")).unwrap();
        fs::write(root.join("CLAUDE.md"), "See `docs/guide/`.\n").unwrap();
        let report = run_over(root, None);
        let blind: Vec<&Finding> = report
            .findings
            .iter()
            .filter(|f| f.finding.contains(BLIND))
            .collect();
        assert_eq!(blind.len(), 1, "{report:?}");
        let brief = &blind[0].brief;
        assert!(!brief.contains("est. tokens"), "{brief}");
        assert!(!brief.contains("~0"), "{brief}");
        assert!(brief.contains("`@<path>`"), "{brief}");
    }

    /// #1320: one blind line naming two documents is ONE finding, with
    /// each document as its own evidence.
    #[test]
    fn one_blind_line_naming_two_documents_is_one_finding() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs").join("a.md"), "a").unwrap();
        fs::write(root.join("docs").join("b.md"), "b").unwrap();
        fs::write(root.join("CLAUDE.md"), "See `docs/a.md` and `docs/b.md`.\n").unwrap();
        let report = run_over(root, None);
        let blind: Vec<&Finding> = report
            .findings
            .iter()
            .filter(|f| f.finding.contains("by name; not imported, no condition"))
            .collect();
        assert_eq!(blind.len(), 1, "{report:?}");
        assert_eq!(
            blind[0].finding,
            "`CLAUDE.md:1` names `docs/a.md` and `docs/b.md` by name; not imported, no condition"
        );
        let measured: Vec<&str> = blind[0]
            .evidence
            .iter()
            .map(|e| e.measured.as_str())
            .collect();
        assert_eq!(measured.len(), 2, "{measured:?}");
        assert!(measured[0].contains("`docs/a.md`"), "{measured:?}");
        assert!(measured[1].contains("`docs/b.md`"), "{measured:?}");
    }

    /// A date or a version beside "as of"/"before"/"after"/"until" is a
    /// dated fact, quoted; a date alone or a temporal word alone is not.
    #[test]
    fn a_dated_fact_is_advice_quoting_the_line_and_its_negatives_are_silent() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::write(
            root.join("CLAUDE.md"),
            "As of 2026-09-21 the API is v2.\n\
             Until v5.20.0 the tag was burned.\n\
             Released 2026-09-21.\n\
             Run it before pushing.\n\
             After March 2026 the flag is gone.\n",
        )
        .unwrap();
        let rot = check_one(root, None);
        let dated: Vec<(usize, &str)> = rot
            .shape
            .iter()
            .filter_map(|s| match s {
                Shape::DatedFact { line, quoted } => Some((*line, quoted.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            dated,
            vec![
                (1, "As of 2026-09-21 the API is v2."),
                (2, "Until v5.20.0 the tag was burned."),
                (5, "After March 2026 the flag is gone."),
            ]
        );
        let report = run_over(root, None);
        let f = report
            .findings
            .iter()
            .find(|f| f.finding.contains("states a dated fact"))
            .unwrap();
        assert_eq!(
            f.finding,
            "`CLAUDE.md:1` states a dated fact: \"As of 2026-09-21 the API is v2.\""
        );
        assert!(f.brief.contains("Re-verify"), "{}", f.brief);
    }

    /// A repository whose only directory is walled and holds no readable
    /// CLAUDE.md is Unknown as a whole: the file may be behind the wall.
    #[cfg(unix)]
    #[test]
    fn no_file_read_and_a_walled_directory_is_unknown_as_a_whole() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let walled = t.path().join("walled");
        fs::create_dir_all(&walled).unwrap();
        fs::set_permissions(&walled, fs::Permissions::from_mode(0o000)).unwrap();
        let report = run_over(t.path(), None);
        fs::set_permissions(&walled, fs::Permissions::from_mode(0o755)).unwrap();
        match coverage(&report) {
            CheckRun::Unknown { reason } => assert!(reason.contains("walled"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    /// The global scope is not checked: its paths are about no
    /// repository in particular.
    #[test]
    fn the_global_file_is_out_of_scope() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude").join("CLAUDE.md"),
            "see `src/nowhere.rs`\n",
        )
        .unwrap();
        let repo = t.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("CLAUDE.md"), "clean\n").unwrap();
        let scan = crate::claudemd::scan_effective_in(&repo, &home);
        assert_eq!(scan.extra.len(), 1, "the global file was read: {scan:?}");
        let cx = Context {
            repo: &repo,
            home: Some(&home),
            scan: &scan,
            definitions: None,
            conn: None,
        };
        let report = super::super::run_with(&cx, &[&Rot]);
        assert!(report.findings.is_empty(), "{report:?}");
    }

    /// #1316: `/api/v1` is a URL route, not a file. An absolute token
    /// outside the repository is never resolved against the host
    /// filesystem: counted, never a finding, and never Missing.
    #[test]
    fn an_absolute_token_outside_the_repository_is_counted_not_missing() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::write(
            root.join("CLAUDE.md"),
            "dev server, proxied at `/api/v1` → `localhost:8000`\n",
        )
        .unwrap();
        let rot = check_one(root, None);
        assert!(rot.findings.is_empty(), "{rot:?}");
        assert_eq!(rot.refs_checked, 0, "{rot:?}");
        assert_eq!(rot.unresolvable, 1, "{rot:?}");
    }

    /// #1316's companion: whether the HOST has a file must not move a
    /// verdict. One absolute path outside the repository exists on disk
    /// and one does not; both are counted the same way, so neither was
    /// stat'd. `/etc/hosts` is the real-world shape of the first.
    #[cfg(unix)]
    #[test]
    fn an_absolute_path_outside_the_repository_is_never_read() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        let outside = t.path().join("outside");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("real.md"), "host file\n").unwrap();
        let body = format!(
            "see `{}`, `{}:3` and `/etc/hosts`\n",
            outside.join("real.md").display(),
            outside.join("gone.md").display()
        );
        fs::write(repo.join("CLAUDE.md"), &body).unwrap();
        let rot = check_one(&repo, None);
        assert!(rot.findings.is_empty(), "{body}: {rot:?}");
        assert_eq!(rot.refs_checked, 0, "nothing outside was resolved: {rot:?}");
        assert_eq!(rot.unresolvable, 3, "{rot:?}");
    }

    /// An absolute path that lies UNDER the repository is a repository
    /// path: its prefix is stripped and it resolves as a relative one,
    /// so a real file is silent and a gone one is still Missing.
    #[cfg(unix)]
    #[test]
    fn an_absolute_path_under_the_repository_resolves_as_relative() {
        let t = fixture();
        let root = t.path();
        let body = format!(
            "`{}` and `{}`\n",
            root.join("src").join("octo.rs").display(),
            root.join("src").join("gone.rs").display()
        );
        fs::write(root.join("CLAUDE.md"), &body).unwrap();
        let rot = check_one(root, None);
        let gone = root.join("src").join("gone.rs").display().to_string();
        assert_eq!(
            verdicts(&rot),
            vec![(gone.as_str(), &Verdict::Missing)],
            "{rot:?}"
        );
        assert_eq!(rot.refs_checked, 2, "{rot:?}");
        assert_eq!(rot.unresolvable, 0, "{rot:?}");
    }

    /// #1317: `../../fixtures` in a nested CLAUDE.md is an import
    /// specifier, relative to a spec file somewhere below. Stripped of
    /// its `../` segments, it has one match under the file's own
    /// directory, so it resolves; one with no such match is Unknown,
    /// never Missing.
    #[test]
    fn a_parent_relative_path_resolves_under_its_own_subtree_or_is_unknown() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("sub").join("src").join("fixtures")).unwrap();
        fs::write(
            root.join("sub").join("CLAUDE.md"),
            "imports one level deeper: `../../fixtures`, `../helpers`\n",
        )
        .unwrap();
        let rot = check_nested(root, "sub/CLAUDE.md");
        assert_eq!(rot.findings.len(), 1, "{rot:?}");
        assert_eq!(rot.findings[0].r.raw, "../helpers");
        match &rot.findings[0].verdict {
            Verdict::Unknown(why) => assert!(why.contains("unstated base"), "{why}"),
            other => panic!("never Missing: {other:?}"),
        }
        assert_eq!(rot.refs_checked, 1, "`../../fixtures` resolved: {rot:?}");
    }

    /// #1317's containment half: a `../` path that would climb out of
    /// the repository is never probed. A directory of that name beside
    /// the repository must not make it resolve; nothing inside matches,
    /// so it is Unknown.
    #[test]
    fn a_parent_relative_path_never_probes_outside_the_repository() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        fs::create_dir_all(repo.join("sub")).unwrap();
        // `repo/sub/../../fixtures` is exactly this directory.
        fs::create_dir_all(t.path().join("fixtures")).unwrap();
        fs::write(repo.join("sub").join("CLAUDE.md"), "see `../../fixtures`\n").unwrap();
        let rot = check_nested(&repo, "sub/CLAUDE.md");
        assert_eq!(rot.findings.len(), 1, "{rot:?}");
        match &rot.findings[0].verdict {
            Verdict::Unknown(why) => assert!(why.contains("unstated base"), "{why}"),
            other => panic!("resolved or Missing from outside the repository: {other:?}"),
        }
        assert_eq!(rot.refs_checked, 0, "{rot:?}");
    }

    /// A `../` path that DOES resolve inside the repository from the
    /// file's own directory still resolves, and a gone one from there
    /// is Unknown rather than Missing: the base was never stated.
    #[test]
    fn a_parent_relative_path_inside_the_repository_still_resolves() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("apps").join("web")).unwrap();
        fs::create_dir_all(root.join("apps").join("api")).unwrap();
        fs::write(root.join("apps").join("api").join("main.py"), "").unwrap();
        fs::write(
            root.join("apps").join("web").join("CLAUDE.md"),
            "the API is `../api/main.py`; not `../api/gone.py`\n",
        )
        .unwrap();
        let rot = check_nested(root, "apps/web/CLAUDE.md");
        assert_eq!(rot.refs_checked, 1, "{rot:?}");
        assert_eq!(rot.findings.len(), 1, "{rot:?}");
        assert!(
            matches!(rot.findings[0].verdict, Verdict::Unknown(_)),
            "{rot:?}"
        );
    }

    /// #1319: a nested CLAUDE.md describes its own directory. A suffix
    /// with one match under it resolves, whatever sibling apps hold.
    #[test]
    fn an_ambiguous_suffix_resolves_by_its_one_match_in_the_own_subtree() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("a").join("src").join("helpers")).unwrap();
        fs::create_dir_all(root.join("b").join("src").join("helpers")).unwrap();
        fs::write(
            root.join("a").join("CLAUDE.md"),
            "shared code in `helpers/`\n",
        )
        .unwrap();
        let rot = check_nested(root, "a/CLAUDE.md");
        assert!(rot.findings.is_empty(), "{rot:?}");
        assert_eq!(rot.refs_checked, 1, "{rot:?}");
    }

    /// Two matches under the own directory is still ambiguous. Never a
    /// guess.
    #[test]
    fn two_matches_in_the_own_subtree_are_still_ambiguous() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("a").join("x").join("helpers")).unwrap();
        fs::create_dir_all(root.join("a").join("y").join("helpers")).unwrap();
        fs::create_dir_all(root.join("b").join("helpers")).unwrap();
        fs::write(
            root.join("a").join("CLAUDE.md"),
            "shared code in `helpers/`\n",
        )
        .unwrap();
        let rot = check_nested(root, "a/CLAUDE.md");
        assert_eq!(rot.findings.len(), 1, "{rot:?}");
        match &rot.findings[0].verdict {
            Verdict::Unknown(why) => assert!(
                why.contains("ambiguous")
                    && why.contains("a/x/helpers")
                    && !why.contains("b/helpers"),
                "the own-subtree matches are named: {why}"
            ),
            other => panic!("{other:?}"),
        }
    }

    /// A tree where `src/testing` lies under three apps and a library,
    /// so the suffix alone is ambiguous.
    fn four_projects() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        for p in [["apps", "a"], ["apps", "b"], ["apps", "c"], ["libs", "d"]] {
            let testing = root.join(p[0]).join(p[1]).join("src").join("testing");
            fs::create_dir_all(&testing).unwrap();
            fs::write(testing.join("main.rs"), "one\n").unwrap();
        }
        t
    }

    /// #1372: an ambiguous suffix is settled by a word on the same line
    /// that equals a path segment of exactly one candidate. A line
    /// naming none of them, or two, stays Unknown: never a guess.
    #[test]
    fn a_word_on_the_line_settles_an_ambiguous_suffix_or_it_stays_unknown() {
        let t = four_projects();
        let root = t.path();
        fs::write(
            root.join("CLAUDE.md"),
            "- c specs and `src/testing`\n\
             - shared specs in `src/testing`\n\
             - a and c both use `src/testing`\n",
        )
        .unwrap();
        let rot = check_one(root, None);
        let lines: Vec<usize> = rot.findings.iter().map(|f| f.r.line).collect();
        assert_eq!(lines, vec![2, 3], "line 1 is settled by `c`: {rot:?}");
        for f in &rot.findings {
            match &f.verdict {
                Verdict::Unknown(why) => assert!(why.contains("ambiguous"), "{why}"),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(rot.refs_checked, 1, "{rot:?}");
    }

    /// #1372: the settling word may sit on a continuation line of the
    /// same list item, and a word every candidate shares settles
    /// nothing. The evidence names the word that settled it.
    #[test]
    fn a_settled_suffix_names_the_word_in_its_evidence() {
        let t = four_projects();
        let root = t.path();
        fs::write(
            root.join("CLAUDE.md"),
            "- the entry is `src/testing/main.rs:9`\n  for project c\n\
             - something else in `src/testing`\n",
        )
        .unwrap();
        let rot = check_one(root, None);
        assert_eq!(rot.findings.len(), 2, "{rot:?}");
        let past = &rot.findings[0];
        assert_eq!(past.verdict, Verdict::LinePastEof { lines: 1 }, "{rot:?}");
        assert!(
            past.measured.contains("`c`") && past.measured.contains("apps/c/src/testing/main.rs"),
            "the settling word is named: {}",
            past.measured
        );
        // The second item's words do not reach back into the first, and
        // the first item's `c` does not reach forward.
        assert!(
            matches!(rot.findings[1].verdict, Verdict::Unknown(_)),
            "{rot:?}"
        );
    }

    fn git_init(dir: &Path) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["init", "-q"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "git init");
    }

    /// The #1318 fixture: a git repository ignoring `**/.env`, with the
    /// committed template beside where the local file would be.
    fn ignored_env_fixture() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        git_init(root);
        fs::write(root.join(".gitignore"), "**/.env\n").unwrap();
        fs::create_dir_all(root.join("tools").join("stack")).unwrap();
        fs::write(
            root.join("tools").join("stack").join(".env.example"),
            "DB_PASSWORD=\n",
        )
        .unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            "Needs `tools/stack/.env` (DB password); `tools/stack/gone.rs` is gone.\n",
        )
        .unwrap();
        t
    }

    /// #1318: a gitignored local file is expected to be absent from a
    /// checkout. With a template beside it, it is silent; the genuinely
    /// gone file on the same line is still a Problem.
    #[test]
    fn an_ignored_file_with_a_template_beside_it_is_not_missing() {
        let t = ignored_env_fixture();
        let report = run_over(t.path(), None);
        let problems: Vec<&str> = report
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Problem)
            .map(|f| f.finding.as_str())
            .collect();
        assert_eq!(
            problems,
            vec!["`CLAUDE.md:1` names `tools/stack/gone.rs`, which does not exist in this repository"],
            "{report:?}"
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.finding.contains("tools/stack/.env`")),
            "{report:?}"
        );
        // The measurement names what it walked, not "tracked" paths.
        let gone = report
            .findings
            .iter()
            .find(|f| f.severity == Severity::Problem)
            .unwrap();
        assert!(
            gone.evidence[0]
                .measured
                .contains("paths in the working tree")
                && !gone.evidence[0].measured.contains("tracked"),
            "{}",
            gone.evidence[0].measured
        );
    }

    /// Delete the template, and the ignored file is Advice -- a
    /// developer has nothing to copy it from -- never a Problem.
    #[test]
    fn an_ignored_file_with_no_template_is_advice_not_a_problem() {
        let t = ignored_env_fixture();
        fs::remove_file(t.path().join("tools").join("stack").join(".env.example")).unwrap();
        let report = run_over(t.path(), None);
        let env: Vec<&Finding> = report
            .findings
            .iter()
            .filter(|f| f.finding.contains("`tools/stack/.env`"))
            .collect();
        assert_eq!(env.len(), 1, "{report:?}");
        assert_eq!(env[0].severity, Severity::Advice);
        assert!(env[0].finding.contains("git ignores"), "{}", env[0].finding);
        assert!(env[0].brief.contains("template"), "{}", env[0].brief);
    }

    /// A git that cannot run never establishes "not ignored", so the
    /// absent file is Unknown, not Missing.
    #[test]
    fn a_git_that_cannot_run_makes_an_absent_path_unknown() {
        let t = ignored_env_fixture();
        let root = t.path();
        let file = root.join("CLAUDE.md");
        let text = fs::read_to_string(&file).unwrap();
        let mut res = Resolver::new(root, None, &BTreeSet::new(), &[root.to_path_buf()])
            .with_git(Path::new("/home/octocat/no-such-git"));
        let rot = check_file(root, &file, &text, &mut res);
        assert_eq!(rot.findings.len(), 2, "{rot:?}");
        for r in &rot.findings {
            match &r.verdict {
                Verdict::Unknown(why) => {
                    assert!(why.contains("git check-ignore could not run"), "{why}")
                }
                other => panic!("{}: never Missing: {other:?}", r.r.raw),
            }
        }
    }

    /// This repository's own CLAUDE.md files, measured: zero certain
    /// rot, and the one real drift case -- `claude/sessions.rs:444` no
    /// longer holds the sentence it cites, but the file has more than
    /// 444 lines -- stays silent. Renaming a file a CLAUDE.md names
    /// fails this test until the CLAUDE.md is updated, which is the
    /// check's purpose.
    #[test]
    fn this_checkouts_own_claude_md_files_have_no_certain_rot() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri sits under the repository root");
        assert!(repo.join("CLAUDE.md").is_file(), "{}", repo.display());
        let inv = inventory(repo);
        let scan = scan_effective_opt(repo, None);
        let cx = Context {
            repo,
            home: None,
            scan: &scan,
            definitions: Some(&inv),
            conn: None,
        };

        let mut total_checked = 0;
        let mut total_unknown = 0;
        for (path, _, result) in analyse(&cx) {
            let (_, rot) = result.unwrap_or_else(|e| panic!("{path}: {e}"));
            println!(
                "{}: {} checked, {} unknown, {} never resolved, findings {:?}, shape {:?}",
                display(repo, &path),
                rot.refs_checked,
                rot.unchecked.len(),
                rot.unresolvable,
                rot.findings
                    .iter()
                    .map(|r| (r.r.raw.as_str(), r.verdict.clone()))
                    .collect::<Vec<_>>(),
                rot.shape
            );
            total_checked += rot.refs_checked;
            total_unknown += rot.unchecked.len();
            assert!(
                !rot.findings.iter().any(|r| r.verdict == Verdict::Missing),
                "{path}: {:?}",
                rot.findings
            );
            assert!(
                !rot.findings
                    .iter()
                    .any(|r| r.r.raw == "claude/sessions.rs:444"),
                "a line within EOF is silent: {:?}",
                rot.findings
            );
        }
        assert!(total_checked > 0, "the check read nothing");
        assert_eq!(
            total_unknown, 0,
            "every reference in this checkout is checkable"
        );
    }
}
