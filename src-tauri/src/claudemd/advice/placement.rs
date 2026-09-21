//! Content in the wrong file: a CLAUDE.md section whose named paths all
//! fall under one subdirectory, plus the two rules the content-shape
//! research hands here -- a run of lines duplicated between two files one
//! session loads, and an all-caps rule in a file that loads lazily.
//!
//! # The cost model is Claude Code's, not ours
//!
//! The memory docs, verbatim: "CLAUDE.md and CLAUDE.local.md files in the
//! directory hierarchy above the working directory are loaded at launch.
//! Files in subdirectories load on demand when Claude reads files in
//! those directories." A root section that is entirely about
//! `packages/api/` costs every session in the repository its tokens; the
//! same text in `packages/api/CLAUDE.md` costs only the sessions that
//! read there. Imports do not help: "imported files still load and enter
//! the context window at launch".
//!
//! # Where a section points, never where its rule applies
//!
//! The heuristic reads which paths a section names. It cannot know
//! whether the rule those paths illustrate applies only there -- a rule
//! for every component author whose implementation happens to live in
//! `src/lib/` looks the same as a rule about `src/lib/`. So the finding
//! says "names only paths under" and never "belongs in", the severity is
//! [`Severity::Advice`], and the reader decides.
//!
//! # Resolution is against two places, never by suffix
//!
//! A candidate resolves against the file's own directory, then the
//! repository root, with `is_file() || is_dir()`. Never by suffix search:
//! this repository's root file names `claude/cli.rs:128`, which exists
//! only under `src-tauri/src/`, and a suffix search would file a
//! repo-wide rule under `src-tauri`, the wrong answer. That path is
//! reported as not found from this file and does not vote.
//!
//! # Votes, and what suppresses a finding
//!
//! Each resolved path votes for its first segment below the file's
//! directory; a file at the file's own level (`CLAUDE.md`, `Makefile`)
//! votes stay; a path outside the file's directory votes elsewhere; a
//! path that does not resolve is listed and does not vote. A finding
//! needs at least two distinct resolved paths, one segment carrying every
//! vote, that segment an existing directory, and no stay or elsewhere
//! vote. Measured on this repository's own files: a repo-wide section that
//! names `src-tauri/`, `src/` and `src-mobile/` spreads its votes; a seam
//! section naming `src-tauri/src/remote/surface.rs` beside
//! `src-mobile/src/surface.rs` casts an elsewhere vote; a rule with one
//! example path is under the floor. All three are suppressed, and the
//! root file scores clean, which the design requires.
//!
//! # Not found is not Unknown
//!
//! A path that does not exist has nothing to place, so it is listed and
//! the section is still judged (`imports.rs` keeps the same distinction).
//! A metadata error while resolving -- a permission wall under a named
//! path -- is [`Severity::Unknown`] for that section with the io error as
//! evidence: never a finding, never silence. A file the scan listed but
//! this producer could not read is Unknown for the whole file.
//!
//! # The two handed rules
//!
//! **Duplicate content across scopes.** A run of three or more identical
//! non-blank lines, whitespace collapsed, between two files a session
//! loads together: the root file and a subdirectory file, the user's
//! `~/.claude/CLAUDE.md` and a project file, `CLAUDE.local.md` and
//! `CLAUDE.md`. Two subdirectory files are not paired: no session loads
//! both at launch. The subject is the narrower file, because every
//! session that loads it also loads the other.
//!
//! **An always-rule in a lazily loaded file.** An all-caps `NEVER`,
//! `ALWAYS`, `IMPORTANT` or `YOU MUST` on a prose line of a subdirectory
//! CLAUDE.md. The finding states the loading fact from Claude Code's
//! docs: such a file loads only after a Read in its directory, not at
//! launch, not on Write, and not for the Explore and Plan agents. Whether
//! the rule needs to hold before that is the reader's call.
//!
//! # A brief is rendered from the finding alone
//!
//! `brief::render` runs at construction with nothing but the [`Finding`],
//! so what the suggestion needs travels in it: the target `CLAUDE.md` and
//! whether it exists is a `Locator::File` evidence entry with no line,
//! and the token figure is read back from the finding sentence this
//! module wrote. A figure that cannot be read back is omitted, never
//! guessed.

use super::{Check, Context, Evidence, Finding, Locator, Producer, Severity, Subject};
use crate::claudemd::imports::parse_imports;
use crate::claudemd::refs::{self, RefKind};
use crate::claudemd::text::{self, Section};
use crate::claudemd::{expand_home_in, tokens, Scope};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

pub struct Placement;

impl Producer for Placement {
    fn check(&self) -> Check {
        Check::Placement
    }

    fn run(&self, cx: &Context) -> Result<Vec<Finding>, String> {
        let mut listed: Vec<(PathBuf, Scope)> = cx
            .scan
            .repo
            .files
            .iter()
            .map(|f| (PathBuf::from(&f.path), Scope::Repo))
            .collect();
        listed.extend(
            cx.scan
                .extra
                .iter()
                .map(|s| (PathBuf::from(&s.file.path), s.scope)),
        );

        let mut out = Vec::new();
        let mut loaded = Vec::new();
        for (path, scope) in listed {
            match std::fs::read_to_string(&path) {
                Ok(text) => loaded.push(Loaded {
                    path,
                    scope,
                    text: text.replace("\r\n", "\n"),
                }),
                // The scan read this file a moment ago. A failure now is
                // still Unknown for the file, never a clean pass.
                Err(e) => out.push(Finding::new(
                    Check::Placement,
                    Severity::Unknown,
                    Subject::ClaudeMd {
                        path: slashed(&path),
                        scope,
                        section: None,
                    },
                    vec![Evidence {
                        at: Locator::File {
                            path: slashed(&path),
                            line: None,
                        },
                        measured: e.to_string(),
                    }],
                    format!("{} could not be read: {e}", slashed(&path)),
                )),
            }
        }

        for f in &loaded {
            out.extend(assess(f, cx.repo, cx.home));
            out.extend(always_rules(f, cx.repo));
        }
        out.extend(duplicates(&loaded, cx.repo));
        Ok(out)
    }
}

