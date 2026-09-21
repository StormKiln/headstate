//! Toolchain coverage: which build systems are on disk, and which of
//! build, test, lint, format, run and deploy no loaded CLAUDE.md names a
//! command for.
//!
//! # Detection wraps `packages::detect`, and does not extend it
//!
//! `packages::Ecosystem` is the Packages page's contract: `program()`,
//! `update_hint()`, and `run::check_repo` spawning one tool per variant.
//! `Make`, `Just` and `Go` must not join it, so [`Toolchain`] wraps the
//! ecosystems `detect::projects` already finds and adds the markers that
//! page has no use for: `Makefile`/`justfile` targets, `go.mod`,
//! `Gemfile`, Gradle, an Xcode bundle whether or not SPM is resolved, and
//! a `pyproject.toml` no recognised tool owns.
//!
//! Two of `detect.rs`'s helpers swallow read errors (`has_xcode_spm` and
//! `has_project_file` return `false` when `read_dir` fails, and an
//! unreadable `pyproject.toml` with no lockfile falls through to nothing).
//! Every marker here is read through this module's own walk and reads,
//! which record the io error: a manifest that could not be read is a
//! [`Severity::Unknown`] finding carrying that error, never a shorter
//! list (absent is not zero, #846).
//!
//! # "Documented" means "named"
//!
//! A verb is named when a backtick span or a fenced-block line, read
//! through `text::spans` and `text::fences`, has the manager as its first
//! token followed by a target, script or subcommand that maps to the verb.
//! Prose never counts. Both directions of error are real on this very
//! repository: `make lint` at `CLAUDE.md:13` is an inline span, so a
//! fenced-only matcher would call lint undocumented; the same line names
//! `yarn lint` in order to say *not* that, so the wording is "names",
//! never "recommends". A manager token inside a path (`src-tauri/Cargo.toml`)
//! fails the first-token rule and counts for nothing.
//!
//! The target-to-verb map is by name (`test*`, `lint*`, `fmt*|format*`,
//! `build*`, `dev|run|start|serve`, `deploy|release|publish`). A target
//! that maps to nothing is listed as "other" in the evidence and is never
//! counted for or against a verb.
//!
//! # A negative needs a complete scan
//!
//! A positive ("`make lint` is named at `CLAUDE.md:13`") stands whatever
//! else was unreadable. A negative ("nothing names `make build`") is an
//! [`Severity::Advice`] finding only when every file a session would load
//! was read: no unreadable scope, directory or file in the scan, no
//! unreadable import, and no file this producer failed to re-read.
//! Otherwise the same (toolchain, verb) is a [`Severity::Unknown`] finding
//! naming what could not be read. `skipped_dirs` qualifies nothing; it is
//! a documented exclusion.
//!
//! No toolchain found is no finding. The report already says "nothing
//! found" only when the check ran, and `detect::projects` stops at depth
//! 3, so even that is a bounded claim this module adds nothing to.
//!
//! # Lint leakage, handed here by the content-shape research
//!
//! A CLAUDE.md line stating a code-style setting (indent width, quotes,
//! semicolons, trailing commas, line length, import order) while a
//! formatter config that sets that setting exists in the repository is a
//! second kind of finding under this check: the formatter already decides
//! it, and a line that restates it is a line the model can get wrong. The
//! survey behind the rule found it in 62% of 100 popular repositories.
//! Matching is deliberately narrow: an explicit setting, never the word
//! "style". The finding's sentence opens with `line N states`, which is
//! how [`suggestion`] tells the two kinds apart.

use super::{Check, Context, Evidence, Finding, Locator, Producer, Severity, Subject};
use crate::claudemd::{text, EffectiveScan, ImportNode, Scope};
use crate::packages::detect::projects;
use crate::packages::scripts::{self, Manifest};
use crate::packages::Ecosystem;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub struct Coverage;

impl Producer for Coverage {
    fn check(&self) -> Check {
        Check::Toolchain
    }