/// One CLAUDE.md this producer read, CRLF normalised.
struct Loaded {
    path: PathBuf,
    scope: Scope,
    text: String,
}

/// Where a candidate path resolved to, relative to the file naming it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Resolved {
    /// Under the file's directory, by way of this first segment.
    Under { segment: String, path: PathBuf },
    /// A file at the file's own level.
    Stay(PathBuf),
    /// Outside the file's directory.
    Elsewhere(PathBuf),
    /// Neither the file's directory nor the repository root has it.
    NotFound,
    /// A metadata error other than not-found, with the io error.
    Unreadable(String),
}

/// A path token: the characters a path is spelled with, and nothing a
/// sentence is (`refs::PATH_TOKEN`'s rule, applied to bare prose).
static BARE_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_./-]+$").unwrap());
/// A trailing `:line`, stripped from any candidate.
static TRAILING_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*):\d+$").unwrap());
/// The all-caps rule words the shape research names.
static RULE_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(NEVER|ALWAYS|IMPORTANT|YOU MUST)\b").unwrap());
/// An inline code span, removed before a prose line is read for words.
static CODE_SPAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`]*`").unwrap());

/// Marks in the finding sentences this module writes, read back by
/// [`suggestion`] to tell the three rules apart.
const DUPLICATE_MARK: &str = " carry the same ";
const LAZY_MARK: &str = "; this file loads only after a Read in ";

/// The path candidates a section names, deduplicated, `:line` stripped.
///
/// Three sources: spans classified as paths by `refs::extract` (a span
/// must look like a path; `make lint` is not one), `@imports` through
/// `parse_imports`, and bare prose tokens containing a `/`.
fn path_candidates(section: &Section) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |c: String| {
        if !c.is_empty() && !out.contains(&c) {
            out.push(c);
        }
    };
    for r in refs::extract(&section.text) {
        match r.kind {
            RefKind::Path { path } | RefKind::PathLine { path, .. } => push(path),
            _ => {}
        }
    }
    for import in parse_imports(&section.text) {
        push(import);
    }
    for (_, line) in text::prose_lines(&section.text) {
        for token in line.split_whitespace() {
            if let Some(c) = bare_path(token) {
                push(c);
            }
        }
    }
    out
}

/// A bare prose token that is a path: contains a `/`, is spelled like
/// one, and is not a span, an `@import`, a URL or `/` alone.
fn bare_path(token: &str) -> Option<String> {
    let t = token
        .trim_start_matches(['(', '[', '{', '<', '"', '\'', '*', '_'])
        .trim_end_matches([
            ')', ']', '}', '>', '"', '\'', '*', '_', ',', '.', ';', ':', '!', '?',
        ]);
    if t.contains('`') || t.starts_with('@') || t.contains("//") || !t.contains('/') {
        return None;
    }
    if !BARE_PATH.is_match(t) {
        return None;
    }
    let t = strip_line(t);
    if t.trim_matches('/').is_empty() || t == "." || t == ".." {
        return None;
    }
    Some(t)
}

fn strip_line(candidate: &str) -> String {
    TRAILING_LINE
        .captures(candidate)
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| candidate.to_string())
}

/// Resolve one candidate against the file's directory, then the
/// repository root when the file is inside the repository. Never by
/// suffix search.
fn resolve(
    candidate: &str,
    file_dir: &Path,
    repo_root: Option<&Path>,
    home: Option<&Path>,
) -> Resolved {
    let targets: Vec<PathBuf> = if candidate.starts_with("~/") {
        match home.and_then(|h| expand_home_in(candidate, h)) {
            Some(p) => vec![p],
            None => return Resolved::NotFound,
        }
    } else if Path::new(candidate).is_absolute() {
        vec![PathBuf::from(candidate)]
    } else {
        let mut t = vec![file_dir.join(candidate)];
        if let Some(root) = repo_root {
            if root != file_dir {
                t.push(root.join(candidate));
            }
        }
        t
    };

    for target in targets {
        match std::fs::metadata(&target) {
            Ok(m) if m.is_file() || m.is_dir() => {
                return classify(&normalise(&target), &normalise(file_dir), m.is_dir());
            }
            Ok(_) => continue,
            Err(e) if e.kind() == ErrorKind::NotFound || e.kind() == ErrorKind::NotADirectory => {
                continue
            }
            Err(e) => return Resolved::Unreadable(format!("{}: {e}", slashed(&target))),
        }
    }
    Resolved::NotFound
}

/// Which way a resolved path votes, given where the file lives.
fn classify(target: &Path, file_dir: &Path, is_dir: bool) -> Resolved {
    let Ok(rel) = target.strip_prefix(file_dir) else {
        return Resolved::Elsewhere(target.to_path_buf());
    };
    let mut comps = rel.components();
    match (comps.next(), comps.next()) {
        (None, _) => Resolved::Stay(target.to_path_buf()),
        // A FILE at the file's own level is stay; a DIRECTORY at that
        // level is the segment itself, named.
        (Some(_), None) if !is_dir => Resolved::Stay(target.to_path_buf()),
        (Some(first), _) => Resolved::Under {
            segment: first.as_os_str().to_string_lossy().to_string(),
            path: target.to_path_buf(),
        },
    }
}

/// Lexical normalisation: `.` dropped, `..` popped. No `canonicalize`,
/// which on Windows returns verbatim `\\?\C:\` paths that no longer
/// share a prefix with the scan's own.
fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// A path with `/` between components, whatever the platform, so a
/// finding reads the same on every machine that renders it.
fn slashed(p: &Path) -> String {
    let mut out = String::new();
    for c in p.components() {
        match c {
            Component::RootDir => out.push('/'),
            Component::Prefix(pre) => out.push_str(&pre.as_os_str().to_string_lossy()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str("..");
            }
            Component::Normal(n) => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(&n.to_string_lossy());
            }
        }
    }
    out
}

/// A path shown relative to the repository when it is inside it.
fn relative_to(p: &Path, repo: &Path) -> String {
    match p.strip_prefix(repo) {
        Ok(rel) if !rel.as_os_str().is_empty() => slashed(rel),
        _ => slashed(p),
    }
}

/// One section's votes.
#[derive(Debug, Default)]
struct Votes {
    /// Segment -> the candidates, as written, that resolved under it. One
    /// entry per DISTINCT resolved path.
    under: BTreeMap<String, Vec<String>>,
    stay: Vec<String>,
    elsewhere: Vec<String>,
    not_found: Vec<String>,
    unreadable: Vec<String>,
    /// How many distinct paths resolved, across every vote.
    distinct: usize,
}

fn assess_section(
    section: &Section,
    file_dir: &Path,
    repo_root: Option<&Path>,
    home: Option<&Path>,
) -> Votes {
    let mut votes = Votes::default();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for candidate in path_candidates(section) {
        match resolve(&candidate, file_dir, repo_root, home) {
            Resolved::Under { segment, path } => {
                if seen.insert(path) {
                    votes.under.entry(segment).or_default().push(candidate);
                }
            }
            Resolved::Stay(path) => {
                if seen.insert(path) {
                    votes.stay.push(candidate);
                }
            }
            Resolved::Elsewhere(path) => {
                if seen.insert(path) {
                    votes.elsewhere.push(candidate);
                }
            }
            Resolved::NotFound => votes.not_found.push(candidate),
            Resolved::Unreadable(why) => votes.unreadable.push(why),
        }
    }
    votes.distinct = seen.len();
    votes
}

/// The heading as written, for the subject; `None` for the preamble.
fn heading_as_written(s: &Section) -> Option<String> {
    s.heading
        .as_ref()
        .map(|h| format!("{} {h}", "#".repeat(usize::from(s.level))))
}

fn heading_text(s: &Section) -> &str {
    s.heading.as_deref().unwrap_or("(before the first heading)")
}

/// The text that would move: the heading line and everything under it.
fn section_source(s: &Section) -> String {
    match heading_as_written(s) {
        Some(h) => format!("{h}\n{}", s.text),
        None => s.text.clone(),
    }
}

fn line_of(n: usize) -> Option<u32> {
    u32::try_from(n).ok()
}

/// The placement findings for one file, one per section at most.
fn assess(file: &Loaded, repo: &Path, home: Option<&Path>) -> Vec<Finding> {
    let Some(file_dir) = file.path.parent() else {
        return Vec::new();
    };
    // The root fallback is for files inside the repository. A global
    // file naming `src/x.ts` is talking about every repository, and
    // resolving it against the one on screen would place it there.
    let repo_root = file_dir.starts_with(repo).then_some(repo);
    let path = slashed(&file.path);
    let mut out = Vec::new();

    for section in text::sections(&file.text) {
        let votes = assess_section(&section, file_dir, repo_root, home);
        let at = Locator::File {
            path: path.clone(),
            line: line_of(section.line),
        };
        let subject = Subject::ClaudeMd {
            path: path.clone(),
            scope: file.scope,
            section: heading_as_written(&section),
        };

        if !votes.unreadable.is_empty() {
            out.push(Finding::new(
                Check::Placement,
                Severity::Unknown,
                subject,
                votes
                    .unreadable
                    .iter()
                    .map(|why| Evidence {
                        at: at.clone(),
                        measured: why.clone(),
                    })
                    .collect(),
                format!(
                    "Section \"{}\" could not be checked: {}",
                    heading_text(&section),
                    votes.unreadable[0]
                ),
            ));
            continue;
        }

        if votes.distinct < 2 || votes.under.len() != 1 {
            continue;
        }
        if !votes.stay.is_empty() || !votes.elsewhere.is_empty() {
            continue;
        }
        let (segment, named) = votes.under.iter().next().expect("one segment");
        let dir = file_dir.join(segment);
        if !dir.is_dir() {
            continue;
        }
        let d = relative_to(&dir, repo);
        let k = named.len();
        let list = named.join(", ");
        let est = tokens::estimate(&section_source(&section));
        let target = dir.join("CLAUDE.md");

        let mut evidence = vec![Evidence {
            at: at.clone(),
            measured: format!("{k} of {k} paths resolve under {d}/"),
        }];
        if !votes.not_found.is_empty() {
            let m = votes.not_found.len();
            evidence.push(Evidence {
                at: at.clone(),
                measured: format!(
                    "{m} of {} named paths do not resolve from this file or the repository root: {}",
                    k + m,
                    votes.not_found.join(", ")
                ),
            });
        }
        evidence.push(Evidence {
            at: Locator::File {
                path: slashed(&target),
                line: None,
            },
            measured: if target.is_file() {
                "exists".to_string()
            } else {
                "does not exist".to_string()
            },
        });

        out.push(Finding::new(
            Check::Placement,
            Severity::Advice,
            subject,
            evidence,
            format!(
                "Section \"{}\" (~{est} est. tokens) names only paths under {d}/: {list}",
                heading_text(&section)
            ),
        ));
    }
    out
}

/// The section a line falls in, for a subject.
fn section_at(sections: &[Section], line: usize) -> Option<String> {
    sections
        .iter()
        .rev()
        .find(|s| s.line <= line)
        .and_then(heading_as_written)
}

/// An all-caps rule word on a prose line of a subdirectory CLAUDE.md.
fn always_rules(file: &Loaded, repo: &Path) -> Vec<Finding> {
    if file.scope != Scope::Repo {
        return Vec::new();
    }
    let Some(file_dir) = file.path.parent() else {
        return Vec::new();
    };
    let dir = match file_dir.strip_prefix(repo) {
        Ok(rel) if !rel.as_os_str().is_empty() => slashed(rel),
        // The root file loads at launch; this rule is about the ones
        // that do not.
        _ => return Vec::new(),
    };
    let path = slashed(&file.path);
    let sections = text::sections(&file.text);
    let root = repo.join("CLAUDE.md");
    let mut out = Vec::new();

    for (n, line) in text::prose_lines(&file.text) {
        let prose = CODE_SPAN.replace_all(line, "");
        let Some(m) = RULE_WORD.find(&prose) else {
            continue;
        };
        let word = m.as_str();
        out.push(Finding::new(
            Check::Placement,
            Severity::Advice,
            Subject::ClaudeMd {
                path: path.clone(),
                scope: file.scope,
                section: section_at(&sections, n),
            },
            vec![
                Evidence {
                    at: Locator::File {
                        path: path.clone(),
                        line: line_of(n),
                    },
                    measured: format!("`{word}` in capitals on a prose line outside a fence"),
                },
                Evidence {
                    at: Locator::File {
                        path: slashed(&root),
                        line: None,
                    },
                    measured: if root.is_file() {
                        "exists, loaded at launch".to_string()
                    } else {
                        "does not exist".to_string()
                    },
                },
            ],
            format!(
                "{path}:{n} says {word}{LAZY_MARK}{dir}/, not at launch, not on Write, \
                 and not for the Explore and Plan agents"
            ),
        ));
    }
    out
}