    fn run(&self, cx: &Context) -> Result<Vec<Finding>, String> {
        let detection = detect(cx.repo);
        let search = documented(cx.scan);
        let subject = subject_for(cx.repo, cx.scan);
        let mut out = Vec::new();

        // What could not be read, anywhere a session would load from.
        // Non-empty means no negative below may be stated as a fact.
        let mut unreadable: Vec<String> = Vec::new();
        unreadable.extend(cx.scan.unreadable.iter().cloned());
        unreadable.extend(cx.scan.repo.unreadable_dirs.iter().cloned());
        unreadable.extend(cx.scan.repo.unreadable_files.iter().cloned());
        for f in &cx.scan.repo.files {
            unreadable_imports(&f.imports, &mut unreadable);
        }
        for s in &cx.scan.extra {
            unreadable_imports(&s.file.imports, &mut unreadable);
        }
        unreadable.extend(search.unreadable.iter().cloned());

        for (dir, why) in &detection.unreadable_dirs {
            out.push(Finding::new(
                Check::Toolchain,
                Severity::Unknown,
                subject.clone(),
                vec![Evidence {
                    at: Locator::File {
                        path: dir.to_string_lossy().to_string(),
                        line: None,
                    },
                    measured: why.clone(),
                }],
                format!(
                    "`{}` could not be listed: {why}; toolchains under it are unknown",
                    dir.to_string_lossy()
                ),
            ));
        }
        for (manifest, why) in &detection.unreadable {
            out.push(Finding::new(
                Check::Toolchain,
                Severity::Unknown,
                subject.clone(),
                vec![Evidence {
                    at: Locator::File {
                        path: manifest.to_string_lossy().to_string(),
                        line: None,
                    },
                    measured: why.clone(),
                }],
                format!(
                    "`{}` could not be read: {why}; what it offers is unknown",
                    manifest.to_string_lossy()
                ),
            ));
        }

        let mut groups: BTreeMap<Toolchain, Vec<&DetectedToolchain>> = BTreeMap::new();
        for t in &detection.toolchains {
            groups.entry(t.toolchain).or_default().push(t);
        }

        for (kind, members) in &groups {
            let label = format!(
                "{} ({})",
                kind.name(),
                members
                    .iter()
                    .map(|m| m.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let offered: Vec<String> = dedup(
                members
                    .iter()
                    .flat_map(|m| m.offers.iter().map(|o| format!("`{}`", o.what))),
            );
            let mut verbs: BTreeSet<Verb> = BTreeSet::new();
            for m in members {
                verbs.extend(m.offers.iter().map(|o| o.verb));
            }

            for verb in verbs {
                let named = search
                    .named
                    .iter()
                    .any(|n| n.verb == verb && kind.managers().contains(&n.manager.as_str()));
                if named {
                    continue;
                }
                let candidates: Vec<String> = dedup(members.iter().flat_map(|m| {
                    m.offers
                        .iter()
                        .filter(|o| o.verb == verb)
                        .map(|o| format!("`{}`", o.command))
                }));
                let mut evidence: Vec<Evidence> = members
                    .iter()
                    .flat_map(|m| m.offers.iter().filter(|o| o.verb == verb))
                    .map(|o| Evidence {
                        at: Locator::File {
                            path: o.file.to_string_lossy().to_string(),
                            line: o.line,
                        },
                        measured: o.measured.clone(),
                    })
                    .collect();
                for m in members.iter().filter(|m| !m.other.is_empty()) {
                    evidence.push(Evidence {
                        at: Locator::File {
                            path: m.manifest.to_string_lossy().to_string(),
                            line: None,
                        },
                        measured: format!(
                            "not mapped to a verb, not counted: {}",
                            m.other
                                .iter()
                                .map(|o| format!("`{o}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    });
                }
                evidence.push(Evidence {
                    at: Locator::File {
                        path: subject.path().to_string(),
                        line: None,
                    },
                    measured: search.measured(),
                });

                let nothing_names = match search.files.len() {
                    0 => format!(
                        "no CLAUDE.md loads for this repository, so nothing names {}",
                        or_list(&candidates)
                    ),
                    n => format!(
                        "none of the {n} file{} read names {}",
                        if n == 1 { "" } else { "s" },
                        or_list(&candidates)
                    ),
                };

                if unreadable.is_empty() {
                    out.push(Finding::new(
                        Check::Toolchain,
                        Severity::Advice,
                        subject.clone(),
                        evidence,
                        format!("{label} offers {}; {nothing_names}", offered.join(", ")),
                    ));
                } else {
                    for u in &unreadable {
                        evidence.push(Evidence {
                            at: Locator::File {
                                path: u.clone(),
                                line: None,
                            },
                            measured: "not readable".to_string(),
                        });
                    }
                    out.push(Finding::new(
                        Check::Toolchain,
                        Severity::Unknown,
                        subject.clone(),
                        evidence,
                        format!(
                            "{label} offers {}; whether any loaded file names {} could not be \
                             decided: {} not readable",
                            offered.join(", "),
                            or_list(&candidates),
                            unreadable
                                .iter()
                                .map(|u| format!("`{u}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
            }
        }

        out.extend(lint_leakage(cx.repo, cx.scan, &detection.formatter_configs));
        Ok(out)
    }
}

/// The brief's suggestion for one of this producer's findings.
///
/// Called from `brief::suggestion`'s `Check::Toolchain` arm. Branches on
/// what the finding IS -- unreadable, a style line, or a gap -- by the
/// severity and the sentence's own opening, both set by this module.
pub fn suggestion(f: &Finding) -> String {
    if f.severity == Severity::Unknown {
        return format!(
            "Make the path named in the evidence readable and run the check again; no edit \
             to `{}` is suggested until it can be decided.",
            f.subject.path()
        );
    }
    if is_leakage(&f.finding) {
        return format!(
            "Delete the line named above from `{}`, or convert it into a hook that runs the \
             formatter; the config named in the evidence already sets it.",
            f.subject.path()
        );
    }
    format!(
        "In `{}`, name the command in a backtick span or a fenced block, beside the other \
         commands the file names, with the flags a session should use. State the command, \
         not a recommendation.",
        f.subject.path()
    )
}

/// A lint-leakage finding's sentence opens `line N states`.
fn is_leakage(finding: &str) -> bool {
    finding.starts_with("line ") && finding.contains(" states ") && finding.ends_with(" sets it")
}

/// The finding's subject: the root CLAUDE.md when the scan read one, else
/// the repository directory (a finding about a file that does not exist).
fn subject_for(repo: &Path, scan: &EffectiveScan) -> Subject {
    let root = repo.join("CLAUDE.md");
    let root_s = root.to_string_lossy();
    match scan.repo.files.iter().find(|f| {
        f.path == root_s.as_ref() || Path::new(&f.path).parent().is_some_and(|p| p == repo)
    }) {
        Some(f) => Subject::ClaudeMd {
            path: f.path.clone(),
            scope: Scope::Repo,
            section: None,
        },
        None => Subject::Directory {
            path: repo.to_string_lossy().to_string(),
        },
    }
}

/// The paths of every import the resolver could not read.
fn unreadable_imports(nodes: &[ImportNode], out: &mut Vec<String>) {
    for n in nodes {
        if n.unreadable {
            if let Some(p) = &n.path {
                out.push(p.clone());
            }
        }
        unreadable_imports(&n.children, out);
    }
}

fn dedup<I: IntoIterator<Item = String>>(items: I) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in items {
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

/// `a`, `a or b`, `a, b or c`.
fn or_list(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        n => format!("{} or {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

// ---------------------------------------------------------------------
// Verbs
// ---------------------------------------------------------------------

/// The six things a CLAUDE.md can name a command for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verb {
    Build,
    Test,
    Lint,
    Format,
    Run,
    Deploy,
}

/// The verb a make target, just recipe or package script maps to, by
/// name. `None` is "other": listed, never counted.
fn verb_by_name(name: &str) -> Option<Verb> {
    let n = name.to_ascii_lowercase();
    if n.starts_with("test") {
        Some(Verb::Test)
    } else if n.starts_with("lint") {
        Some(Verb::Lint)
    } else if n.starts_with("fmt") || n.starts_with("format") {
        Some(Verb::Format)
    } else if n.starts_with("build") {
        Some(Verb::Build)
    } else if matches!(n.as_str(), "dev" | "run" | "start" | "serve") {
        Some(Verb::Run)
    } else if matches!(n.as_str(), "deploy" | "release" | "publish") {
        Some(Verb::Deploy)
    } else {
        None
    }
}

/// The verb a command names, given its manager token and the arguments
/// after it. The first token is the manager or the command counts for
/// nothing; that rule is what keeps `src-tauri/Cargo.toml` from being a
/// cargo command.
fn verb_of(manager: &str, args: &[&str]) -> Option<Verb> {
    let first = args.first().copied();
    match manager {
        "make" | "just" => first.and_then(verb_by_name),
        "yarn" => match first {
            Some("run") => args.get(1).copied().and_then(verb_by_name),
            Some(s) => verb_by_name(s),
            None => None,
        },
        "npm" => match first {
            Some("run") | Some("run-script") => args.get(1).copied().and_then(verb_by_name),
            // `npm test` and `npm start` are the two scripts npm runs
            // without `run`. `npm install` maps to nothing.
            Some(s @ ("test" | "start")) => verb_by_name(s),
            _ => None,
        },
        "cargo" => match first {
            Some("build") => Some(Verb::Build),
            Some("test") => Some(Verb::Test),
            Some("clippy") => Some(Verb::Lint),
            Some("fmt") => Some(Verb::Format),
            Some("run") => Some(Verb::Run),
            Some("publish") => Some(Verb::Deploy),
            _ => None,
        },
        "go" => match first {
            Some("build") => Some(Verb::Build),
            Some("test") => Some(Verb::Test),
            Some("vet") => Some(Verb::Lint),
            Some("fmt") => Some(Verb::Format),
            Some("run") => Some(Verb::Run),
            _ => None,
        },
        "gradle" => match first {
            Some("build") | Some("assemble") => Some(Verb::Build),
            Some("test") => Some(Verb::Test),
            Some("check") => Some(Verb::Lint),
            Some("run") => Some(Verb::Run),
            Some("publish") => Some(Verb::Deploy),
            Some(s) => verb_by_name(s),
            None => None,
        },
        // Actions can follow `-scheme X` and other options, so any
        // literal action word among the arguments counts.
        "xcodebuild" => args.iter().find_map(|a| match *a {
            "build" => Some(Verb::Build),
            "test" => Some(Verb::Test),
            "archive" => Some(Verb::Deploy),
            _ => None,
        }),
        "poetry" | "uv" => match first {
            Some("build") => Some(Verb::Build),
            Some("publish") => Some(Verb::Deploy),
            Some("run") => tool_verb(&args[1..]),
            _ => None,
        },
        "pytest" | "ruff" | "rspec" | "rubocop" | "rake" => tool_verb(
            &std::iter::once(manager)
                .chain(args.iter().copied())
                .collect::<Vec<_>>(),
        ),
        "dotnet" => match first {
            Some("build") => Some(Verb::Build),
            Some("test") => Some(Verb::Test),
            Some("format") => Some(Verb::Format),
            Some("run") => Some(Verb::Run),
            Some("publish") => Some(Verb::Deploy),
            _ => None,
        },
        "swift" => match first {
            Some("build") => Some(Verb::Build),
            Some("test") => Some(Verb::Test),
            Some("run") => Some(Verb::Run),
            _ => None,
        },
        "terraform" => match first {
            Some("validate") => Some(Verb::Lint),
            Some("fmt") => Some(Verb::Format),
            Some("apply") => Some(Verb::Deploy),
            _ => None,
        },
        "bundle" => match first {
            Some("exec") => tool_verb(&args[1..]),
            _ => None,
        },
        _ => None,
    }
}

/// A tool run through `poetry run`, `uv run` or `bundle exec`, or named
/// bare.
fn tool_verb(args: &[&str]) -> Option<Verb> {
    match args.first().copied() {
        Some("pytest") | Some("rspec") => Some(Verb::Test),
        Some("rubocop") => Some(Verb::Lint),
        Some("ruff") => match args.get(1).copied() {
            Some("check") => Some(Verb::Lint),
            Some("format") => Some(Verb::Format),
            _ => None,
        },
        Some("rake") => args.get(1).copied().and_then(verb_by_name),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------

/// A build system or package manager on disk.
///
/// Wraps `packages::Ecosystem` rather than extending it, for the reason
/// in the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Toolchain {
    Ecosystem(Ecosystem),
    Make,
    Just,
    Go,
    Bundler,
    Gradle,
    Xcode,
    /// A `pyproject.toml` that neither Poetry nor uv owns.
    PyprojectUnknown,
}

impl Toolchain {
    fn name(self) -> &'static str {
        match self {
            Toolchain::Ecosystem(Ecosystem::Npm) => "npm",
            Toolchain::Ecosystem(Ecosystem::Yarn) => "yarn",
            Toolchain::Ecosystem(Ecosystem::Poetry) => "poetry",
            Toolchain::Ecosystem(Ecosystem::Uv) => "uv",
            Toolchain::Ecosystem(Ecosystem::Dotnet) => "dotnet",
            Toolchain::Ecosystem(Ecosystem::Cocoapods) => "cocoapods",
            Toolchain::Ecosystem(Ecosystem::Terraform) => "terraform",
            Toolchain::Ecosystem(Ecosystem::Swift) => "swift",
            Toolchain::Ecosystem(Ecosystem::Cargo) => "cargo",
            Toolchain::Make => "make",
            Toolchain::Just => "just",
            Toolchain::Go => "go",
            Toolchain::Bundler => "bundler",
            Toolchain::Gradle => "gradle",
            Toolchain::Xcode => "xcode",
            Toolchain::PyprojectUnknown => "pyproject (tool unknown)",
        }
    }

    /// The first tokens that name a command of this toolchain.
    fn managers(self) -> &'static [&'static str] {
        match self {
            Toolchain::Ecosystem(Ecosystem::Npm) => &["npm"],
            Toolchain::Ecosystem(Ecosystem::Yarn) => &["yarn"],
            Toolchain::Ecosystem(Ecosystem::Poetry) => &["poetry", "pytest", "ruff"],
            Toolchain::Ecosystem(Ecosystem::Uv) => &["uv", "pytest", "ruff"],
            Toolchain::Ecosystem(Ecosystem::Dotnet) => &["dotnet"],
            Toolchain::Ecosystem(Ecosystem::Cocoapods) => &["pod"],
            Toolchain::Ecosystem(Ecosystem::Terraform) => &["terraform"],
            Toolchain::Ecosystem(Ecosystem::Swift) => &["swift"],
            Toolchain::Ecosystem(Ecosystem::Cargo) => &["cargo"],
            Toolchain::Make => &["make"],
            Toolchain::Just => &["just"],
            Toolchain::Go => &["go"],
            Toolchain::Bundler => &["bundle", "rspec", "rubocop", "rake"],
            Toolchain::Gradle => &["gradle"],
            Toolchain::Xcode => &["xcodebuild"],
            Toolchain::PyprojectUnknown => &["pytest", "ruff"],
        }
    }
}

/// One command a toolchain offers for one verb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub verb: Verb,
    /// The command as a CLAUDE.md would name it: `make test`.
    pub command: String,
    /// The target, script or subcommand alone: `test`.
    pub what: String,
    /// The manifest the offer comes from.
    pub file: PathBuf,
    /// The manifest line, when the parser recorded one. `package.json`
    /// scripts carry none; a made-up line would be a confident wrong
    /// number.
    pub line: Option<u32>,
    /// What was measured: "target `test`", "script `test`".
    pub measured: String,
}

/// One toolchain found in one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedToolchain {
    pub toolchain: Toolchain,
    pub dir: PathBuf,
    /// The manifest the detection rests on.
    pub manifest: PathBuf,
    /// "Makefile at root", "Cargo.toml at src-tauri".
    pub label: String,
    pub offers: Vec<Offer>,
    /// Targets or scripts that map to no verb. Listed, never counted.
    pub other: Vec<String>,
}

/// Everything detection found, and everything it could not read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detection {
    pub toolchains: Vec<DetectedToolchain>,
    /// Manifests that exist and could not be read or parsed, with the
    /// error. Each is an Unknown finding.
    pub unreadable: Vec<(PathBuf, String)>,
    /// Directories the walk could not list, with the error.
    pub unreadable_dirs: Vec<(PathBuf, String)>,
    /// Formatter configs found, for the lint-leakage rule.
    pub formatter_configs: Vec<PathBuf>,
}

/// The same bound and the same exclusions as `detect::projects`, so the
/// two walks see the same directories. Dot-directories are skipped there
/// too.
const MAX_DEPTH: usize = 3;
const SKIP: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".terraform",
    ".venv",
    "venv",
    "dist",
    "build",
    "bin",
    "obj",
    ".worktrees",
];

/// One listed directory and its sorted entry names.
type Listed = (PathBuf, Vec<String>);
/// One directory that could not be listed, with the io error.
type Unlisted = (PathBuf, String);

/// Every directory to `MAX_DEPTH`, with its entry names, plus the
/// directories that could not be listed.
fn walk(repo: &Path) -> (Vec<Listed>, Vec<Unlisted>) {
    let mut dirs = Vec::new();
    let mut unreadable = Vec::new();
    let mut queue = std::collections::VecDeque::from([(repo.to_path_buf(), 0usize)]);
    while let Some((dir, depth)) = queue.pop_front() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                unreadable.push((dir, e.to_string()));
                continue;
            }
        };
        let mut names = Vec::new();
        let mut children = Vec::new();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let is_dir = e.metadata().map(|m| m.is_dir()).unwrap_or(false);
            if is_dir
                && depth < MAX_DEPTH
                && !SKIP.contains(&name.as_str())
                && !name.starts_with('.')
            {
                children.push(e.path());
            }
            names.push(name);
        }
        names.sort();
        children.sort();
        dirs.push((dir, names));
        for c in children {
            queue.push_back((c, depth + 1));
        }
    }
    (dirs, unreadable)
}

fn label_for(manifest: &str, repo: &Path, dir: &Path) -> String {
    let rel = dir
        .strip_prefix(repo)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    if rel.is_empty() {
        format!("{manifest} at root")
    } else {
        format!("{manifest} at {rel}")
    }
}

fn fixed(verb: Verb, manager: &str, sub: &str, file: &Path) -> Offer {
    Offer {
        verb,
        command: format!("{manager} {sub}"),
        what: sub.to_string(),
        file: file.to_path_buf(),
        line: None,
        measured: format!("`{manager} {sub}` is a built-in subcommand"),
    }
}

/// Every toolchain under `repo`, to depth 3.
pub fn detect(repo: &Path) -> Detection {
    let mut out = Detection::default();
    let (dirs, unreadable_dirs) = walk(repo);
    out.unreadable_dirs = unreadable_dirs;
    let names_of: BTreeMap<&Path, &Vec<String>> =
        dirs.iter().map(|(d, n)| (d.as_path(), n)).collect();

    // The ecosystems the Packages page already finds.
    let projects = projects(repo);
    for p in &projects {
        let dir = PathBuf::from(&p.path);
        let names = names_of.get(dir.as_path()).copied();
        let has = |n: &str| names.is_some_and(|ns| ns.iter().any(|x| x == n));
        for eco in &p.ecosystems {
            let mut t = DetectedToolchain {
                toolchain: Toolchain::Ecosystem(*eco),
                dir: dir.clone(),
                manifest: dir.clone(),
                label: String::new(),
                offers: Vec::new(),
                other: Vec::new(),
            };
            match eco {
                Ecosystem::Yarn | Ecosystem::Npm => {
                    t.manifest = dir.join("package.json");
                    t.label = label_for(
                        if *eco == Ecosystem::Yarn {
                            "package.json + yarn.lock"
                        } else {
                            "package.json"
                        },
                        repo,
                        &dir,
                    );
                    match scripts::scripts(&dir) {
                        Manifest::Present(list) => {
                            for s in list {
                                match verb_by_name(&s) {
                                    Some(verb) => t.offers.push(Offer {
                                        verb,
                                        command: if *eco == Ecosystem::Yarn {
                                            format!("yarn {s}")
                                        } else {
                                            format!("npm run {s}")
                                        },
                                        what: s.clone(),
                                        file: t.manifest.clone(),
                                        line: None,
                                        measured: format!("script `{s}`"),
                                    }),
                                    None => t.other.push(s),
                                }
                            }
                        }
                        Manifest::Unreadable(e) => {
                            out.unreadable.push((t.manifest.clone(), e));
                            continue;
                        }
                        Manifest::Absent => continue,
                    }
                }
                Ecosystem::Cargo => {
                    t.manifest = dir.join("Cargo.toml");
                    t.label = label_for("Cargo.toml", repo, &dir);
                    let text = match std::fs::read_to_string(&t.manifest) {
                        Ok(s) => s,
                        Err(e) => {
                            out.unreadable.push((t.manifest.clone(), e.to_string()));
                            continue;
                        }
                    };
                    // `toml::from_str`, not `str::parse`: `Value: FromStr`
                    // parses one value expression, so a document opening
                    // with `[package]` reads as an array and fails.
                    let doc: toml::Table = match toml::from_str(&text) {
                        Ok(v) => v,
                        Err(e) => {
                            out.unreadable.push((t.manifest.clone(), e.to_string()));
                            continue;
                        }
                    };
                    let members: Vec<String> = doc
                        .get("workspace")
                        .and_then(|w| w.get("members"))
                        .and_then(|m| m.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    if !members.is_empty() {
                        t.label = format!(
                            "{} (workspace of {} member{})",
                            t.label,
                            members.len(),
                            if members.len() == 1 { "" } else { "s" }
                        );
                    }
                    let m = t.manifest.clone();
                    t.offers.push(fixed(Verb::Build, "cargo", "build", &m));
                    t.offers.push(fixed(Verb::Test, "cargo", "test", &m));
                    t.offers.push(fixed(Verb::Lint, "cargo", "clippy", &m));
                    t.offers.push(fixed(Verb::Format, "cargo", "fmt", &m));
                    // `cargo run` only where there is a binary to run.
                    let has_bin = doc.get("bin").is_some()
                        || names_of
                            .get(dir.join("src").as_path())
                            .is_some_and(|ns| ns.iter().any(|n| n == "main.rs"));
                    if has_bin {
                        t.offers.push(fixed(Verb::Run, "cargo", "run", &m));
                    }
                }
                Ecosystem::Poetry | Ecosystem::Uv => {
                    t.manifest = dir.join("pyproject.toml");
                    t.label = label_for("pyproject.toml", repo, &dir);
                    let manager = if *eco == Ecosystem::Poetry {
                        "poetry"
                    } else {
                        "uv"
                    };
                    let m = t.manifest.clone();
                    t.offers.push(fixed(Verb::Build, manager, "build", &m));
                    t.offers.push(fixed(Verb::Deploy, manager, "publish", &m));
                    match pyproject_tools(&m) {
                        Ok(offers) => t.offers.extend(offers),
                        Err(e) => {
                            out.unreadable.push((m, e));
                            continue;
                        }
                    }
                }
                Ecosystem::Dotnet => {
                    let project = names
                        .and_then(|ns| {
                            ns.iter().find(|n| {
                                Path::new(n).extension().is_some_and(|x| {
                                    ["csproj", "fsproj", "vbproj", "sln"]
                                        .iter()
                                        .any(|w| x.eq_ignore_ascii_case(w))
                                })
                            })
                        })
                        .cloned()
                        .unwrap_or_else(|| "project file".to_string());
                    t.manifest = dir.join(&project);
                    t.label = label_for(&project, repo, &dir);
                    let m = t.manifest.clone();
                    t.offers.push(fixed(Verb::Build, "dotnet", "build", &m));
                    t.offers.push(fixed(Verb::Test, "dotnet", "test", &m));
                    t.offers.push(fixed(Verb::Format, "dotnet", "format", &m));
                }
                Ecosystem::Swift => {
                    // `swift build` needs a package; an Xcode-managed
                    // project without one is the Xcode toolchain below.
                    if !has("Package.swift") {
                        continue;
                    }
                    t.manifest = dir.join("Package.swift");
                    t.label = label_for("Package.swift", repo, &dir);
                    let m = t.manifest.clone();
                    t.offers.push(fixed(Verb::Build, "swift", "build", &m));
                    t.offers.push(fixed(Verb::Test, "swift", "test", &m));
                }
                Ecosystem::Terraform => {
                    t.label = label_for(".terraform.lock.hcl", repo, &dir);
                    let m = t.manifest.clone();
                    t.offers
                        .push(fixed(Verb::Lint, "terraform", "validate", &m));
                    t.offers.push(fixed(Verb::Format, "terraform", "fmt", &m));
                    t.offers.push(fixed(Verb::Deploy, "terraform", "apply", &m));
                }
                Ecosystem::Cocoapods => {
                    // Dependencies only; `pod` builds nothing.
                    t.manifest = dir.join("Podfile");
                    t.label = label_for("Podfile", repo, &dir);
                }
            }
            out.toolchains.push(t);
        }
    }

    // The markers `detect` has no use for.
    for (dir, names) in &dirs {
        let has = |n: &str| names.iter().any(|x| x == n);
        let claimed = |eco: Ecosystem| {
            projects
                .iter()
                .any(|p| Path::new(&p.path) == dir && p.ecosystems.contains(&eco))
        };

        match scripts::targets(dir) {
            Manifest::Present(targets) => {
                for kind in [Toolchain::Make, Toolchain::Just] {
                    let mine: Vec<_> = targets
                        .iter()
                        .filter(|t| (t.file == "justfile") == (kind == Toolchain::Just))
                        .collect();
                    if mine.is_empty() {
                        continue;
                    }
                    let file = mine[0].file.clone();
                    let manager = if kind == Toolchain::Just {
                        "just"
                    } else {
                        "make"
                    };
                    let mut t = DetectedToolchain {
                        toolchain: kind,
                        dir: dir.clone(),
                        manifest: dir.join(&file),
                        label: label_for(&file, repo, dir),
                        offers: Vec::new(),
                        other: Vec::new(),
                    };
                    for target in mine {
                        match verb_by_name(&target.name) {
                            Some(verb) => t.offers.push(Offer {
                                verb,
                                command: format!("{manager} {}", target.name),
                                what: target.name.clone(),
                                file: t.manifest.clone(),
                                line: u32::try_from(target.line).ok(),
                                measured: format!("target `{}`", target.name),
                            }),
                            None => t.other.push(target.name.clone()),
                        }
                    }
                    out.toolchains.push(t);
                }
            }
            Manifest::Unreadable(e) => out.unreadable.push((dir.clone(), e)),
            Manifest::Absent => {}
        }

        if has("go.mod") {
            let m = dir.join("go.mod");
            out.toolchains.push(DetectedToolchain {
                toolchain: Toolchain::Go,
                dir: dir.clone(),
                manifest: m.clone(),
                label: label_for("go.mod", repo, dir),
                offers: vec![
                    fixed(Verb::Build, "go", "build", &m),
                    fixed(Verb::Test, "go", "test", &m),
                    fixed(Verb::Lint, "go", "vet", &m),
                    fixed(Verb::Format, "go", "fmt", &m),
                ],
                other: Vec::new(),
            });
        }

        if has("Gemfile") {
            // A Gemfile says which gems, not how to build; the verbs
            // would come from a Rakefile this producer does not parse.
            out.toolchains.push(DetectedToolchain {
                toolchain: Toolchain::Bundler,
                dir: dir.clone(),
                manifest: dir.join("Gemfile"),
                label: label_for("Gemfile", repo, dir),
                offers: Vec::new(),
                other: Vec::new(),
            });
        }

        if let Some(g) = [
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ]
        .into_iter()
        .find(|g| has(g))
        {
            let m = dir.join(g);
            let manager = if has("gradlew") {
                "./gradlew"
            } else {
                "gradle"
            };
            out.toolchains.push(DetectedToolchain {
                toolchain: Toolchain::Gradle,
                dir: dir.clone(),
                manifest: m.clone(),
                label: label_for(g, repo, dir),
                offers: vec![
                    fixed(Verb::Build, manager, "build", &m),
                    fixed(Verb::Test, manager, "test", &m),
                    fixed(Verb::Lint, manager, "check", &m),
                ],
                other: Vec::new(),
            });
        }

        if let Some(bundle) = names.iter().find(|n| {
            Path::new(n)
                .extension()
                .is_some_and(|x| x == "xcodeproj" || x == "xcworkspace")
        }) {
            let m = dir.join(bundle);
            out.toolchains.push(DetectedToolchain {
                toolchain: Toolchain::Xcode,
                dir: dir.clone(),
                manifest: m.clone(),
                label: label_for(bundle, repo, dir),
                offers: vec![
                    fixed(Verb::Build, "xcodebuild", "build", &m),
                    fixed(Verb::Test, "xcodebuild", "test", &m),
                ],
                other: Vec::new(),
            });
        }

        if has("pyproject.toml") && !claimed(Ecosystem::Poetry) && !claimed(Ecosystem::Uv) {
            let m = dir.join("pyproject.toml");
            match pyproject_tools(&m) {
                Ok(offers) => out.toolchains.push(DetectedToolchain {
                    toolchain: Toolchain::PyprojectUnknown,
                    dir: dir.clone(),
                    manifest: m.clone(),
                    label: label_for("pyproject.toml", repo, dir),
                    offers,
                    other: Vec::new(),
                }),
                Err(e) => out.unreadable.push((m, e)),
            }
        }

        for n in names {
            let is_config = n.starts_with(".prettierrc")
                || matches!(
                    n.as_str(),
                    "rustfmt.toml"
                        | ".rustfmt.toml"
                        | ".editorconfig"
                        | "biome.json"
                        | "biome.jsonc"
                        | "ruff.toml"
                        | ".ruff.toml"
                        | ".clang-format"
                );
            if is_config {
                out.formatter_configs.push(dir.join(n));
            }
        }
    }

    out.toolchains
        .sort_by(|a, b| a.toolchain.cmp(&b.toolchain).then(a.dir.cmp(&b.dir)));
    out
}

/// The tools a `pyproject.toml` configures: `[tool.pytest…]` offers
/// `pytest`, `[tool.ruff…]` offers `ruff check` and `ruff format`.
fn pyproject_tools(path: &Path) -> Result<Vec<Offer>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if text.contains("[tool.pytest") {
        out.push(Offer {
            verb: Verb::Test,
            command: "pytest".into(),
            what: "pytest".into(),
            file: path.to_path_buf(),
            line: None,
            measured: "`[tool.pytest]` is configured".into(),
        });
    }
    if text.contains("[tool.ruff") {
        for (verb, sub) in [(Verb::Lint, "check"), (Verb::Format, "format")] {
            out.push(Offer {
                verb,
                command: format!("ruff {sub}"),
                what: format!("ruff {sub}"),
                file: path.to_path_buf(),
                line: None,
                measured: "`[tool.ruff]` is configured".into(),
            });
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// What the loaded files name
// ---------------------------------------------------------------------

/// One command a loaded file names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// The first token, normalised: `./gradlew` is `gradle`.
    pub manager: String,
    pub verb: Verb,
    pub file: PathBuf,
    pub line: u32,
    /// The span or fenced line, verbatim.
    pub text: String,
}

/// Every command the loaded files name, and how much was searched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    pub named: Vec<Named>,
    /// Every file read: each CLAUDE.md the scan loaded and each import
    /// it resolved.
    pub files: Vec<PathBuf>,
    pub spans: usize,
    pub fenced_lines: usize,
    /// Files the scan listed and this producer could not re-read.
    pub unreadable: Vec<String>,
}

impl Search {
    /// The count and how it was counted, for the evidence.
    fn measured(&self) -> String {
        let n = self.files.len();
        let list = self
            .files
            .iter()
            .map(|f| format!("`{}`", f.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{n} file{} read{}, {} span{} and {} fenced line{} searched",
            if n == 1 { "" } else { "s" },
            if n == 0 {
                String::new()
            } else {
                format!(" ({list})")
            },
            self.spans,
            if self.spans == 1 { "" } else { "s" },
            self.fenced_lines,
            if self.fenced_lines == 1 { "" } else { "s" },
        )
    }
}

/// Every file a session loads from this scan: the CLAUDE.md files and
/// every import that resolved and read. An import the resolver could not
/// read is not listed; it is already an unreadable path.
fn loaded_files(scan: &EffectiveScan) -> Vec<PathBuf> {
    fn imports(nodes: &[ImportNode], out: &mut Vec<PathBuf>) {
        for n in nodes {
            if n.problem.is_none() {
                if let Some(p) = &n.path {
                    out.push(PathBuf::from(p));
                }
            }
            imports(&n.children, out);
        }
    }
    let mut out = Vec::new();
    for f in &scan.repo.files {
        out.push(PathBuf::from(&f.path));
        imports(&f.imports, &mut out);
    }
    for s in &scan.extra {
        out.push(PathBuf::from(&s.file.path));
        imports(&s.file.imports, &mut out);
    }
    out.dedup();
    out
}

/// What the loaded files name, through `text::spans` and `text::fences`.
pub fn documented(scan: &EffectiveScan) -> Search {
    let mut out = Search::default();
    for path in loaded_files(scan) {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                out.unreadable
                    .push(format!("{} ({e})", path.to_string_lossy()));
                continue;
            }
        };
        for s in text::spans(&text) {
            out.spans += 1;
            for (manager, verb) in commands_in(&s.text) {
                out.named.push(Named {
                    manager,
                    verb,
                    file: path.clone(),
                    line: u32::try_from(s.line).unwrap_or(u32::MAX),
                    text: s.text.clone(),
                });
            }
        }
        for f in text::fences(&text) {
            for (i, line) in f.body.split('\n').enumerate() {
                if line.trim().is_empty() || line.trim_start().starts_with('#') {
                    continue;
                }
                out.fenced_lines += 1;
                for (manager, verb) in commands_in(line) {
                    out.named.push(Named {
                        manager,
                        verb,
                        file: path.clone(),
                        line: u32::try_from(f.line + 1 + i).unwrap_or(u32::MAX),
                        text: line.to_string(),
                    });
                }
            }
        }
        out.files.push(path);
    }
    out
}

/// The verbs one span or one fenced line names. A line can chain
/// commands (`cd src-tauri && cargo test --lib`), so each segment is read
/// on its own; a `$ ` prompt is stripped.
fn commands_in(text: &str) -> Vec<(String, Verb)> {
    let mut out = Vec::new();
    for segment in split_chain(text) {
        let segment = segment.trim();
        let segment = segment.strip_prefix("$ ").unwrap_or(segment);
        let tokens: Vec<&str> = segment.split_whitespace().collect();
        let Some(first) = tokens.first() else {
            continue;
        };
        let manager = match *first {
            "./gradlew" | "gradlew" => "gradle",
            other => other,
        };
        if let Some(verb) = verb_of(manager, &tokens[1..]) {
            out.push((manager.to_string(), verb));
        }
    }
    out
}

/// A line split at `&&`, `||`, `;` and `|`, in order.
fn split_chain(text: &str) -> Vec<&str> {
    const SEPS: [&str; 4] = ["&&", "||", ";", "|"];
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        let next = SEPS
            .iter()
            .filter_map(|s| rest.find(s).map(|i| (i, s.len())))
            .min();
        match next {
            Some((i, len)) => {
                out.push(&rest[..i]);
                rest = &rest[i + len..];
            }
            None => {
                out.push(rest);
                return out;
            }
        }
    }
}

// ---------------------------------------------------------------------
// Lint leakage
// ---------------------------------------------------------------------

/// A code-style setting a line can state, and the configs that set it.
struct Setting {
    what: &'static str,
    pattern: LazyLock<Regex>,
    /// Config file names (or the `.prettierrc` prefix) that decide it.
    configs: &'static [&'static str],
}

const PRETTIER: &str = ".prettierrc";

static SETTINGS: [Setting; 6] = [
    Setting {
        what: "an indent width",
        pattern: LazyLock::new(|| {
            Regex::new(r"(?i)\b(indent|indentation)\b.*\b(\d+|tabs?|spaces?)\b|\b(\d+[ -]spaces?|tabs?)\b.*\bindent").unwrap()
        }),
        configs: &[
            PRETTIER,
            ".editorconfig",
            "rustfmt.toml",
            ".rustfmt.toml",
            "biome.json",
            "biome.jsonc",
            "ruff.toml",
            ".ruff.toml",
            ".clang-format",
        ],
    },
    Setting {
        what: "a quote style",
        pattern: LazyLock::new(|| Regex::new(r"(?i)\b(single|double)[ -]quot(e|es|ed)\b").unwrap()),
        configs: &[
            PRETTIER,
            "biome.json",
            "biome.jsonc",
            "ruff.toml",
            ".ruff.toml",
        ],
    },
    Setting {
        what: "a semicolon rule",
        pattern: LazyLock::new(|| Regex::new(r"(?i)\bsemicolons?\b").unwrap()),
        configs: &[PRETTIER, "biome.json", "biome.jsonc"],
    },
    Setting {
        what: "a trailing-comma rule",
        pattern: LazyLock::new(|| Regex::new(r"(?i)\btrailing[ -]commas?\b").unwrap()),
        configs: &[
            PRETTIER,
            "biome.json",
            "biome.jsonc",
            "rustfmt.toml",
            ".rustfmt.toml",
        ],
    },
    Setting {
        what: "a line length",
        pattern: LazyLock::new(|| {
            Regex::new(r"(?i)\b(line[ -](length|width)|max(imum)?[ -]line|\d+[ -](columns|chars|characters)\b.*\b(line|wide))\b").unwrap()
        }),
        configs: &[
            PRETTIER,
            ".editorconfig",
            "rustfmt.toml",
            ".rustfmt.toml",
            "biome.json",
            "biome.jsonc",
            "ruff.toml",
            ".ruff.toml",
            ".clang-format",
        ],
    },
    Setting {
        what: "an import order",
        pattern: LazyLock::new(|| {
            Regex::new(r"(?i)\b(import[ -](order|ordering|sorting)|sort(ed)?[ -]imports)\b")
                .unwrap()
        }),
        configs: &[
            "rustfmt.toml",
            ".rustfmt.toml",
            "biome.json",
            "biome.jsonc",
            "ruff.toml",
            ".ruff.toml",
            ".clang-format",
        ],
    },
];

fn config_sets(config: &Path, setting: &Setting) -> bool {
    let name = config
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    setting.configs.iter().any(|c| {
        if *c == PRETTIER {
            name.starts_with(PRETTIER)
        } else {
            name == *c
        }
    })
}

/// One finding per CLAUDE.md prose line that states a setting some
/// formatter config in the repository sets.
fn lint_leakage(repo: &Path, scan: &EffectiveScan, configs: &[PathBuf]) -> Vec<Finding> {
    let mut out = Vec::new();
    if configs.is_empty() {
        return out;
    }
    let files: Vec<(String, Scope)> = scan
        .repo
        .files
        .iter()
        .map(|f| (f.path.clone(), Scope::Repo))
        .chain(scan.extra.iter().map(|s| (s.file.path.clone(), s.scope)))
        .collect();
    for (path, scope) in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            // Already an unreadable path in `documented`; nothing to say
            // twice.
            continue;
        };
        for (n, line) in text::prose_lines(&text) {
            for setting in &SETTINGS {
                if !setting.pattern.is_match(line) {
                    continue;
                }
                let setters: Vec<&PathBuf> =
                    configs.iter().filter(|c| config_sets(c, setting)).collect();
                if setters.is_empty() {
                    continue;
                }
                let names: Vec<String> = setters
                    .iter()
                    .map(|c| format!("`{}`", c.strip_prefix(repo).unwrap_or(c).to_string_lossy()))
                    .collect();
                let mut evidence = vec![Evidence {
                    at: Locator::File {
                        path: path.clone(),
                        line: u32::try_from(n).ok(),
                    },
                    measured: clamp(line.trim(), 160),
                }];
                evidence.extend(setters.iter().map(|c| Evidence {
                    at: Locator::File {
                        path: c.to_string_lossy().to_string(),
                        line: None,
                    },
                    measured: format!("sets {}", setting.what),
                }));
                out.push(Finding::new(
                    Check::Toolchain,
                    Severity::Advice,
                    Subject::ClaudeMd {
                        path: path.clone(),
                        scope,
                        section: None,
                    },
                    evidence,
                    format!(
                        "line {n} states {}; {} sets it",
                        setting.what,
                        names.join(" and ")
                    ),
                ));
                // One finding per line, whatever else it states.
                break;
            }
        }
    }
    out
}

fn clamp(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claudemd::advice::{CheckRun, Report};
    use crate::claudemd::scan_effective_in;
    use std::fs;

    /// A repository and a home directory with no global CLAUDE.md, so
    /// the scan is complete and a negative can be stated. (`home: None`
    /// records the global scope as unreadable, which would make every
    /// negative Unknown.)
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("octocat-app");
        let home = t.path().join("home");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(home.join(".claude")).unwrap();
        (t, repo, home)
    }

    fn run_over(repo: &Path, home: &Path) -> Report {
        let scan = scan_effective_in(repo, home);
        let cx = Context {
            repo,
            home: Some(home),
            scan: &scan,
            definitions: None,
            conn: None,
        };
        super::super::run(&cx)
    }

    fn toolchain_findings(r: &Report) -> Vec<&Finding> {
        r.findings
            .iter()
            .filter(|f| f.check == Check::Toolchain)
            .collect()
    }

    /// This check's coverage row, found by check rather than position:
    /// every producer runs, and the report lists them in `Check::ALL`
    /// order.
    fn coverage(r: &Report) -> CheckRun {
        r.checks
            .iter()
            .find(|c| c.check == Check::Toolchain)
            .expect("a coverage row for toolchain")
            .run
            .clone()
    }

    fn yarn_app(repo: &Path) {
        fs::write(
            repo.join("package.json"),
            r#"{"scripts":{"test":"vitest","lint":"eslint ."}}"#,
        )
        .unwrap();
        fs::write(repo.join("yarn.lock"), "").unwrap();
    }

    /// Fixture 1: yarn with `test` and `lint` scripts and a CLAUDE.md
    /// naming only `yarn lint` is one Advice finding, for test, whose
    /// evidence names `package.json` and the count of spans searched.
    /// Build, run and deploy are not offered, so nothing is said about
    /// them.
    #[test]
    fn a_yarn_script_nothing_names_is_one_advice_finding() {
        let (_t, repo, home) = fixture();
        yarn_app(&repo);
        fs::write(repo.join("CLAUDE.md"), "Run `yarn lint` before pushing.\n").unwrap();

        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);

        assert_eq!(found.len(), 1, "{report:#?}");
        let f = found[0];
        assert_eq!(f.severity, Severity::Advice);
        assert_eq!(
            f.finding,
            "yarn (package.json + yarn.lock at root) offers `test`, `lint`; none of the 1 file \
             read names `yarn test`"
        );
        assert_eq!(f.subject.path(), repo.join("CLAUDE.md").to_string_lossy());
        assert!(
            f.evidence.iter().any(|e| matches!(
                &e.at,
                Locator::File { path, line: None } if path.ends_with("package.json")
            ) && e.measured == "script `test`"),
            "{:?}",
            f.evidence
        );
        assert!(
            f.evidence
                .iter()
                .any(|e| e.measured.contains("1 file read") && e.measured.contains("1 span")),
            "{:?}",
            f.evidence
        );
        assert!(f.brief.contains("Suggested change: In `"), "{}", f.brief);
        assert_eq!(coverage(&report), CheckRun::Ran { findings: 1 });

        // The negative can fail: naming the test script clears it.
        fs::write(
            repo.join("CLAUDE.md"),
            "Run `yarn lint` and `yarn test` before pushing.\n",
        )
        .unwrap();
        let report = run_over(&repo, &home);
        assert!(toolchain_findings(&report).is_empty(), "{report:#?}");
    }