/// How narrow a file's audience is. The narrower file of a pair is the
/// subject of a duplicate finding: every session that loads it also
/// loads the broader one.
fn rank(file: &Loaded, repo: &Path) -> u8 {
    match file.scope {
        Scope::Global => 0,
        Scope::Local => 2,
        Scope::Repo => {
            if file.path.parent().is_some_and(|d| d == repo) {
                1
            } else {
                3
            }
        }
    }
}

/// Non-blank lines with whitespace collapsed, each with its 1-based line.
fn normalised_lines(text: &str) -> Vec<(usize, String)> {
    text.split('\n')
        .enumerate()
        .filter_map(|(i, l)| {
            let joined = l.split_whitespace().collect::<Vec<_>>().join(" ");
            (!joined.is_empty()).then_some((i + 1, joined))
        })
        .collect()
}

/// A run of identical normalised lines: `(first, last)` in each file and
/// its length.
struct Run {
    a: (usize, usize),
    b: (usize, usize),
    len: usize,
}

/// Every maximal common run of at least `min` lines.
fn common_runs(a: &[(usize, String)], b: &[(usize, String)], min: usize) -> Vec<Run> {
    let mut out = Vec::new();
    for i in 0..a.len() {
        for j in 0..b.len() {
            if a[i].1 != b[j].1 {
                continue;
            }
            // Only a run's START is counted, so one run is one finding.
            if i > 0 && j > 0 && a[i - 1].1 == b[j - 1].1 {
                continue;
            }
            let mut k = 1;
            while i + k < a.len() && j + k < b.len() && a[i + k].1 == b[j + k].1 {
                k += 1;
            }
            if k >= min {
                out.push(Run {
                    a: (a[i].0, a[i + k - 1].0),
                    b: (b[j].0, b[j + k - 1].0),
                    len: k,
                });
            }
        }
    }
    out
}

/// The duplicate-content findings across every pair one session loads.
fn duplicates(files: &[Loaded], repo: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    for (i, x) in files.iter().enumerate() {
        for y in &files[i + 1..] {
            let (rx, ry) = (rank(x, repo), rank(y, repo));
            // Two subdirectory files are never in one launch set.
            if rx == 3 && ry == 3 {
                continue;
            }
            let (narrow, broad) = if rx >= ry { (x, y) } else { (y, x) };
            let na = normalised_lines(&narrow.text);
            let nb = normalised_lines(&broad.text);
            let sections = text::sections(&narrow.text);
            for run in common_runs(&na, &nb, 3) {
                let narrow_path = slashed(&narrow.path);
                let broad_path = slashed(&broad.path);
                out.push(Finding::new(
                    Check::Placement,
                    Severity::Advice,
                    Subject::ClaudeMd {
                        path: narrow_path.clone(),
                        scope: narrow.scope,
                        section: section_at(&sections, run.a.0),
                    },
                    vec![
                        Evidence {
                            at: Locator::File {
                                path: narrow_path.clone(),
                                line: line_of(run.a.0),
                            },
                            measured: format!(
                                "lines {}-{}: {} non-blank lines, identical after whitespace is collapsed",
                                run.a.0, run.a.1, run.len
                            ),
                        },
                        Evidence {
                            at: Locator::File {
                                path: broad_path.clone(),
                                line: line_of(run.b.0),
                            },
                            measured: format!(
                                "lines {}-{}: the same {} lines",
                                run.b.0, run.b.1, run.len
                            ),
                        },
                    ],
                    format!(
                        "{narrow_path}:{}-{} and {broad_path}:{}-{}{DUPLICATE_MARK}{} lines",
                        run.a.0, run.a.1, run.b.0, run.b.1, run.len
                    ),
                ));
            }
        }
    }
    out
}

/// The `~N` in a sentence this module wrote, when it can be read back.
fn est_tokens(sentence: &str) -> Option<&str> {
    let (_, rest) = sentence.split_once("(~")?;
    let (n, _) = rest.split_once(" est. tokens)")?;
    n.parse::<u64>().ok().map(|_| n)
}

/// The other file a finding's evidence names with no line: the target
/// `CLAUDE.md` and what was measured about it.
fn other_file(f: &Finding) -> Option<(&str, &str)> {
    f.evidence.iter().find_map(|e| match &e.at {
        Locator::File { path, line: None } if path != f.subject.path() => {
            Some((path.as_str(), e.measured.as_str()))
        }
        _ => None,
    })
}