    /// Fixture 2: a Makefile with `test:` and `lint:` and a CLAUDE.md
    /// naming neither lists both targets by name and line. With no
    /// CLAUDE.md at all the subject is the directory.
    #[test]
    fn make_targets_are_cited_by_name_and_line() {
        let (_t, repo, home) = fixture();
        yarn_app(&repo);
        fs::write(
            repo.join("Makefile"),
            ".PHONY: test lint\ntest:\n\tyarn test\n\nlint:\n\tyarn lint\n",
        )
        .unwrap();
        fs::write(repo.join("CLAUDE.md"), "Be careful.\n").unwrap();

        let report = run_over(&repo, &home);
        let make: Vec<&Finding> = toolchain_findings(&report)
            .into_iter()
            .filter(|f| f.finding.starts_with("make ("))
            .collect();
        let sentences: Vec<&str> = make.iter().map(|f| f.finding.as_str()).collect();
        assert_eq!(
            sentences,
            vec![
                "make (Makefile at root) offers `test`, `lint`; none of the 1 file read names \
                 `make test`",
                "make (Makefile at root) offers `test`, `lint`; none of the 1 file read names \
                 `make lint`",
            ]
        );
        let makefile = repo.join("Makefile").to_string_lossy().to_string();
        assert!(
            make[0]
                .brief
                .contains(&format!("`{makefile}:2` — target `test`")),
            "{}",
            make[0].brief
        );
        assert!(
            make[1]
                .brief
                .contains(&format!("`{makefile}:5` — target `lint`")),
            "{}",
            make[1].brief
        );
        // The yarn gaps are there too: four findings in all.
        assert_eq!(toolchain_findings(&report).len(), 4, "{report:#?}");

        fs::remove_file(repo.join("CLAUDE.md")).unwrap();
        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        let f = found
            .iter()
            .find(|f| f.finding.ends_with("`make test`"))
            .expect("the make test gap");
        assert_eq!(
            f.subject,
            Subject::Directory {
                path: repo.to_string_lossy().to_string()
            }
        );
        assert!(
            f.finding
                .ends_with("no CLAUDE.md loads for this repository, so nothing names `make test`"),
            "{}",
            f.finding
        );
    }