/// The brief's "Suggested change" for a placement finding.
///
/// Called from `brief.rs`'s match on [`Check`], so the wording lives
/// beside the rules it describes.
pub(crate) fn suggestion(f: &Finding) -> String {
    let subject = f.subject.path();
    if f.severity == Severity::Unknown {
        return format!(
            "Nothing yet: the path named in the evidence could not be read, so this section \
             of `{subject}` was not judged. Make it readable and run the check again."
        );
    }
    if f.finding.contains(LAZY_MARK) {
        return match other_file(f) {
            Some((root, measured)) if measured.starts_with("exists") => format!(
                "If this rule must hold before a session reads anything in this directory, \
                 move the line to `{root}`, which loads at launch, or enforce it with a hook. \
                 If it matters only once a session is reading files here, leave it in `{subject}`."
            ),
            _ => format!(
                "If this rule must hold before a session reads anything in this directory, \
                 move the line to a root `CLAUDE.md`, which loads at launch, or enforce it with \
                 a hook. If it matters only once a session is reading files here, leave it in \
                 `{subject}`."
            ),
        };
    }
    if f.finding.contains(DUPLICATE_MARK) {
        let broad = f
            .evidence
            .get(1)
            .map(|e| super::brief::locator(&e.at))
            .unwrap_or_default();
        return format!(
            "Delete the run the first evidence line marks from `{subject}`; the same lines \
             are at {broad}, which every session that loads `{subject}` also loads. Keep the \
             copy there."
        );
    }
    let saving = match est_tokens(&f.finding) {
        Some(n) => format!(
            "The section's ~{n} est. tokens then load only when a session reads files under \
             that directory, instead of every time `{subject}` loads."
        ),
        None => format!(
            "The section then loads only when a session reads files under that directory, \
             instead of every time `{subject}` loads."
        ),
    };
    match other_file(f) {
        Some((target, measured)) if measured.starts_with("does not exist") => format!(
            "Moving this section would need `{target}`, which does not exist. Whether to \
             create one is the missing-subdirectory-CLAUDE.md check's call, not this one's; \
             until it exists, leave the section in `{subject}`."
        ),
        Some((target, _)) => {
            format!(
                "Cut the section from `{subject}` and add it to `{target}`, which exists. {saving}"
            )
        }
        None => format!(
            "Cut the section from `{subject}` and add it to the `CLAUDE.md` of the directory \
             the finding names. {saving}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claudemd::advice::{report_in, CheckRun, Report};
    use std::fs;

    fn placement(report: &Report) -> Vec<&Finding> {
        report
            .findings
            .iter()
            .filter(|f| f.check == Check::Placement)
            .collect()
    }

    fn ran(report: &Report) {
        let c = report
            .checks
            .iter()
            .find(|c| c.check == Check::Placement)
            .unwrap();
        assert!(matches!(c.run, CheckRun::Ran { .. }), "{report:?}");
    }

    /// A repository with `octo/a.rs`, `octo/b.rs`, and the root
    /// `CLAUDE.md` given.
    fn octo(claude_md: &str) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("octo")).unwrap();
        fs::write(t.path().join("octo").join("a.rs"), "").unwrap();
        fs::write(t.path().join("octo").join("b.rs"), "").unwrap();
        fs::write(t.path().join("CLAUDE.md"), claude_md).unwrap();
        t
    }

    const OCTO_SECTION: &str =
        "# Root\n\nrepo-wide.\n\n## Octo rules\n\nEdit `octo/a.rs` and `octo/b.rs:12` together.\n";

    /// (a) The founding case: two paths under one directory that has a
    /// CLAUDE.md. The sentence is exact, the token figure is the
    /// estimator's over the section, and the brief names the target.
    #[test]
    fn a_section_naming_only_one_subdirectory_is_a_finding() {
        let t = octo(OCTO_SECTION);
        fs::write(t.path().join("octo").join("CLAUDE.md"), "# octo\n").unwrap();

        let report = report_in(t.path(), None);
        ran(&report);
        let found = placement(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        let f = found[0];
        assert_eq!(f.severity, Severity::Advice);
        let est =
            tokens::estimate("## Octo rules\n\nEdit `octo/a.rs` and `octo/b.rs:12` together.");
        assert_eq!(
            f.finding,
            format!(
                "Section \"Octo rules\" (~{est} est. tokens) names only paths under octo/: octo/a.rs, octo/b.rs"
            )
        );
        assert_eq!(
            f.subject,
            Subject::ClaudeMd {
                path: slashed(&t.path().join("CLAUDE.md")),
                scope: Scope::Repo,
                section: Some("## Octo rules".into()),
            }
        );
        assert_eq!(
            f.evidence[0].at,
            Locator::File {
                path: slashed(&t.path().join("CLAUDE.md")),
                line: Some(5),
            }
        );
        assert_eq!(f.evidence[0].measured, "2 of 2 paths resolve under octo/");
        let target = slashed(&t.path().join("octo").join("CLAUDE.md"));
        assert!(
            f.brief
                .contains(&format!("add it to `{target}`, which exists")),
            "{}",
            f.brief
        );
        assert!(
            f.brief.contains(&format!("~{est} est. tokens")),
            "the saving is stated as an estimate: {}",
            f.brief
        );
        assert!(!f.brief.contains("does not exist"), "{}", f.brief);
        assert!(!f.finding.contains("belongs in"));
    }

    /// (b) The same section with no `octo/CLAUDE.md`: the fact is the
    /// same, and the suggestion is qualified rather than an instruction
    /// to create a file, which the gaps check owns.
    #[test]
    fn a_missing_target_qualifies_the_suggestion() {
        let t = octo(OCTO_SECTION);

        let report = report_in(t.path(), None);
        let found = placement(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        let f = found[0];
        assert!(f.finding.contains("names only paths under octo/"));
        let target = slashed(&t.path().join("octo").join("CLAUDE.md"));
        assert!(
            f.brief
                .contains(&format!("would need `{target}`, which does not exist")),
            "{}",
            f.brief
        );
        assert!(
            f.brief
                .contains("missing-subdirectory-CLAUDE.md check's call"),
            "{}",
            f.brief
        );
        assert!(!f.brief.contains("Cut the section"), "{}", f.brief);
    }

    /// (c) A path at the file's own level is a stay vote, and one stay
    /// vote suppresses the finding.
    #[test]
    fn a_stay_vote_suppresses_the_finding() {
        let t =
            octo("## Octo rules\n\nEdit `octo/a.rs` and `octo/b.rs`; run `Makefile` targets.\n");
        fs::write(t.path().join("Makefile"), "lint:\n").unwrap();

        let report = report_in(t.path(), None);
        ran(&report);
        assert!(placement(&report).is_empty(), "{report:?}");

        // The negative can fail: remove the stay vote and it fires.
        fs::remove_file(t.path().join("Makefile")).unwrap();
        let report = report_in(t.path(), None);
        assert_eq!(placement(&report).len(), 1, "{report:?}");
    }

    /// (d) One path is not a concentration.
    #[test]
    fn a_single_path_is_under_the_floor() {
        let t = octo("## Octo rules\n\nEdit `octo/a.rs` carefully.\n");
        let report = report_in(t.path(), None);
        ran(&report);
        assert!(placement(&report).is_empty(), "{report:?}");

        let t = octo("## Octo rules\n\nEdit `octo/a.rs` and `octo/a.rs` and `./octo/a.rs`.\n");
        let report = report_in(t.path(), None);
        assert!(
            placement(&report).is_empty(),
            "one file named three ways is one path: {report:?}"
        );
    }

    /// (e) A `#` inside a bash fence is a comment, not a heading, so the
    /// section is still one section and the paths in its prose still
    /// count together.
    #[test]
    fn a_hash_inside_a_fence_does_not_split_the_section() {
        let t = octo(
            "## Octo rules\n\nEdit `octo/a.rs`.\n\n```bash\n# not a heading\nls octo/\n```\n\nAnd `octo/b.rs`.\n",
        );
        let report = report_in(t.path(), None);
        let found = placement(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        assert!(
            found[0].finding.starts_with("Section \"Octo rules\""),
            "{}",
            found[0].finding
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.finding.contains("not a heading")),
            "{report:?}"
        );
    }

    /// (f) A permission wall under a named path is Unknown for that
    /// section: not a finding, not silence. Unix only: `chmod 000` is
    /// the mechanism and Windows does not honour it. As root the wall
    /// is not a wall; the gate runs under `capsh` for that reason.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_path_is_unknown_for_the_section() {
        use std::os::unix::fs::PermissionsExt;

        let t = octo("## Octo rules\n\nEdit `octo/a.rs`, `octo/b.rs` and `walled/secrets/c.rs`.\n");
        let blocked = t.path().join("walled").join("secrets");
        fs::create_dir_all(&blocked).unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();

        let report = report_in(t.path(), None);

        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();

        ran(&report);
        let found = placement(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        let f = found[0];
        assert_eq!(f.severity, Severity::Unknown, "{f:?}");
        assert!(
            f.finding
                .starts_with("Section \"Octo rules\" could not be checked: "),
            "{}",
            f.finding
        );
        assert!(
            f.evidence[0].measured.contains("walled/secrets/c.rs"),
            "{:?}",
            f.evidence
        );
        assert!(
            f.evidence[0].measured.to_lowercase().contains("permission"),
            "the io error travels as the reason: {:?}",
            f.evidence
        );
        assert!(f.brief.contains("Nothing yet"), "{}", f.brief);
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.severity == Severity::Advice),
            "an unjudged section is never advice: {report:?}"
        );
    }

    /// (g) A path that does not exist is listed as not found and does
    /// not vote: it neither makes a finding nor blocks one.
    #[test]
    fn a_missing_path_is_listed_and_does_not_vote() {
        let t = octo("## Octo rules\n\nEdit `octo/a.rs` and `claude/cli.rs:128`.\n");
        let report = report_in(t.path(), None);
        ran(&report);
        assert!(
            placement(&report).is_empty(),
            "one resolved path plus one not found is under the floor: {report:?}"
        );

        // And, resolved directly, the section lists it.
        let sections =
            text::sections("## Octo rules\n\nEdit `octo/a.rs` and `claude/cli.rs:128`.\n");
        let votes = assess_section(&sections[0], t.path(), Some(t.path()), None);
        assert_eq!(votes.not_found, vec!["claude/cli.rs"]);
        assert_eq!(votes.distinct, 1);
        assert!(votes.unreadable.is_empty());

        // With a second resolved path the finding fires and the
        // not-found list travels as evidence.
        let t = octo("## Octo rules\n\nEdit `octo/a.rs`, `octo/b.rs` and `claude/cli.rs:128`.\n");
        let report = report_in(t.path(), None);
        let found = placement(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        assert!(
            found[0].evidence.iter().any(|e| e.measured
                == "1 of 3 named paths do not resolve from this file or the repository root: claude/cli.rs"),
            "{:?}",
            found[0].evidence
        );
    }

    /// A seam between two directories casts an elsewhere vote. The
    /// `src-tauri/CLAUDE.md` surface-tables section is the shape.
    #[test]
    fn an_elsewhere_vote_suppresses_the_finding() {
        let t = tempfile::tempdir().unwrap();
        for p in ["src-tauri/src/remote", "src-mobile/src"] {
            fs::create_dir_all(t.path().join(p)).unwrap();
        }
        fs::write(t.path().join("src-tauri/src/remote/surface.rs"), "").unwrap();
        fs::write(t.path().join("src-tauri/src/invariants.rs"), "").unwrap();
        fs::write(t.path().join("src-mobile/src/surface.rs"), "").unwrap();
        fs::write(
            t.path().join("src-tauri").join("CLAUDE.md"),
            "## Both tables\n\n`src-tauri/src/remote/surface.rs` **and** `src-mobile/src/surface.rs`, plus `src-tauri/src/invariants.rs`.\n",
        )
        .unwrap();

        let report = report_in(t.path(), None);
        assert!(placement(&report).is_empty(), "{report:?}");

        // Remove the seam and the two remaining paths are one segment.
        fs::write(
            t.path().join("src-tauri").join("CLAUDE.md"),
            "## Both tables\n\n`src-tauri/src/remote/surface.rs` and `src-tauri/src/invariants.rs`.\n",
        )
        .unwrap();
        let report = report_in(t.path(), None);
        let found = placement(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        assert!(
            found[0]
                .finding
                .contains("names only paths under src-tauri/src/:"),
            "{}",
            found[0].finding
        );
    }

    /// This repository's own root file, with the paths it names stubbed,
    /// scores clean: its intro spreads votes over three directories plus
    /// a stay vote for `CLAUDE.md`, and its rules section names two paths
    /// that do not resolve from the root. The design says the check must
    /// score it clean or it is measuring the wrong thing.
    #[test]
    fn this_repositorys_root_file_scores_clean() {
        let root = include_str!("../../../../CLAUDE.md");
        let t = tempfile::tempdir().unwrap();
        for d in ["src-tauri/src", "src", "src-mobile"] {
            fs::create_dir_all(t.path().join(d)).unwrap();
        }
        fs::write(t.path().join("src-tauri/src/invariants.rs"), "").unwrap();
        fs::write(t.path().join("CLAUDE.md"), root).unwrap();

        let report = report_in(t.path(), None);
        ran(&report);
        assert!(placement(&report).is_empty(), "{report:?}");

        // The suppression is real: a suffix search would have found
        // `claude/cli.rs`. Prove the two rules paths are not found and
        // the intro's votes are spread, so a future edit that changes
        // either is visible here.
        let sections = text::sections(root);
        let rules = sections
            .iter()
            .find(|s| s.heading.as_deref() == Some("Rules that have shipped as defects"))
            .expect("the rules section");
        let votes = assess_section(rules, t.path(), Some(t.path()), None);
        assert_eq!(votes.not_found, vec!["claude/cli.rs", "claude/sessions.rs"]);
        assert!(votes.under.is_empty(), "{votes:?}");
        let intro = &sections[0];
        let votes = assess_section(intro, t.path(), Some(t.path()), None);
        assert!(votes.under.len() >= 3, "{votes:?}");
        assert_eq!(votes.stay, vec!["CLAUDE.md"]);
    }

    /// The one hit the design predicts on this repository: the
    /// `useIsMobile()` section of `src/CLAUDE.md`, qualified because
    /// `src/lib/CLAUDE.md` does not exist. The token figure is measured
    /// here, not copied from the design's hand count.
    #[test]
    fn the_use_is_mobile_section_is_the_predicted_qualified_hit() {
        const SECTION: &str = "\
# src

The React 19 + TypeScript frontend.

## `useIsMobile()` vs `IS_MOBILE_BUILD`

Two different questions, routinely confused:

- **`useIsMobile()`** (`src/lib/useIsMobile.ts`) — a *layout* question. Is the
  viewport narrow? True in a resized desktop window.
- **`IS_MOBILE_BUILD`** (`src/lib/target.ts`) — a *capability* question. Is this
  the iOS build? Decides whether a command exists at all.

`src/lib/target.ts` argues the distinction at length. Read it before reaching
for either; the file exists because getting it wrong is easy.
";
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("src").join("lib")).unwrap();
        fs::write(t.path().join("src/lib/useIsMobile.ts"), "").unwrap();
        fs::write(t.path().join("src/lib/target.ts"), "").unwrap();
        fs::write(t.path().join("src").join("CLAUDE.md"), SECTION).unwrap();

        let report = report_in(t.path(), None);
        let found = placement(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        let f = found[0];
        let section = &text::sections(SECTION)[1];
        let est = tokens::estimate(&section_source(section));
        // Measured: 125 est. tokens over the heading line and its text,
        // by chars ÷ 4, which is also the design's hand count.
        assert_eq!(est, 125);
        assert_eq!(
            f.finding,
            format!(
                "Section \"`useIsMobile()` vs `IS_MOBILE_BUILD`\" (~{est} est. tokens) names only \
                 paths under src/lib/: src/lib/useIsMobile.ts, src/lib/target.ts"
            )
        );
        assert_eq!(
            f.evidence[0].measured,
            "2 of 2 paths resolve under src/lib/"
        );
        assert!(f.brief.contains("which does not exist"), "{}", f.brief);
    }

    /// Handed rule 1: a run of three identical lines between the root
    /// file and a subdirectory file. Both paths and line ranges are in
    /// the evidence; the subject is the narrower file.
    #[test]
    fn three_identical_lines_across_scopes_is_a_finding() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("octo")).unwrap();
        fs::write(
            t.path().join("CLAUDE.md"),
            "# Root\n\n## Verifying\n\nRun `make lint`.\nThen   `make test`.\nReport the counts.\n",
        )
        .unwrap();
        fs::write(
            t.path().join("octo").join("CLAUDE.md"),
            "# octo\n\nintro\n\nRun `make lint`.\nThen `make test`.\nReport the counts.\n",
        )
        .unwrap();

        let report = report_in(t.path(), None);
        let found = placement(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        let f = found[0];
        assert_eq!(f.severity, Severity::Advice);
        let sub = slashed(&t.path().join("octo").join("CLAUDE.md"));
        let root = slashed(&t.path().join("CLAUDE.md"));
        assert_eq!(f.subject.path(), sub);
        assert_eq!(
            f.finding,
            format!("{sub}:5-7 and {root}:5-7 carry the same 3 lines")
        );
        assert_eq!(
            f.evidence[0].at,
            Locator::File {
                path: sub.clone(),
                line: Some(5)
            }
        );
        assert_eq!(
            f.evidence[1].at,
            Locator::File {
                path: root.clone(),
                line: Some(5)
            }
        );
        assert!(f.evidence[0].measured.contains("lines 5-7"));
        assert!(f.brief.contains(&format!("`{root}:5`")), "{}", f.brief);
    }

    /// The negative for rule 1: two shared lines are under the floor,
    /// and two SUBDIRECTORY files are never paired.
    #[test]
    fn two_shared_lines_or_two_subdirectory_files_are_not_a_finding() {
        let t = tempfile::tempdir().unwrap();
        fs::write(
            t.path().join("CLAUDE.md"),
            "# Root\n\nRun `make lint`.\nReport the counts.\n",
        )
        .unwrap();
        for d in ["octo", "cat"] {
            fs::create_dir_all(t.path().join(d)).unwrap();
            fs::write(
                t.path().join(d).join("CLAUDE.md"),
                "shared one\nshared two\nshared three\nRun `make lint`.\nReport the counts.\n",
            )
            .unwrap();
        }
        let report = report_in(t.path(), None);
        assert!(placement(&report).is_empty(), "{report:?}");
    }

    /// Rule 1 across the user's global file and the project, through the
    /// injected home.
    #[test]
    fn the_global_file_is_paired_with_the_project() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let repo = t.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let block = "- one rule\n- two rule\n- three rule\n";
        fs::write(home.join(".claude").join("CLAUDE.md"), block).unwrap();
        fs::write(repo.join("CLAUDE.md"), format!("# Root\n\n{block}")).unwrap();

        let report = report_in(&repo, Some(&home));
        let found = placement(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        assert_eq!(found[0].subject.path(), slashed(&repo.join("CLAUDE.md")));
        assert!(found[0].finding.contains("carry the same 3 lines"));
    }

    /// Handed rule 2: an all-caps rule word in a subdirectory file, with
    /// the loading fact stated verbatim.
    #[test]
    fn an_all_caps_rule_in_a_subdirectory_file_is_a_finding() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("octo")).unwrap();
        fs::write(t.path().join("CLAUDE.md"), "# Root\n").unwrap();
        fs::write(
            t.path().join("octo").join("CLAUDE.md"),
            "# octo\n\n## Rules\n\n**NEVER** commit generated files.\n",
        )
        .unwrap();

        let report = report_in(t.path(), None);
        let found = placement(&report);
        assert_eq!(found.len(), 1, "{report:?}");
        let f = found[0];
        let path = slashed(&t.path().join("octo").join("CLAUDE.md"));
        assert_eq!(
            f.finding,
            format!(
                "{path}:5 says NEVER; this file loads only after a Read in octo/, not at launch, \
                 not on Write, and not for the Explore and Plan agents"
            )
        );
        assert_eq!(f.severity, Severity::Advice);
        assert_eq!(
            f.subject,
            Subject::ClaudeMd {
                path: path.clone(),
                scope: Scope::Repo,
                section: Some("## Rules".into()),
            }
        );
        assert_eq!(
            f.evidence[0].at,
            Locator::File {
                path,
                line: Some(5)
            }
        );
        let root = slashed(&t.path().join("CLAUDE.md"));
        assert!(
            f.brief.contains(&format!("move the line to `{root}`")),
            "{}",
            f.brief
        );
    }

    /// The negatives for rule 2: the root file loads at launch; a
    /// lowercase word is prose; a word inside a fence or a code span is
    /// syntax.
    #[test]
    fn the_root_file_lowercase_words_and_fenced_words_are_not_findings() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("octo")).unwrap();
        fs::write(
            t.path().join("CLAUDE.md"),
            "# Root\n\nNEVER commit generated files. YOU MUST run the gate.\n",
        )
        .unwrap();
        fs::write(
            t.path().join("octo").join("CLAUDE.md"),
            "# octo\n\nnever commit generated files; it is important.\n\n```rust\nconst ALWAYS: bool = true; // IMPORTANT\n```\n\nThe `ALWAYS` flag and UNIMPORTANT details.\n",
        )
        .unwrap();

        let report = report_in(t.path(), None);
        assert!(placement(&report).is_empty(), "{report:?}");

        // The negative can fail: one capitalised word in prose fires.
        fs::write(
            t.path().join("octo").join("CLAUDE.md"),
            "# octo\n\nIMPORTANT: run the gate.\n",
        )
        .unwrap();
        let report = report_in(t.path(), None);
        assert_eq!(placement(&report).len(), 1, "{report:?}");
        assert!(placement(&report)[0].finding.contains("says IMPORTANT;"));
    }

    /// The resolver's votes, directly: a directory at the file's level is
    /// the segment, a file at that level is stay, `..` is elsewhere, and
    /// an absolute path is placed by where it is.
    #[test]
    fn resolve_votes_by_position_relative_to_the_file() {
        let t = octo("");
        fs::write(t.path().join("Makefile"), "").unwrap();
        let sub = t.path().join("octo");
        fs::create_dir_all(sub.join("deep")).unwrap();
        fs::write(sub.join("deep").join("x.rs"), "").unwrap();

        let root = t.path();
        assert_eq!(
            resolve("octo/", root, Some(root), None),
            Resolved::Under {
                segment: "octo".into(),
                path: normalise(&sub)
            }
        );
        assert_eq!(
            resolve("Makefile", root, Some(root), None),
            Resolved::Stay(normalise(&root.join("Makefile")))
        );
        // From octo/CLAUDE.md: its own files stay, deeper ones vote, the
        // root's Makefile is elsewhere, and a root-relative spelling of
        // its own file resolves through the root fallback.
        assert!(matches!(
            resolve("a.rs", &sub, Some(root), None),
            Resolved::Stay(_)
        ));
        assert!(matches!(
            resolve("deep/x.rs", &sub, Some(root), None),
            Resolved::Under { segment, .. } if segment == "deep"
        ));
        assert!(matches!(
            resolve("Makefile", &sub, Some(root), None),
            Resolved::Elsewhere(_)
        ));
        assert!(matches!(
            resolve("octo/a.rs", &sub, Some(root), None),
            Resolved::Stay(_)
        ));
        assert!(matches!(
            resolve("../Makefile", &sub, Some(root), None),
            Resolved::Elsewhere(_)
        ));
        assert_eq!(
            resolve("nope/x.rs", root, Some(root), None),
            Resolved::NotFound
        );
        assert_eq!(
            resolve("~/x.md", root, Some(root), None),
            Resolved::NotFound,
            "no home, nothing to expand against"
        );
    }

    /// Candidates: spans that look like paths, `@imports`, bare `/`
    /// tokens; `:line` stripped; commands and symbols are not paths.
    #[test]
    fn path_candidates_come_from_spans_imports_and_bare_tokens() {
        let s = &text::sections(
            "## X\n\nSee `octo/a.rs:12`, `make lint`, `Budget::record`, (bare/token.rs). \
             https://example.invalid/x and and/or.\n@./shared.md\n```\nfenced/path.rs\n```\n",
        )[0];
        assert_eq!(
            path_candidates(s),
            vec!["octo/a.rs", "./shared.md", "bare/token.rs", "and/or"]
        );
    }

    #[test]
    fn est_tokens_reads_back_only_its_own_shape() {
        assert_eq!(
            est_tokens("Section \"X\" (~127 est. tokens) names"),
            Some("127")
        );
        assert_eq!(est_tokens("Section \"X\" (~lots est. tokens) names"), None);
        assert_eq!(est_tokens("no figure"), None);
    }
}