    /// Fixture 3: the global `~/.claude/CLAUDE.md` counts. It names
    /// `make test`, the repo file names nothing, and there is no test
    /// gap; the remaining gap's evidence cites the global path among the
    /// files read.
    #[test]
    fn the_global_scope_counts_and_is_cited() {
        let (_t, repo, home) = fixture();
        fs::write(repo.join("Makefile"), "test:\n\ttrue\nlint:\n\ttrue\n").unwrap();
        fs::write(repo.join("CLAUDE.md"), "Nothing here.\n").unwrap();
        let global = home.join(".claude").join("CLAUDE.md");
        fs::write(&global, "Always run `make test`.\n").unwrap();

        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        assert_eq!(found.len(), 1, "{report:#?}");
        assert!(
            found[0].finding.ends_with("names `make lint`"),
            "{}",
            found[0].finding
        );
        assert!(
            !found.iter().any(|f| f.finding.contains("`make test`")),
            "the global file names the test command"
        );
        let searched = found[0]
            .evidence
            .iter()
            .find(|e| e.measured.contains("files read"))
            .expect("the search evidence");
        assert!(
            searched
                .measured
                .contains(&global.to_string_lossy().to_string()),
            "{}",
            searched.measured
        );
        assert!(
            searched.measured.starts_with("2 files read"),
            "{}",
            searched.measured
        );
    }

    /// Fixture 4: a command named in an imported file counts, and when
    /// the import cannot be read the verb is Unknown, never Advice.
    ///
    /// Unix-only: the wall is a permission bit. Under root's DAC override
    /// the bit does nothing; the gate runs the suite under `capsh` for
    /// this reason.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_import_makes_the_negative_unknown_not_advice() {
        use std::os::unix::fs::PermissionsExt;

        let (_t, repo, home) = fixture();
        fs::write(repo.join("Cargo.toml"), "[package]\nname = \"octocat\"\n").unwrap();
        fs::write(repo.join("CLAUDE.md"), "@./tooling.md\n").unwrap();
        let tooling = repo.join("tooling.md");
        fs::write(&tooling, "Run `cargo test` first.\n").unwrap();

        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        assert!(
            !found.iter().any(|f| f.finding.contains("`cargo test`")),
            "the import names the test command: {report:#?}"
        );
        assert!(
            found.iter().all(|f| f.severity == Severity::Advice),
            "{report:#?}"
        );

        fs::set_permissions(&tooling, fs::Permissions::from_mode(0o000)).unwrap();
        let report = run_over(&repo, &home);
        fs::set_permissions(&tooling, fs::Permissions::from_mode(0o644)).unwrap();

        let found = toolchain_findings(&report);
        let test = found
            .iter()
            .find(|f| f.finding.contains("`cargo test`"))
            .expect("the test verb is reported, not silently cleared");
        assert_eq!(test.severity, Severity::Unknown, "{}", test.finding);
        // Compared by suffix: the resolver keeps the `./` the import was
        // written with, and the property under test is WHICH file.
        assert!(
            test.finding.contains("could not be decided")
                && test.finding.contains("tooling.md` not readable"),
            "{}",
            test.finding
        );
        assert!(
            found.iter().all(|f| f.severity == Severity::Unknown),
            "no negative is stated while a loaded file is unreadable: {found:#?}"
        );
        assert!(test.brief.contains("no edit to `"), "{}", test.brief);
    }

    /// Fixture 5: an unreadable `package.json` is Unknown with the io
    /// error as evidence, never "no scripts". The regression guard for
    /// absent-is-not-zero (#846).
    #[cfg(unix)]
    #[test]
    fn an_unreadable_package_json_is_unknown_with_the_io_error() {
        use std::os::unix::fs::PermissionsExt;

        let (_t, repo, home) = fixture();
        yarn_app(&repo);
        fs::write(repo.join("CLAUDE.md"), "Nothing.\n").unwrap();
        let manifest = repo.join("package.json");
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o000)).unwrap();
        let report = run_over(&repo, &home);
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o644)).unwrap();

        let found = toolchain_findings(&report);
        assert_eq!(found.len(), 1, "{report:#?}");
        let f = found[0];
        assert_eq!(f.severity, Severity::Unknown);
        assert!(f.finding.contains("could not be read"), "{}", f.finding);
        assert!(
            f.evidence[0].measured.contains("Permission denied"),
            "{:?}",
            f.evidence
        );
        assert_eq!(
            f.evidence[0].at,
            Locator::File {
                path: manifest.to_string_lossy().to_string(),
                line: None
            }
        );
        assert!(
            !found.iter().any(|f| f.finding.contains(" offers `")),
            "an unreadable manifest offers nothing knowable: {found:#?}"
        );
    }

    /// Fixture 6: prose never counts, a fenced line does, and a manager
    /// token inside a path is not a command.
    #[test]
    fn fenced_counts_prose_does_not_and_a_path_is_not_a_command() {
        let (_t, repo, home) = fixture();
        fs::write(repo.join("Makefile"), "lint:\n\ttrue\n").unwrap();
        fs::write(repo.join("Cargo.toml"), "[package]\nname = \"octocat\"\n").unwrap();

        let gaps = |body: &str| -> Vec<String> {
            fs::write(repo.join("CLAUDE.md"), body).unwrap();
            toolchain_findings(&run_over(&repo, &home))
                .iter()
                .map(|f| f.finding.clone())
                .collect()
        };

        // Prose: the lint gap stands.
        let prose = gaps("Run make lint before pushing.\n");
        assert!(
            prose.iter().any(|s| s.ends_with("names `make lint`")),
            "{prose:?}"
        );

        // Fenced: cleared.
        let fenced = gaps("```bash\nmake lint\n```\n");
        assert!(
            !fenced.iter().any(|s| s.contains("`make lint`")),
            "{fenced:?}"
        );

        // A chained fenced line names each segment's command.
        let chained = gaps("```bash\ncd src-tauri && cargo test --lib\n```\n");
        assert!(
            !chained.iter().any(|s| s.contains("`cargo test`")),
            "{chained:?}"
        );
        assert!(
            chained.iter().any(|s| s.contains("`cargo build`")),
            "{chained:?}"
        );

        // A path whose token is a manager counts for nothing.
        let path = gaps("Edit `src-tauri/Cargo.toml` and `cargo/test`.\n");
        assert!(path.iter().any(|s| s.contains("`cargo test`")), "{path:?}");
        assert!(
            path.iter().any(|s| s.ends_with("names `make lint`")),
            "{path:?}"
        );
    }

    /// Lint leakage: a style line with a formatter config that sets it is
    /// an Advice finding naming the line and the config; the same line
    /// with no config is nothing, and a config that does not decide that
    /// setting is nothing either.
    #[test]
    fn a_style_line_beside_a_formatter_config_is_leakage() {
        let (_t, repo, home) = fixture();
        fs::write(
            repo.join("CLAUDE.md"),
            "# Style\n\nUse 2-space indentation.\nPrefer single quotes.\n",
        )
        .unwrap();

        // No config: nothing.
        let report = run_over(&repo, &home);
        assert!(toolchain_findings(&report).is_empty(), "{report:#?}");

        // `.editorconfig` sets indent, not quotes: one finding.
        fs::write(repo.join(".editorconfig"), "[*]\nindent_size = 2\n").unwrap();
        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        assert_eq!(found.len(), 1, "{report:#?}");
        let f = found[0];
        assert_eq!(f.severity, Severity::Advice);
        assert_eq!(
            f.finding,
            "line 3 states an indent width; `.editorconfig` sets it"
        );
        assert_eq!(
            f.evidence[0].at,
            Locator::File {
                path: repo.join("CLAUDE.md").to_string_lossy().to_string(),
                line: Some(3)
            }
        );
        assert_eq!(f.evidence[0].measured, "Use 2-space indentation.");
        assert!(
            f.brief.contains("Delete the line named above"),
            "{}",
            f.brief
        );

        // Prettier sets quotes too: two findings.
        fs::write(repo.join(".prettierrc"), "{}").unwrap();
        let report = run_over(&repo, &home);
        let sentences: Vec<&str> = toolchain_findings(&report)
            .iter()
            .map(|f| f.finding.as_str())
            .collect();
        assert_eq!(
            sentences,
            vec![
                "line 3 states an indent width; `.editorconfig` and `.prettierrc` sets it",
                "line 4 states a quote style; `.prettierrc` sets it",
            ]
        );
    }

    /// The word "style" alone is not a setting, and a fenced line is not
    /// prose.
    #[test]
    fn leakage_matching_is_narrow() {
        let (_t, repo, home) = fixture();
        fs::write(repo.join(".prettierrc"), "{}").unwrap();
        fs::write(
            repo.join("CLAUDE.md"),
            "Follow the house style.\n```json\n{ \"semi\": false }\n```\nsemicolons\n",
        )
        .unwrap();
        let report = run_over(&repo, &home);
        let sentences: Vec<&str> = toolchain_findings(&report)
            .iter()
            .map(|f| f.finding.as_str())
            .collect();
        assert_eq!(
            sentences,
            vec!["line 5 states a semicolon rule; `.prettierrc` sets it"]
        );
    }

    /// No toolchain under the walk is no finding, and the check ran.
    #[test]
    fn no_toolchain_is_no_finding() {
        let (_t, repo, home) = fixture();
        fs::write(repo.join("CLAUDE.md"), "Nothing.\n").unwrap();
        let report = run_over(&repo, &home);
        assert!(toolchain_findings(&report).is_empty(), "{report:#?}");
        assert_eq!(coverage(&report), CheckRun::Ran { findings: 0 });
    }

    /// The verb map, by name and by subcommand.
    #[test]
    fn verbs_map_by_name_and_unmapped_is_none() {
        assert_eq!(verb_by_name("test-mobile"), Some(Verb::Test));
        assert_eq!(verb_by_name("lint-rust"), Some(Verb::Lint));
        assert_eq!(verb_by_name("fmt"), Some(Verb::Format));
        assert_eq!(verb_by_name("build"), Some(Verb::Build));
        assert_eq!(verb_by_name("dev"), Some(Verb::Run));
        assert_eq!(verb_by_name("release"), Some(Verb::Deploy));
        assert_eq!(verb_by_name("icons"), None);
        assert_eq!(verb_by_name("check-mobile-ios"), None);

        assert_eq!(commands_in("yarn vitest run"), vec![]);
        assert_eq!(
            commands_in("yarn run build"),
            vec![("yarn".into(), Verb::Build)]
        );
        assert_eq!(commands_in("npm test"), vec![("npm".into(), Verb::Test)]);
        assert_eq!(commands_in("npm install"), vec![]);
        assert_eq!(
            commands_in("cargo test --lib"),
            vec![("cargo".into(), Verb::Test)]
        );
        assert_eq!(
            commands_in("./gradlew check"),
            vec![("gradle".into(), Verb::Lint)]
        );
        assert_eq!(
            commands_in("xcodebuild -scheme app test"),
            vec![("xcodebuild".into(), Verb::Test)]
        );
        assert_eq!(
            commands_in("uv run pytest -q"),
            vec![("uv".into(), Verb::Test)]
        );
        assert_eq!(
            commands_in("ruff format ."),
            vec![("ruff".into(), Verb::Format)]
        );
        assert_eq!(
            commands_in("$ make fmt && make test-rust | tee out"),
            vec![("make".into(), Verb::Format), ("make".into(), Verb::Test)]
        );
        assert_eq!(commands_in("src-tauri/Cargo.toml"), vec![]);
    }

    /// Detection over the added markers, each read through this module's
    /// own walk.
    #[test]
    fn added_markers_are_detected_with_their_offers() {
        let (_t, repo, _home) = fixture();
        fs::write(repo.join("go.mod"), "module octocat\n").unwrap();
        fs::write(repo.join("Gemfile"), "source 'https://rubygems.org'\n").unwrap();
        fs::write(repo.join("build.gradle.kts"), "").unwrap();
        fs::write(repo.join("gradlew"), "").unwrap();
        fs::create_dir_all(repo.join("App.xcodeproj")).unwrap();
        fs::write(
            repo.join("pyproject.toml"),
            "[tool.hatch]\n[tool.ruff]\nline-length = 100\n[tool.pytest.ini_options]\n",
        )
        .unwrap();
        fs::write(
            repo.join("justfile"),
            "build:\n  cargo build\nfmt:\n  cargo fmt\n",
        )
        .unwrap();

        let d = detect(&repo);
        assert!(
            d.unreadable.is_empty() && d.unreadable_dirs.is_empty(),
            "{d:#?}"
        );
        let kinds: Vec<Toolchain> = d.toolchains.iter().map(|t| t.toolchain).collect();
        assert_eq!(
            kinds,
            vec![
                Toolchain::Just,
                Toolchain::Go,
                Toolchain::Bundler,
                Toolchain::Gradle,
                Toolchain::Xcode,
                Toolchain::PyprojectUnknown,
            ]
        );
        let by = |k: Toolchain| d.toolchains.iter().find(|t| t.toolchain == k).unwrap();
        assert_eq!(
            by(Toolchain::Just)
                .offers
                .iter()
                .map(|o| (o.verb, o.command.as_str(), o.line))
                .collect::<Vec<_>>(),
            vec![
                (Verb::Build, "just build", Some(1)),
                (Verb::Format, "just fmt", Some(3))
            ]
        );
        assert_eq!(by(Toolchain::Gradle).offers[0].command, "./gradlew build");
        assert_eq!(by(Toolchain::Xcode).label, "App.xcodeproj at root");
        assert_eq!(
            by(Toolchain::PyprojectUnknown)
                .offers
                .iter()
                .map(|o| o.command.as_str())
                .collect::<Vec<_>>(),
            vec!["pytest", "ruff check", "ruff format"]
        );
        assert!(by(Toolchain::Bundler).offers.is_empty());
    }

    /// A Cargo workspace is one toolchain whose label counts its
    /// members, and `cargo run` is offered only where a binary exists.
    #[test]
    fn a_cargo_workspace_is_labelled_and_run_needs_a_binary() {
        let (_t, repo, _home) = fixture();
        fs::write(
            repo.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n",
        )
        .unwrap();
        let d = detect(&repo);
        let cargo = &d.toolchains[0];
        assert_eq!(cargo.label, "Cargo.toml at root (workspace of 2 members)");
        assert!(
            !cargo.offers.iter().any(|o| o.verb == Verb::Run),
            "{cargo:#?}"
        );

        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src").join("main.rs"), "fn main() {}\n").unwrap();
        let d = detect(&repo);
        assert!(d.toolchains[0]
            .offers
            .iter()
            .any(|o| o.command == "cargo run"));
    }

    /// The worked example: this repository's own checkout. Printed so the
    /// PR body can carry a measured figure, and asserted only on what the
    /// sub-issue states about the three CLAUDE.md files.
    #[test]
    fn this_repository_worked_example() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let home = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let report = run_over(&repo, home.path());
        let elapsed = started.elapsed();
        let found = toolchain_findings(&report);
        eprintln!(
            "toolchain over {}: {} findings in {elapsed:?}",
            repo.display(),
            found.len()
        );
        for f in &found {
            eprintln!("  [{:?}] {}", f.severity, f.finding);
        }
        // `make lint`, `make test-mobile`, `cargo fmt` and `cargo test
        // --lib` are named, so none of these is a gap.
        for named in [
            "`make lint`",
            "`make test-mobile`",
            "`cargo fmt`",
            "`cargo test`",
        ] {
            assert!(
                !found.iter().any(|f| f.finding.contains(named)),
                "{named} is named in a CLAUDE.md here: {:?}",
                found.iter().map(|f| &f.finding).collect::<Vec<_>>()
            );
        }
        // `make build` and `make dev` exist and nothing names them.
        for gap in ["`make build`", "`make dev`", "`yarn test`"] {
            assert!(
                found.iter().any(|f| f.finding.contains(gap)),
                "{gap} is a gap here: {:?}",
                found.iter().map(|f| &f.finding).collect::<Vec<_>>()
            );
        }
    }
}
