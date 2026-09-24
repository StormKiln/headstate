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
//! The files searched are every CLAUDE.md the scan loaded, their imports,
//! and the repository's `.claude/rules/*.md` (`claudemd::rules`, #1340),
//! path-scoped or not: a rule loads when a session works where its
//! `paths:` point, as a nested CLAUDE.md does. So are the skills
//! (`claudemd::skill_files`, #1394): every `SKILL.md` under the
//! repository's `.claude/skills` and the user's `~/.claude/skills`, the
//! walk the transcripts producer's "already written" corpus uses (#1370).
//! A gate documented in a skill the root CLAUDE.md points at is
//! documented; plugin skills are out of scope (#1365). The sentence counts
//! the three apart ("none of the 7 files read, 2 rules or 4 skills names
//! …"). A file two CLAUDE.md files both import is one file read, not two
//! (#1350). Docs linked from a CLAUDE.md by an ordinary markdown link are
//! not searched: they are not loaded into a session, so they instruct
//! nothing until read.
//!
//! The target-to-verb map is by name (`test*`, `lint*`, `fmt*|format*`,
//! `build*`, `dev|run|start|serve`, `deploy|release|publish`). A target
//! that maps to nothing is listed as "other" in the evidence and is never
//! counted for or against a verb.
//!
//! A package.json script whose name maps to nothing is mapped by its body
//! instead (#1341: a `verify` script running prettier, eslint and vitest
//! was listed as "other", and format reported undocumented). The body is
//! split at `&&`, `||`, `;` and `|`, leading `NAME=value` assignments are
//! skipped, a launcher is read through, and the tool maps: `prettier` and
//! `biome format` to format, `eslint` and `biome lint` to lint, `vitest`,
//! `jest`, `mocha` and `playwright test` to test, `tsc` and `vite build`
//! to build. A command running another script (`npm run <s>`) maps `<s>`
//! by name, else by its body, one level only. One script can offer
//! several verbs, and a loaded file naming it names all of them.
//!
//! The same tool map reads a binary a loaded file runs through a manager
//! (#1392): `yarn vitest run`, `npx prettier --check .`, `pnpm exec
//! eslint`, `bunx tsc`, `yarn dlx`, `bun x`, and `yarn|pnpm|bun [run]
//! <bin>`. This reverses #1341's decision that the map applies to bodies
//! only ("`npx prettier` in a CLAUDE.md still names nothing"): on this
//! repository `src/CLAUDE.md` names `yarn vitest run`, which is what the
//! `test` script runs, and the test gap it produced was false. A command
//! that runs a tool directly names what that tool does. The command is
//! credited to the manager that ran it (`npx`, `yarn`), so it covers the
//! JS family's verb and no other toolchain's. A script of the same name
//! takes precedence (`yarn test` is the script, and `yarn vitest` runs a
//! script called `vitest` when one exists); `npm run` runs scripts only;
//! a manager's own subcommand (`yarn install`) names nothing.
//!
//! A body that runs a repository script file (`bash <file>`, `sh <file>`,
//! `node <file>`, `./<file>`) is followed into it one level (#1376: a
//! `verify` running `bash tools/verify.sh`, which ran prettier, was
//! reported as naming no format command). The file resolves against the
//! package's directory and must stay under the repository, checked
//! lexically and again through symlinks; one outside it is never read.
//! Its non-comment lines (`#`, `//`) are read with the same tool map, and
//! a file it runs in turn is not followed. Lines are read by their first
//! words, so a tool a `node` file spawns from inside a JS expression is
//! not found. A file that could not be read, or lies outside the
//! repository, makes that script's verbs Unknown, not uncovered: a gap a
//! loaded file naming the script might cover is a [`Severity::Unknown`]
//! finding naming the file, and the script is not listed as "other". A
//! script nothing names changes nothing. A script file is followed from a
//! package.json body only: a make recipe running one (`./scripts/x.sh`)
//! maps by its first word, which names nothing.
//!
//! A make or just target a loaded file names also names what its recipe
//! runs (#1393: `make lint` ran `cargo clippy` through `lint-rust`, and
//! the cargo lint gap was reported anyway). `packages::scripts` reads each
//! target's prerequisites and recipe lines from the file the target was
//! parsed from; the target is followed through its prerequisites and any
//! recipe line calling the same manager (`$(MAKE) <t>`, `make <t>`, `just
//! <t>`), recursively, each target once. `-C`, `-f` and `--justfile` point
//! at another file, which is not followed. Each recipe command is mapped
//! as a loaded file's would be, split at `&&`, `||`, `;` and `|`, with
//! `NAME=value` assignments and shell keywords (`do`, `then`) skipped: `cd
//! a && cargo clippy` is cargo's lint and `yarn vitest run` yarn's test. A
//! makefile variable assigned a literal (`CARGO := cargo`) is read; a
//! command still starting with any other variable reference is not a
//! literal and is skipped. Every verb a command maps to is credited, not
//! only the target's own: on this repository `make lint` runs `lint-ui`,
//! whose `yarn tsc -b` is the tool map's build. A recipe reaching a target
//! the file does not define is reaching a file, unless the makefile is
//! open-ended (`scripts::makefile_is_open_ended`: an `include` or a `%`
//! pattern rule), when that target might be defined where the parser
//! cannot see: the gaps the named target might cover are
//! [`Severity::Unknown`], naming the target, for every toolchain. A target
//! name two makefiles share takes the union of what either runs.
//!
//! A JS launcher is read through to the tool it runs (#1323): `npx`,
//! `bunx`, `pnpm exec`, `bun x` and `pnpm nx` name what follows them, and
//! `pnpm`/`bun` run scripts as `yarn` does. An `nx` command's verbs come
//! from its targets by the same name map: `run web:test`, `web:test`,
//! `nx test web`, or `affected`/`run-many` with `-t lint,test`.
//!
//! # Which named command covers which toolchain
//!
//! A verb is covered for a toolchain when a command naming it is run by
//! one of that toolchain's own managers, with two widenings. `npm` and
//! `yarn` accept the whole JS family (`npm`, `yarn`, `pnpm`, `bun`,
//! `npx`, `bunx`, `nx`), which run the same scripts and tools. `make` and
//! `just` accept any manager: a target fronts some other tool, so once
//! anything names the verb the target is an alternative, not a gap. The
//! widening stops there on purpose: `cargo test` covers `make test`, but
//! never a package.json `test` script, because a Rust suite being named
//! says nothing about whether the JS suite is.
//!
//! A binary a JS manager runs (#1392) is credited to that manager, so
//! `npx prettier` covers the JS family's format and nothing else. What a
//! named target's recipe runs (#1393) is credited to the manager that
//! runs it in the recipe, not to `make`: `make lint` reaching `cargo
//! clippy` covers cargo's lint, as `cargo clippy` named directly would,
//! and make's own lint by name as before.
//!
//! # One finding per verb
//!
//! Each uncovered verb is one finding, listing every toolchain that offers
//! it and nothing covers (#1395): on this repository `make dev` is `yarn
//! tauri dev`, which builds and runs the Rust side, and `yarn dev`, `cargo
//! run` and `make dev` were three findings for one workflow. The sentence
//! names each toolchain with only the members offering the verb ("cargo
//! (Cargo.toml at src-tauri) offers `run`"), then what each offers, then
//! every command nothing names; the evidence cites each offer where it was
//! found. Only the scripts, targets or subcommands for that verb are
//! listed, not everything a toolchain offers (#1376). Whether a toolchain
//! is covered is still decided per toolchain, by the rule above; this
//! changes only how the uncovered ones are grouped. A toolchain whose
//! negative cannot be decided is not folded into the Advice finding: the
//! same verb gets a second, [`Severity::Unknown`], finding for those.
//!
//! `cargo run` is offered only for a crate with a binary target:
//! `src/main.rs`, `src/bin/`, or `[[bin]]` in `Cargo.toml`. A library
//! crate cannot be run.
//!
//! # A negative needs a complete scan
//!
//! A positive ("`make lint` is named at `CLAUDE.md:13`") stands whatever
//! else was unreadable. A negative ("nothing names `make build`") is an
//! [`Severity::Advice`] finding only when every file a session would load
//! was read: no unreadable scope, directory or file in the scan, no
//! unreadable import, no file this producer failed to re-read, no
//! `.claude/rules` directory or rule that exists and could not be read,
//! and no skills directory or `SKILL.md` that exists and could not be
//! read (#1394). Otherwise the verb's finding is [`Severity::Unknown`],
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
use crate::claudemd::rules::{self, Rules};
use crate::claudemd::{skill_files, text, EffectiveScan, ImportNode, Scope};
use crate::packages::detect::projects;
use crate::packages::scripts::{self, Manifest, Target};
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
        let rules = rules::read(cx.repo);
        let skills = skill_files::read(cx.repo, cx.home);
        let search = documented(cx.scan, &detection, &rules, &skills);
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
        unreadable.extend(rules.unreadable.iter().cloned());
        // A walled `.claude/rules` is both a directory the scan could not
        // list and the rules reader's; name it once.
        let unreadable = dedup(unreadable);

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
        let verbs: BTreeSet<Verb> = detection
            .toolchains
            .iter()
            .flat_map(|t| t.offers.iter().map(|o| o.verb))
            .collect();

        // One finding per uncovered verb, listing every toolchain that
        // offers it and nothing covers (#1395). Whether each toolchain is
        // covered is #1327's rule, unchanged. A toolchain whose negative
        // cannot be decided goes in a second, Unknown, finding for the
        // same verb: Advice and Unknown are different states.
        for verb in verbs {
            let mut decided: Vec<Gap> = Vec::new();
            let mut undecided: Vec<Gap> = Vec::new();
            for (kind, members) in &groups {
                if !members
                    .iter()
                    .any(|m| m.offers.iter().any(|o| o.verb == verb))
                {
                    continue;
                }
                let named = search
                    .named
                    .iter()
                    .any(|n| n.verb == verb && kind.covered_by(&n.manager));
                if named {
                    continue;
                }
                // A named script or target whose file could not be read
                // might run this verb: Unknown, never uncovered (#1376,
                // #1393).
                let mut blockers: Vec<&(PathBuf, String)> = Vec::new();
                for u in search
                    .unfollowed
                    .iter()
                    .filter(|u| u.manager.as_deref().is_none_or(|m| kind.covered_by(m)))
                {
                    for r in &u.reasons {
                        if !blockers.contains(&r) {
                            blockers.push(r);
                        }
                    }
                }
                let gap = Gap {
                    kind: *kind,
                    members,
                    blockers,
                };
                if unreadable.is_empty() && gap.blockers.is_empty() {
                    decided.push(gap);
                } else {
                    undecided.push(gap);
                }
            }
            if !decided.is_empty() {
                out.push(gap_finding(verb, &decided, &search, &subject, &[]));
            }
            if !undecided.is_empty() {
                out.push(gap_finding(
                    verb,
                    &undecided,
                    &search,
                    &subject,
                    &unreadable,
                ));
            }
        }

        out.extend(lint_leakage(cx.repo, cx.scan, &detection.formatter_configs));
        Ok(out)
    }
}

/// One toolchain that offers a verb nothing covers.
struct Gap<'a> {
    kind: Toolchain,
    /// Every member of the kind; only those offering the verb are named.
    members: &'a [&'a DetectedToolchain],
    /// What a named command reaches and could not be read.
    blockers: Vec<&'a (PathBuf, String)>,
}

/// The one finding for `verb` over `gaps` (#1395): each toolchain and
/// the members offering it, what each offers, and the commands nothing
/// names. Advice when `unreadable` is empty and no gap has blockers;
/// otherwise Unknown, naming what could not be read.
fn gap_finding(
    verb: Verb,
    gaps: &[Gap],
    search: &Search,
    subject: &Subject,
    unreadable: &[String],
) -> Finding {
    let offers = |g: &Gap| -> Vec<Offer> {
        g.members
            .iter()
            .flat_map(|m| m.offers.iter().filter(|o| o.verb == verb).cloned())
            .collect()
    };
    let mut clauses: Vec<String> = Vec::new();
    let mut candidates: Vec<String> = Vec::new();
    let mut evidence: Vec<Evidence> = Vec::new();
    let mut blockers: Vec<&(PathBuf, String)> = Vec::new();
    for g in gaps {
        // Only the members offering this verb: a library crate is not
        // named in a sentence about `cargo run` (#1395).
        let labels: Vec<&str> = g
            .members
            .iter()
            .filter(|m| m.offers.iter().any(|o| o.verb == verb))
            .map(|m| m.label.as_str())
            .collect();
        // Only the scripts for this verb, not every script (#1376).
        let offered = dedup(offers(g).iter().map(|o| format!("`{}`", o.what)));
        clauses.push(format!(
            "{} ({}) offers {}",
            g.kind.name(),
            labels.join(", "),
            offered.join(", ")
        ));
        for c in offers(g).iter().map(|o| format!("`{}`", o.command)) {
            if !candidates.contains(&c) {
                candidates.push(c);
            }
        }
        evidence.extend(offers(g).into_iter().map(|o| Evidence {
            at: Locator::File {
                path: o.file.to_string_lossy().to_string(),
                line: o.line,
            },
            measured: o.measured,
        }));
        for m in g.members.iter().filter(|m| !m.other.is_empty()) {
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
        for b in &g.blockers {
            if !blockers.contains(b) {
                blockers.push(b);
            }
        }
    }
    evidence.push(Evidence {
        at: Locator::File {
            path: subject.path().to_string(),
            line: None,
        },
        measured: search.measured(),
    });
    for (path, why) in &blockers {
        evidence.push(Evidence {
            at: Locator::File {
                path: path.to_string_lossy().to_string(),
                line: None,
            },
            measured: why.clone(),
        });
    }
    for u in unreadable {
        evidence.push(Evidence {
            at: Locator::File {
                path: u.clone(),
                line: None,
            },
            measured: "not readable".to_string(),
        });
    }

    let offered = and_list(&clauses);
    let (severity, sentence) = if unreadable.is_empty() && blockers.is_empty() {
        (
            Severity::Advice,
            format!("{offered}; {}", search.nothing_names(&candidates)),
        )
    } else if unreadable.is_empty() {
        (
            Severity::Unknown,
            format!(
                "{offered}; whether any loaded file names {} could not be decided: {}",
                or_list(&candidates),
                blockers
                    .iter()
                    .map(|(_, why)| why.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        )
    } else {
        (
            Severity::Unknown,
            format!(
                "{offered}; whether any loaded file names {} could not be decided: {} not \
                 readable",
                or_list(&candidates),
                unreadable
                    .iter()
                    .map(|u| format!("`{u}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    };
    Finding::new(
        Check::Toolchain,
        severity,
        subject.clone(),
        evidence,
        sentence,
    )
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
    joined(items, "or")
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    joined(items, "and")
}

fn joined(items: &[String], word: &str) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        n => format!("{} {word} {}", items[..n - 1].join(", "), items[n - 1]),
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
        // All three run a package.json script without `run`.
        "yarn" | "pnpm" | "bun" => match first {
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

/// The package.json script a JS command runs, when it runs one: `npm run
/// <s>`, `npm test`, `yarn <s>`, `pnpm run <s>`, `bun run <s>`.
fn script_run<'a>(manager: &str, args: &[&'a str]) -> Option<&'a str> {
    let first = args.first().copied();
    match manager {
        "yarn" | "pnpm" | "bun" => match first {
            Some("run") => args.get(1).copied(),
            s => s,
        },
        "npm" => match first {
            Some("run" | "run-script") => args.get(1).copied(),
            s @ Some("test" | "start") => s,
            _ => None,
        },
        _ => None,
    }
}

/// The verb a JS tool names: run from a script body (#1341), or run
/// directly through a manager in a loaded file (#1392).
fn js_tool_verb(tool: &str, args: &[&str]) -> Option<Verb> {
    let first = args.first().copied();
    match tool {
        "prettier" => Some(Verb::Format),
        "eslint" => Some(Verb::Lint),
        "biome" => match first {
            Some("lint") => Some(Verb::Lint),
            Some("format") => Some(Verb::Format),
            _ => None,
        },
        "vitest" | "jest" | "mocha" => Some(Verb::Test),
        "playwright" if first == Some("test") => Some(Verb::Test),
        "tsc" => Some(Verb::Build),
        "vite" if first == Some("build") => Some(Verb::Build),
        _ => None,
    }
}

/// The verb a manager running a JS binary names (#1392): `npx <bin>`,
/// `bunx <bin>`, `pnpm exec <bin>`, `yarn dlx <bin>` and `bun x <bin>`
/// (already read through by [`unwrap_launcher`], so `tool` differs from
/// `launcher`), or `yarn|pnpm|bun [run] <bin>` where `<bin>` is not a
/// script. A script of that name takes precedence (`is_script`), and so
/// does a name the script map already reads (`yarn test` is the script).
/// `npm run` runs scripts only, never a binary.
fn js_binary_verb(
    launcher: &str,
    tool: &str,
    args: &[&str],
    is_script: &dyn Fn(&str) -> bool,
) -> Option<Verb> {
    if tool != launcher {
        return js_tool_verb(tool, args);
    }
    if !matches!(tool, "yarn" | "pnpm" | "bun") {
        return None;
    }
    let (bin, rest) = match args {
        ["run", bin, rest @ ..] | [bin, rest @ ..] => (*bin, rest),
        [] => return None,
    };
    if bin == "run" || is_script(bin) || verb_by_name(bin).is_some() {
        return None;
    }
    js_tool_verb(bin, rest)
}

/// What a package.json script's body runs: its verbs, and the script
/// files it runs that could not be read or lie outside the repository,
/// each with the path and what was measured (#1376).
#[derive(Debug, Default)]
struct Body {
    verbs: Vec<Verb>,
    unknown: Vec<(PathBuf, String)>,
}

/// Where a script body runs: the package's directory, which a relative
/// script file resolves against, and the repository it must stay under.
struct Files<'a> {
    dir: &'a Path,
    repo: &'a Path,
}

/// The verbs a package.json script's body runs (#1341), in order, each
/// once. The body is split at `&&`, `||`, `;` and `|`; leading `NAME=value`
/// assignments are skipped and a launcher is read through
/// ([`unwrap_launcher`]). A command running another script maps it by name,
/// else, when `follow` is set, by that script's own body with `follow`
/// cleared: one level, so a cycle cannot loop and a chain is not chased.
///
/// With `files`, a command running a repository script file (`bash|sh
/// <file>`, `node <file>`, `./<file>`) maps by that file's non-comment
/// lines, read with `files` cleared: one level (#1376). A file that could
/// not be read, or lies outside the repository, is in `unknown`.
fn body_verbs(
    body: &str,
    bodies: &BTreeMap<&str, &str>,
    follow: bool,
    files: Option<&Files>,
) -> Body {
    let mut out = Body::default();
    for segment in split_chain(body) {
        let tokens: Vec<&str> = segment
            .split_whitespace()
            .skip_while(|t| is_assignment(t))
            .collect();
        let Some(first) = tokens.first() else {
            continue;
        };
        let (tool, args) = unwrap_launcher(first, &tokens[1..]);
        let found: Vec<Verb> = if let Some(v) = js_tool_verb(tool, args) {
            vec![v]
        } else if tool == "nx" {
            nx_verbs(args)
        } else if let Some(s) = script_run(tool, args) {
            match verb_by_name(s) {
                Some(v) => vec![v],
                None => match bodies.get(s) {
                    Some(b) if follow => {
                        let inner = body_verbs(b, bodies, false, files);
                        out.unknown.extend(inner.unknown);
                        inner.verbs
                    }
                    Some(_) => Vec::new(),
                    // Not a script: a binary the manager runs (#1392).
                    None => js_binary_verb(first, tool, args, &|n| bodies.contains_key(n))
                        .into_iter()
                        .collect(),
                },
            }
        } else if let (Some(at), Some(file)) = (files, script_file(tool, args)) {
            match read_script_file(file, at) {
                Ok(text) => {
                    let mut verbs = Vec::new();
                    for line in text.replace("\r\n", "\n").split('\n') {
                        let t = line.trim_start();
                        if t.is_empty() || t.starts_with('#') || t.starts_with("//") {
                            continue;
                        }
                        verbs.extend(body_verbs(line, bodies, false, None).verbs);
                    }
                    verbs
                }
                Err(unknown) => {
                    out.unknown.push(unknown);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        for v in found {
            if !out.verbs.contains(&v) {
                out.verbs.push(v);
            }
        }
    }
    out
}

/// The repository script file a command runs, as written: the first
/// non-flag argument of `bash`, `sh` or `node`, or a `./` command. `bash
/// -c` runs a string, not a file.
fn script_file<'a>(tool: &'a str, args: &[&'a str]) -> Option<&'a str> {
    let file = match tool {
        "bash" | "sh" if args.contains(&"-c") => None,
        "bash" | "sh" | "node" => args.iter().copied().find(|a| !a.starts_with('-')),
        t if t.len() > 2 && t.starts_with("./") => Some(t),
        _ => None,
    }?;
    let file = file.trim_matches(['"', '\'']);
    (!file.is_empty()).then_some(file)
}

/// Lexical normalisation: `.` dropped, `..` popped.
fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// A script file's text, when it is under the repository and readable.
/// Otherwise the path and what was measured: a file outside the
/// repository is not read at all, which is said as such, never as a
/// read that failed. Checked lexically first, then through symlinks.
fn read_script_file(file: &str, at: &Files) -> Result<String, (PathBuf, String)> {
    let path = normalise(&at.dir.join(file));
    let outside = |p: &Path| {
        (
            p.to_path_buf(),
            format!("`{file}` is outside the repository, so it was not read"),
        )
    };
    if Path::new(file).is_absolute() || !path.starts_with(normalise(at.repo)) {
        return Err(outside(&path));
    }
    let unreadable = |e: std::io::Error| (path.clone(), format!("`{file}` could not be read: {e}"));
    let real = std::fs::canonicalize(&path).map_err(unreadable)?;
    let root = std::fs::canonicalize(at.repo).map_err(unreadable)?;
    if !real.starts_with(&root) {
        return Err(outside(&path));
    }
    std::fs::read_to_string(&real).map_err(unreadable)
}

/// `NAME=value` before a command: an environment assignment.
fn is_assignment(token: &str) -> bool {
    match token.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
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

    /// Whether a command named with `manager` counts for this toolchain's
    /// verb. Its own managers always do. A JS toolchain also takes any of
    /// [`JS_FAMILY`], since they all run the same package.json scripts and
    /// the same tools. A task runner (`make`, `just`) takes any manager:
    /// its targets front other tools, so once any command names the verb
    /// the target is an alternative, not a gap (#1323).
    fn covered_by(self, manager: &str) -> bool {
        match self {
            Toolchain::Make | Toolchain::Just => true,
            Toolchain::Ecosystem(Ecosystem::Npm | Ecosystem::Yarn) => JS_FAMILY.contains(&manager),
            _ => self.managers().contains(&manager),
        }
    }
}

/// The first tokens (after [`unwrap_launcher`]) that run a package.json
/// script or a JS workspace tool.
const JS_FAMILY: &[&str] = &["npm", "yarn", "pnpm", "bun", "npx", "bunx", "nx"];

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
    /// The verbs a package.json script named for no verb runs, by
    /// script name, so a loaded file naming `npm run verify` names what
    /// `verify` runs (#1341). A name two manifests share takes the union
    /// of what either runs.
    pub script_verbs: BTreeMap<String, Vec<Verb>>,
    /// The script files a package.json script named for no verb runs and
    /// that could not be read or lie outside the repository, by script
    /// name, each with the file and what was measured (#1376). A loaded
    /// file naming such a script names verbs nobody could read, so a gap
    /// it might cover is Unknown.
    pub script_unknown: BTreeMap<String, Vec<(PathBuf, String)>>,
    /// Every package.json script name, in any manifest: `yarn <name>`
    /// runs the script, not a binary of that name (#1392).
    pub script_names: BTreeSet<String>,
    /// What each make or just target's recipe runs, followed through its
    /// prerequisites, by (manager, target), so a loaded file naming
    /// `make lint` names what `lint` runs (#1393). A name two makefiles
    /// share takes the union.
    pub target_runs: TargetRuns,
    /// What a target's recipe reaches and could not be read, by
    /// (manager, target): a target an open-ended makefile might define,
    /// or a script whose file could not be read (#1393).
    pub target_unknown: TargetUnknown,
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
                    match scripts::script_bodies(&dir) {
                        Manifest::Present(list) => {
                            let bodies: BTreeMap<&str, &str> = list
                                .iter()
                                .map(|s| (s.name.as_str(), s.body.as_str()))
                                .collect();
                            for s in &list {
                                let name = &s.name;
                                out.script_names.insert(name.clone());
                                let command = if *eco == Ecosystem::Yarn {
                                    format!("yarn {name}")
                                } else {
                                    format!("npm run {name}")
                                };
                                // By name first; by body only when the
                                // name maps to nothing (#1341).
                                let mut unknown = false;
                                let (verbs, measured) = match verb_by_name(name) {
                                    Some(verb) => (vec![verb], format!("script `{name}`")),
                                    None => {
                                        let at = Files { dir: &dir, repo };
                                        let body = body_verbs(&s.body, &bodies, true, Some(&at));
                                        if !body.unknown.is_empty() {
                                            unknown = true;
                                            let known =
                                                out.script_unknown.entry(name.clone()).or_default();
                                            for (path, why) in body.unknown {
                                                let why = format!("script `{name}` runs {why}");
                                                if !known.iter().any(|(_, w)| *w == why) {
                                                    known.push((path, why));
                                                }
                                            }
                                        }
                                        let verbs = body.verbs;
                                        if !verbs.is_empty() {
                                            let known =
                                                out.script_verbs.entry(name.clone()).or_default();
                                            for v in &verbs {
                                                if !known.contains(v) {
                                                    known.push(*v);
                                                }
                                            }
                                        }
                                        (
                                            verbs,
                                            format!(
                                                "script `{name}` runs `{}`",
                                                clamp(s.body.trim(), 80)
                                            ),
                                        )
                                    }
                                };
                                // A script whose file could not be read is
                                // Unknown, not "not mapped" (#1376).
                                if verbs.is_empty() && !unknown {
                                    t.other.push(name.clone());
                                }
                                for verb in verbs {
                                    t.offers.push(Offer {
                                        verb,
                                        command: command.clone(),
                                        what: name.clone(),
                                        file: t.manifest.clone(),
                                        line: None,
                                        measured: measured.clone(),
                                    });
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
                    // `cargo run` only where there is a binary to run:
                    // `[[bin]]`, `src/main.rs` or `src/bin/` (#1395). A
                    // library crate offers none.
                    let src = dir.join("src");
                    let has_bin = doc.get("bin").is_some()
                        || names_of
                            .get(src.as_path())
                            .is_some_and(|ns| ns.iter().any(|n| n == "main.rs"))
                        || src.join("bin").is_dir();
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
                    // What each target's recipe runs (#1393), read
                    // from the file the target was parsed from.
                    let open = if kind == Toolchain::Make {
                        scripts::makefile_is_open_ended(dir)
                    } else {
                        None
                    };
                    let no_targets = BTreeMap::new();
                    let no_unknown = BTreeMap::new();
                    let cx = Scripts {
                        verbs: &out.script_verbs,
                        unknown: &out.script_unknown,
                        names: &out.script_names,
                        targets: &no_targets,
                        target_unknown: &no_unknown,
                    };
                    let found: Vec<_> = mine
                        .iter()
                        .map(|target| {
                            let (runs, unknown) =
                                recipe_runs(manager, &target.name, &mine, &t.manifest, open, &cx);
                            (target.name.clone(), runs, unknown)
                        })
                        .collect();
                    for (name, runs, unknown) in found {
                        let key = (manager.to_string(), name);
                        if !runs.is_empty() {
                            let known = out.target_runs.entry(key.clone()).or_default();
                            for r in runs {
                                if !known.contains(&r) {
                                    known.push(r);
                                }
                            }
                        }
                        if !unknown.is_empty() {
                            let known = out.target_unknown.entry(key).or_default();
                            for u in unknown {
                                if !known.contains(&u) {
                                    known.push(u);
                                }
                            }
                        }
                    }
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
    /// The first token, normalised: `./gradlew` is `gradle`, and a
    /// launcher is read through, so `npx nx run web:test` is `nx`.
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
    /// Every `.claude/rules` file read (#1340), counted apart from
    /// `files` so the sentence can say which is which.
    pub rules: Vec<PathBuf>,
    /// Every `SKILL.md` read (#1394), counted apart from both.
    pub skills: Vec<PathBuf>,
    pub spans: usize,
    pub fenced_lines: usize,
    /// Files the scan listed and this producer could not re-read.
    pub unreadable: Vec<String>,
    /// Named scripts whose script file could not be read (#1376).
    pub unfollowed: Vec<Unfollowed>,
}

/// A command a loaded file names that runs something this producer could
/// not read: a package.json script whose script file could not be read or
/// lies outside the repository (#1376), or a make target whose recipe
/// reaches a target an open-ended makefile might define (#1393).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unfollowed {
    /// The manager whose toolchains the unread part could cover; `None`
    /// when it could be any (a target the parser cannot see).
    pub manager: Option<String>,
    pub reasons: Vec<(PathBuf, String)>,
}

impl Search {
    /// "none of the 7 files read, 2 rules or 4 skills names `x`": what was
    /// searched, each kind counted apart (#1340, #1394), and a kind with
    /// none left out.
    fn nothing_names(&self, candidates: &[String]) -> String {
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let mut others: Vec<String> = Vec::new();
        if !self.rules.is_empty() {
            let r = self.rules.len();
            others.push(format!("{r} rule{}", plural(r)));
        }
        if !self.skills.is_empty() {
            let k = self.skills.len();
            others.push(format!("{k} skill{}", plural(k)));
        }
        let named = or_list(candidates);
        match self.files.len() {
            0 if others.is_empty() => {
                format!("no CLAUDE.md loads for this repository, so nothing names {named}")
            }
            0 => format!(
                "no CLAUDE.md loads for this repository, and none of the {} names {named}",
                or_list(&others)
            ),
            n => {
                let mut all = vec![format!("{n} file{} read", plural(n))];
                all.extend(others);
                format!("none of the {} names {named}", or_list(&all))
            }
        }
    }

    /// The count and how it was counted, for the evidence.
    fn measured(&self) -> String {
        let listed = |what: &str, paths: &[PathBuf]| {
            let n = paths.len();
            let list = paths
                .iter()
                .map(|f| format!("`{}`", f.to_string_lossy()))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{n} {what}{} read{}",
                if n == 1 { "" } else { "s" },
                if n == 0 {
                    String::new()
                } else {
                    format!(" ({list})")
                }
            )
        };
        let mut rules = String::new();
        if !self.rules.is_empty() {
            rules.push_str(&format!(", {}", listed("rule", &self.rules)));
        }
        if !self.skills.is_empty() {
            rules.push_str(&format!(", {}", listed("skill", &self.skills)));
        }
        format!(
            "{}{rules}, {} span{} and {} fenced line{} searched",
            listed("file", &self.files),
            self.spans,
            if self.spans == 1 { "" } else { "s" },
            self.fenced_lines,
            if self.fenced_lines == 1 { "" } else { "s" },
        )
    }
}

/// Every file a session loads from this scan: the CLAUDE.md files and
/// every import that resolved and read, each once, in walk order. An
/// import the resolver could not read is not listed; it is already an
/// unreadable path.
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
    // By path, keeping the first: a file two CLAUDE.md files import is
    // not consecutive in walk order, so `Vec::dedup` kept both (#1350).
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

/// What the loaded files, the repository's rules and the skills name,
/// through `text::spans` and `text::fences`. A rule the reader could not
/// read is already in `rules.unreadable`, which the caller counts; a
/// skill that could not be read, or a skills directory that could not be
/// listed, goes in [`Search::unreadable`] (#1394).
pub fn documented(
    scan: &EffectiveScan,
    detection: &Detection,
    rules: &Rules,
    skills: &skill_files::Skills,
) -> Search {
    let scripts = Scripts {
        verbs: &detection.script_verbs,
        unknown: &detection.script_unknown,
        names: &detection.script_names,
        targets: &detection.target_runs,
        target_unknown: &detection.target_unknown,
    };
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
        search_text(&mut out, &path, &text, &scripts);
        out.files.push(path);
    }
    for rule in &rules.files {
        search_text(&mut out, &rule.path, &rule.text, &scripts);
        out.rules.push(rule.path.clone());
    }
    for (dir, e) in &skills.unreadable {
        out.unreadable.push(format!("{dir} ({e})"));
    }
    for (path, _) in &skills.files {
        let path = PathBuf::from(path);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                search_text(&mut out, &path, &text, &scripts);
                out.skills.push(path);
            }
            Err(e) => out
                .unreadable
                .push(format!("{} ({e})", path.to_string_lossy())),
        }
    }
    out
}

/// What detection learned about package.json scripts, for the search.
struct Scripts<'a> {
    verbs: &'a BTreeMap<String, Vec<Verb>>,
    unknown: &'a BTreeMap<String, Vec<(PathBuf, String)>>,
    /// Every script name, so `yarn <name>` is read as the script (#1392).
    names: &'a BTreeSet<String>,
    /// What a make or just target's recipe runs, by (manager, target)
    /// (#1393).
    targets: &'a TargetRuns,
    /// What a target's recipe reaches and could not be read (#1393).
    target_unknown: &'a TargetUnknown,
}

/// What each make or just target's recipe runs, by (manager, target).
pub type TargetRuns = BTreeMap<(String, String), Vec<(String, Verb)>>;
/// What each target's recipe reaches and could not be read.
pub type TargetUnknown = BTreeMap<(String, String), Vec<Unfollowed>>;

impl Scripts<'_> {
    /// The named scripts in one span or fenced line whose script file
    /// could not be read, into `out`.
    fn unfollowed(&self, text: &str, out: &mut Vec<Unfollowed>) {
        for (manager, script) in scripts_named(text) {
            if let Some(reasons) = self.unknown.get(&script) {
                out.push(Unfollowed {
                    manager: Some(manager),
                    reasons: reasons.clone(),
                });
            }
        }
        for (manager, target) in targets_named(text) {
            if let Some(unknown) = self.target_unknown.get(&(manager, target)) {
                out.extend(unknown.iter().cloned());
            }
        }
    }
}

/// One file's spans and fenced lines into `out`.
fn search_text(out: &mut Search, path: &Path, text: &str, scripts: &Scripts) {
    for s in text::spans(text) {
        out.spans += 1;
        scripts.unfollowed(&s.text, &mut out.unfollowed);
        for (manager, verb) in commands_with(&s.text, scripts) {
            out.named.push(Named {
                manager,
                verb,
                file: path.to_path_buf(),
                line: u32::try_from(s.line).unwrap_or(u32::MAX),
                text: s.text.clone(),
            });
        }
    }
    for f in text::fences(text) {
        for (i, line) in f.body.split('\n').enumerate() {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            out.fenced_lines += 1;
            scripts.unfollowed(line, &mut out.unfollowed);
            for (manager, verb) in commands_with(line, scripts) {
                out.named.push(Named {
                    manager,
                    verb,
                    file: path.to_path_buf(),
                    line: u32::try_from(f.line + 1 + i).unwrap_or(u32::MAX),
                    text: line.to_string(),
                });
            }
        }
    }
}

/// [`commands_with`] and no scripts.
#[cfg(test)]
fn commands_in(text: &str) -> Vec<(String, Verb)> {
    commands_with(
        text,
        &Scripts {
            verbs: &BTreeMap::new(),
            unknown: &BTreeMap::new(),
            names: &BTreeSet::new(),
            targets: &BTreeMap::new(),
            target_unknown: &BTreeMap::new(),
        },
    )
}

/// The verbs one span or one fenced line names. A line can chain
/// commands (`cd src-tauri && cargo test --lib`), so each segment is read
/// on its own; a `$ ` prompt is stripped. A script whose name maps to no
/// verb names what its body runs, from `scripts.verbs` (#1341). A binary
/// a JS manager runs names what the tool map says, credited to that
/// manager (#1392).
fn commands_with(text: &str, scripts: &Scripts) -> Vec<(String, Verb)> {
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
        let launcher = manager;
        let (manager, args) = unwrap_launcher(manager, &tokens[1..]);
        if manager == "nx" {
            out.extend(nx_verbs(args).into_iter().map(|v| ("nx".to_string(), v)));
        } else if let Some(verb) = verb_of(manager, args) {
            out.push((manager.to_string(), verb));
        } else if let Some(verbs) = script_run(manager, args).and_then(|s| scripts.verbs.get(s)) {
            out.extend(verbs.iter().map(|v| (manager.to_string(), *v)));
        } else if let Some(verb) =
            js_binary_verb(launcher, manager, args, &|n| scripts.names.contains(n))
        {
            out.push((launcher.to_string(), verb));
        }
        // A named target names what its recipe runs, too (#1393).
        if matches!(manager, "make" | "just") {
            for target in target_args(args) {
                if let Some(runs) = scripts
                    .targets
                    .get(&(manager.to_string(), target.to_string()))
                {
                    out.extend(runs.iter().cloned());
                }
            }
        }
    }
    out
}

/// The targets a `make` or `just` command runs: its positional
/// arguments, less flags, their values and `NAME=value` assignments.
/// Nothing when a flag points it at another file or directory (`-C`,
/// `-f`, `--justfile`): those are not this makefile's targets.
fn target_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    const ELSEWHERE: &[&str] = &[
        "-C",
        "-f",
        "--file",
        "--makefile",
        "--directory",
        "--justfile",
        "--working-directory",
        "-d",
    ];
    if args.iter().any(|a| {
        ELSEWHERE.contains(a)
            || ELSEWHERE
                .iter()
                .any(|f| f.starts_with("--") && a.starts_with(&format!("{f}=")))
            || (a.starts_with("-C") && a.len() > 2)
    }) {
        return Vec::new();
    }
    args.iter()
        .copied()
        .filter(|a| !a.starts_with('-') && !is_assignment(a))
        .collect()
}

/// The make and just targets one span or fenced line runs, with the
/// manager running each: the same segments [`commands_with`] reads.
fn targets_named(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for segment in split_chain(text) {
        let segment = segment.trim();
        let segment = segment.strip_prefix("$ ").unwrap_or(segment);
        let tokens: Vec<&str> = segment.split_whitespace().collect();
        if let Some(manager @ ("make" | "just")) = tokens.first().copied() {
            for t in target_args(&tokens[1..]) {
                out.push((manager.to_string(), t.to_string()));
            }
        }
    }
    out
}

/// What running `name` runs, through its prerequisites and its recipe,
/// each target once (#1393). `targets` are the ones parsed from the one
/// makefile or justfile at `manifest`; a recipe line calling the same
/// manager (`$(MAKE) x`, `just x`) follows `x` in that file, and one
/// pointed at another file is mapped by name only. Each recipe command
/// is mapped with [`commands_with`], split at `&&`, `||`, `;` and `|`,
/// with shell keywords (`do`, `then`) and `NAME=value` assignments
/// skipped. A command that still starts with a variable reference is
/// not a literal and is skipped.
///
/// A target the file does not define is a file prerequisite, unless the
/// makefile is open-ended (`open`): then it might be defined where the
/// parser cannot see, and it is Unknown, never nothing.
fn recipe_runs(
    manager: &str,
    name: &str,
    targets: &[&Target],
    manifest: &Path,
    open: Option<&str>,
    scripts: &Scripts,
) -> (Vec<(String, Verb)>, Vec<Unfollowed>) {
    struct Walk<'a> {
        manager: &'a str,
        targets: &'a [&'a Target],
        manifest: &'a Path,
        open: Option<&'a str>,
        scripts: &'a Scripts<'a>,
        seen: BTreeSet<String>,
        runs: Vec<(String, Verb)>,
        unknown: Vec<Unfollowed>,
    }
    fn visit(w: &mut Walk, name: &str) {
        if !w.seen.insert(name.to_string()) {
            return;
        }
        let Some(target) = w.targets.iter().find(|t| t.name == name) else {
            if let Some(why) = w.open {
                w.unknown.push(Unfollowed {
                    manager: None,
                    reasons: vec![(
                        w.manifest.to_path_buf(),
                        format!(
                            "target `{name}` is not one the parser can see, and `{}` {why}",
                            w.manifest.to_string_lossy()
                        ),
                    )],
                });
            }
            return;
        };
        for p in &target.prereqs {
            visit(w, p);
        }
        for line in &target.recipe {
            for segment in split_chain(line) {
                let tokens: Vec<&str> = segment
                    .split_whitespace()
                    .skip_while(|t| {
                        matches!(*t, "do" | "then" | "else" | "{" | "(") || is_assignment(t)
                    })
                    .collect();
                let Some(head) = tokens.first() else {
                    continue;
                };
                if head.starts_with('$') {
                    continue;
                }
                if *head == w.manager {
                    for t in target_args(&tokens[1..]) {
                        visit(w, t);
                    }
                }
                let command = tokens.join(" ");
                for run in commands_with(&command, w.scripts) {
                    if !w.runs.contains(&run) {
                        w.runs.push(run);
                    }
                }
                w.scripts.unfollowed(&command, &mut w.unknown);
            }
        }
    }
    let mut w = Walk {
        manager,
        targets,
        manifest,
        open,
        scripts,
        seen: BTreeSet::new(),
        runs: Vec::new(),
        unknown: Vec::new(),
    };
    visit(&mut w, name);
    (w.runs, w.unknown)
}

/// The package.json scripts one span or fenced line runs, with the
/// manager running each: the same segments [`commands_with`] reads.
fn scripts_named(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for segment in split_chain(text) {
        let segment = segment.trim();
        let segment = segment.strip_prefix("$ ").unwrap_or(segment);
        let tokens: Vec<&str> = segment.split_whitespace().collect();
        let Some(first) = tokens.first() else {
            continue;
        };
        let (manager, args) = unwrap_launcher(first, &tokens[1..]);
        if let Some(s) = script_run(manager, args) {
            out.push((manager.to_string(), s.to_string()));
        }
    }
    out
}

/// The tool a JS launcher runs, and its arguments: `npx nx …`, `bunx nx
/// …`, `pnpm exec nx …`, `yarn dlx nx …`, `bun x nx …` and `pnpm nx …`
/// all run `nx`. Launcher flags (`-y`, `--`) before the tool are skipped
/// and an `@version` suffix is dropped. Anything else comes back as it
/// went in.
fn unwrap_launcher<'a>(manager: &'a str, args: &'a [&'a str]) -> (&'a str, &'a [&'a str]) {
    let rest: &[&str] = match (manager, args.first().copied()) {
        ("npx" | "bunx", _) => args,
        ("pnpm" | "yarn" | "npm", Some("exec" | "dlx")) | ("bun", Some("x")) => &args[1..],
        ("pnpm" | "yarn" | "bun", Some("nx")) => args,
        _ => return (manager, args),
    };
    match rest.iter().position(|a| !a.starts_with('-')) {
        Some(i) => {
            let tool = rest[i];
            let tool = match tool.rfind('@') {
                Some(at) if at > 0 => &tool[..at],
                _ => tool,
            };
            (tool, &rest[i + 1..])
        }
        None => ("", &[]),
    }
}

/// The verbs an `nx` command names, from its targets mapped by
/// [`verb_by_name`]: `-t`/`--target`/`--targets` (comma- or
/// space-separated, as `nx affected` and `nx run-many` take them), else
/// `run <project>:<target>`, `<project>:<target>`, the legacy
/// `affected:<target>`, `format:write`, or `nx <target> <project>`.
fn nx_verbs(args: &[&str]) -> Vec<Verb> {
    let mut targets: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if let Some(v) = ["--targets=", "--target=", "-t="]
            .iter()
            .find_map(|p| a.strip_prefix(p))
        {
            targets.extend(v.split(','));
        } else if matches!(a, "-t" | "--target" | "--targets") {
            while let Some(v) = args.get(i + 1).filter(|v| !v.starts_with('-')) {
                targets.extend(v.split(','));
                i += 1;
            }
        }
        i += 1;
    }
    if targets.is_empty() {
        let mut positional = args.iter().copied().filter(|a| !a.starts_with('-'));
        match positional.next() {
            Some("run") => {
                if let Some(t) = positional.next().and_then(|p| p.split(':').nth(1)) {
                    targets.push(t);
                }
            }
            // Both take their targets through `-t` only.
            Some("affected" | "run-many") | None => {}
            Some(p) => match p.split_once(':') {
                Some(("affected", t)) => targets.push(t),
                Some(("format", _)) | None => targets.push(p),
                Some((_project, t)) => targets.push(t.split(':').next().unwrap_or(t)),
            },
        }
    }
    let mut verbs = Vec::new();
    for v in targets.into_iter().filter_map(verb_by_name) {
        if !verbs.contains(&v) {
            verbs.push(v);
        }
    }
    verbs
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
            "yarn (package.json + yarn.lock at root) offers `test`; none of the 1 file read \
             names `yarn test`"
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
        let found = toolchain_findings(&report);
        let sentences: Vec<&str> = found.iter().map(|f| f.finding.as_str()).collect();
        // One finding per verb, each listing yarn's script and make's
        // target (#1395).
        assert_eq!(
            sentences,
            vec![
                "yarn (package.json + yarn.lock at root) offers `test` and make (Makefile at \
                 root) offers `test`; none of the 1 file read names `yarn test` or `make test`",
                "yarn (package.json + yarn.lock at root) offers `lint` and make (Makefile at \
                 root) offers `lint`; none of the 1 file read names `yarn lint` or `make lint`",
            ]
        );
        let makefile = repo.join("Makefile").to_string_lossy().to_string();
        assert!(
            found[0]
                .brief
                .contains(&format!("`{makefile}:2` — target `test`")),
            "{}",
            found[0].brief
        );
        assert!(
            found[1]
                .brief
                .contains(&format!("`{makefile}:5` — target `lint`")),
            "{}",
            found[1].brief
        );

        fs::remove_file(repo.join("CLAUDE.md")).unwrap();
        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        let f = found
            .iter()
            .find(|f| f.finding.contains("`make test`"))
            .expect("the make test gap");
        assert_eq!(
            f.subject,
            Subject::Directory {
                path: repo.to_string_lossy().to_string()
            }
        );
        assert!(
            f.finding.ends_with(
                "no CLAUDE.md loads for this repository, so nothing names `yarn test` or \
                 `make test`"
            ),
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

    /// #1350: a file imported by two CLAUDE.md files is read once and
    /// counted once. The walk order is root, its import, the nested file,
    /// its import, so the repeat is not consecutive and `Vec::dedup`
    /// kept it.
    #[test]
    fn a_shared_import_is_counted_once() {
        let (_t, repo, home) = fixture();
        fs::write(repo.join("Makefile"), "test:\n\ttrue\n").unwrap();
        let shared = repo.join("shared.md");
        fs::write(&shared, "Be careful.\n").unwrap();
        let at = format!("@{}\n", shared.to_string_lossy());
        fs::write(repo.join("CLAUDE.md"), &at).unwrap();
        let sub = repo.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("CLAUDE.md"), &at).unwrap();

        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        assert_eq!(found.len(), 1, "{report:#?}");
        assert_eq!(
            found[0].finding,
            "make (Makefile at root) offers `test`; none of the 3 files read names `make test`"
        );
        let searched = found[0]
            .evidence
            .iter()
            .find(|e| e.measured.contains("files read"))
            .expect("the search evidence");
        assert!(
            searched.measured.starts_with("3 files read"),
            "{}",
            searched.measured
        );
        let shared = shared.to_string_lossy().to_string();
        assert_eq!(
            searched.measured.matches(shared.as_str()).count(),
            1,
            "{}",
            searched.measured
        );
    }

    /// #1340's test: a `make test` named only in `.claude/rules/testing.md`
    /// is not a gap, and the gap that remains counts the rules it read.
    #[test]
    fn a_command_named_in_a_rule_counts_and_the_rules_are_counted() {
        let (_t, repo, home) = fixture();
        fs::write(repo.join("Makefile"), "test:\n\ttrue\nlint:\n\ttrue\n").unwrap();
        fs::write(repo.join("CLAUDE.md"), "Nothing here.\n").unwrap();
        let rules = repo.join(".claude").join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("testing.md"), "Run `make test`.\n").unwrap();
        fs::write(
            rules.join("style.md"),
            "---\npaths: src/**\n---\nBe terse.\n",
        )
        .unwrap();

        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        assert_eq!(found.len(), 1, "{report:#?}");
        assert_eq!(found[0].severity, Severity::Advice);
        assert_eq!(
            found[0].finding,
            "make (Makefile at root) offers `lint`; none of the 1 file read or 2 rules names \
             `make lint`"
        );
        let searched = found[0]
            .evidence
            .iter()
            .find(|e| e.measured.contains("files read") || e.measured.contains("file read"))
            .expect("the search evidence");
        assert!(
            searched.measured.contains("2 rules read")
                && searched
                    .measured
                    .contains(&rules.join("testing.md").to_string_lossy().to_string()),
            "{}",
            searched.measured
        );
    }

    /// A rule that exists and cannot be read makes every negative
    /// Unknown, never "no rules". (A walled `.claude/rules` directory is
    /// already an unreadable directory in the scan and is said once; a
    /// walled rule file is what only the rules reader sees.)
    #[cfg(unix)]
    #[test]
    fn an_unreadable_rule_makes_the_negative_unknown() {
        use std::os::unix::fs::PermissionsExt;

        let (_t, repo, home) = fixture();
        fs::write(repo.join("Makefile"), "test:\n\ttrue\n").unwrap();
        fs::write(repo.join("CLAUDE.md"), "Nothing here.\n").unwrap();
        let rules = repo.join(".claude").join("rules");
        fs::create_dir_all(&rules).unwrap();
        let rule = rules.join("testing.md");
        fs::write(&rule, "Run `make lint`.\n").unwrap();
        fs::set_permissions(&rule, fs::Permissions::from_mode(0o000)).unwrap();
        let report = run_over(&repo, &home);
        fs::set_permissions(&rule, fs::Permissions::from_mode(0o644)).unwrap();

        let found = toolchain_findings(&report);
        assert_eq!(found.len(), 1, "{report:#?}");
        assert_eq!(found[0].severity, Severity::Unknown, "{}", found[0].finding);
        assert!(
            found[0].finding.contains("testing.md (")
                && found[0].finding.ends_with("` not readable"),
            "{}",
            found[0].finding
        );

        // The walled directory: Unknown, and named once.
        fs::set_permissions(&rules, fs::Permissions::from_mode(0o000)).unwrap();
        let report = run_over(&repo, &home);
        fs::set_permissions(&rules, fs::Permissions::from_mode(0o755)).unwrap();
        let found = toolchain_findings(&report);
        assert_eq!(found[0].severity, Severity::Unknown, "{}", found[0].finding);
        assert_eq!(
            found[0].finding.matches("not readable").count(),
            1,
            "{}",
            found[0].finding
        );
        assert_eq!(
            found[0].finding.matches("`, `").count(),
            0,
            "{}",
            found[0].finding
        );
    }

    /// #1394's test: a `make test` named only in a repository skill is
    /// not a gap, nor is a `make fmt` named only in a user skill, and
    /// the gap that remains counts the skills apart from the files and
    /// rules it read.
    #[test]
    fn a_command_named_in_a_skill_counts_and_the_skills_are_counted() {
        let (_t, repo, home) = fixture();
        fs::write(
            repo.join("Makefile"),
            "test:\n\ttrue\nlint:\n\ttrue\nfmt:\n\ttrue\n",
        )
        .unwrap();
        fs::write(repo.join("CLAUDE.md"), "Nothing here.\n").unwrap();
        let gate = repo.join(".claude").join("skills").join("gate");
        fs::create_dir_all(&gate).unwrap();
        fs::write(
            gate.join("SKILL.md"),
            "---\nname: gate\n---\nRun `make test`.\n",
        )
        .unwrap();
        let mine = home.join(".claude").join("skills").join("tidy");
        fs::create_dir_all(&mine).unwrap();
        fs::write(mine.join("SKILL.md"), "```\nmake fmt\n```\n").unwrap();
        let rules = repo.join(".claude").join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("style.md"), "Be terse.\n").unwrap();

        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        assert_eq!(found.len(), 1, "{report:#?}");
        assert_eq!(found[0].severity, Severity::Advice);
        assert_eq!(
            found[0].finding,
            "make (Makefile at root) offers `lint`; none of the 1 file read, 1 rule or 2 skills \
             names `make lint`"
        );
        let searched = found[0]
            .evidence
            .iter()
            .find(|e| e.measured.contains("file read"))
            .expect("the search evidence");
        assert!(
            searched.measured.contains("2 skills read")
                && searched
                    .measured
                    .contains(&gate.join("SKILL.md").to_string_lossy().to_string()),
            "{}",
            searched.measured
        );

        // The negative can fail: without the skills, test and fmt are
        // gaps again.
        fs::remove_dir_all(repo.join(".claude").join("skills")).unwrap();
        fs::remove_dir_all(home.join(".claude").join("skills")).unwrap();
        assert_eq!(toolchain_findings(&run_over(&repo, &home)).len(), 3);
    }

    /// A skill that exists and cannot be read makes the negative Unknown,
    /// never "not named" (#1351's rule, #1394).
    #[cfg(unix)]
    #[test]
    fn an_unreadable_skill_makes_the_negative_unknown() {
        use std::os::unix::fs::PermissionsExt;

        let (_t, repo, home) = fixture();
        fs::write(repo.join("Makefile"), "test:\n\ttrue\n").unwrap();
        fs::write(repo.join("CLAUDE.md"), "Nothing here.\n").unwrap();
        let gate = repo.join(".claude").join("skills").join("gate");
        fs::create_dir_all(&gate).unwrap();
        let skill = gate.join("SKILL.md");
        fs::write(&skill, "Run `make lint`.\n").unwrap();
        let report = run_over(&repo, &home);
        assert_eq!(
            toolchain_findings(&report)[0].severity,
            Severity::Advice,
            "{report:#?}"
        );

        fs::set_permissions(&skill, fs::Permissions::from_mode(0o000)).unwrap();
        let report = run_over(&repo, &home);
        fs::set_permissions(&skill, fs::Permissions::from_mode(0o644)).unwrap();
        let found = toolchain_findings(&report);
        assert_eq!(found.len(), 1, "{report:#?}");
        assert_eq!(found[0].severity, Severity::Unknown, "{}", found[0].finding);
        assert!(
            found[0].finding.contains("SKILL.md (") && found[0].finding.ends_with("` not readable"),
            "{}",
            found[0].finding
        );
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
        assert!(prose.iter().any(|s| s.contains("`make lint`")), "{prose:?}");

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
        assert!(path.iter().any(|s| s.contains("`make lint`")), "{path:?}");
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

        // #1392: a binary run through a manager maps by the tool map.
        assert_eq!(
            commands_in("yarn vitest run"),
            vec![("yarn".into(), Verb::Test)]
        );
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

    /// Launchers (#1323): `npx`, `bunx`, `pnpm`, `pnpm exec`, `bun run`
    /// and `nx` name a verb. An nx target comes from `run <project>:`,
    /// `<project>:<target>`, `nx <target>`, or `-t`/`--target(s)`, and
    /// one `-t lint,test` names both.
    #[test]
    fn launchers_and_nx_targets_name_a_verb() {
        assert_eq!(
            commands_in("npx nx run web:test --include=src"),
            vec![("nx".into(), Verb::Test)]
        );
        assert_eq!(
            commands_in("npx -y nx run web:build:production"),
            vec![("nx".into(), Verb::Build)]
        );
        assert_eq!(
            commands_in("nx affected -t lint"),
            vec![("nx".into(), Verb::Lint)]
        );
        assert_eq!(
            commands_in("bunx nx affected --targets=lint,test"),
            vec![("nx".into(), Verb::Lint), ("nx".into(), Verb::Test)]
        );
        assert_eq!(
            commands_in("nx run-many --target build"),
            vec![("nx".into(), Verb::Build)]
        );
        assert_eq!(
            commands_in("pnpm exec nx format:write"),
            vec![("nx".into(), Verb::Format)]
        );
        assert_eq!(
            commands_in("pnpm nx web:serve"),
            vec![("nx".into(), Verb::Run)]
        );
        assert_eq!(commands_in("nx test web"), vec![("nx".into(), Verb::Test)]);
        assert_eq!(commands_in("pnpm test"), vec![("pnpm".into(), Verb::Test)]);
        assert_eq!(
            commands_in("pnpm run lint"),
            vec![("pnpm".into(), Verb::Lint)]
        );
        assert_eq!(commands_in("bun run dev"), vec![("bun".into(), Verb::Run)]);
        // A launcher with nothing verb-shaped after it names nothing.
        assert_eq!(commands_in("npx playwright install"), vec![]);
        assert_eq!(commands_in("pnpm install"), vec![]);
        assert_eq!(commands_in("nx graph"), vec![]);
        assert_eq!(commands_in("npx"), vec![]);
    }

    /// #1323's test: a command naming the verb through a JS launcher
    /// clears the Makefile's `test-unit` gap, whichever manager offers the
    /// target. The cover stops at the family: `cargo test` clears the
    /// make gap too, but never a package.json script's.
    #[test]
    fn a_launcher_command_covers_the_verb_for_make_and_the_js_family() {
        let (_t, repo, home) = fixture();
        fs::write(repo.join("Makefile"), "test-unit:\n\ttrue\n").unwrap();
        fs::write(
            repo.join("package.json"),
            r#"{"scripts":{"test":"vitest"}}"#,
        )
        .unwrap();
        fs::write(repo.join("yarn.lock"), "").unwrap();
        fs::write(repo.join("Cargo.toml"), "[package]\nname = \"octocat\"\n").unwrap();

        // The test commands the one test finding lists (#1395).
        let test_gaps = |body: &str| -> Vec<String> {
            fs::write(repo.join("CLAUDE.md"), body).unwrap();
            let found = toolchain_findings(&run_over(&repo, &home))
                .iter()
                .map(|f| f.finding.clone())
                .collect::<Vec<_>>();
            ["`make test-unit`", "`yarn test`", "`cargo test`"]
                .into_iter()
                .filter(|c| found.iter().any(|s| s.contains(c)))
                .map(str::to_string)
                .collect()
        };

        // Nothing named: all three are gaps. The negative can fail.
        assert_eq!(test_gaps("Nothing.\n").len(), 3);

        for body in [
            "Run `npx nx run web:test` and `cargo test`.\n",
            "Run `pnpm test` and `cargo test`.\n",
        ] {
            let gaps = test_gaps(body);
            assert!(gaps.is_empty(), "{body}: {gaps:?}");
        }

        // `cargo test` alone covers make's test target, not yarn's.
        let gaps = test_gaps("Run `cargo test`.\n");
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert_eq!(gaps[0], "`yarn test`", "{gaps:?}");
    }

    /// #1392: a manager running a binary names what the binary does,
    /// through the same tool map script bodies use, and the command is
    /// credited to the manager that ran it. A script of the same name
    /// takes precedence, and a manager subcommand names nothing.
    #[test]
    fn a_binary_run_through_a_manager_maps_by_the_tool_map() {
        assert_eq!(
            commands_in("npx prettier --check ."),
            vec![("npx".into(), Verb::Format)]
        );
        assert_eq!(
            commands_in("pnpm exec eslint ."),
            vec![("pnpm".into(), Verb::Lint)]
        );
        assert_eq!(
            commands_in("bunx tsc -b"),
            vec![("bunx".into(), Verb::Build)]
        );
        assert_eq!(
            commands_in("yarn run jest --ci"),
            vec![("yarn".into(), Verb::Test)]
        );
        assert_eq!(
            commands_in("pnpm playwright test"),
            vec![("pnpm".into(), Verb::Test)]
        );
        assert_eq!(
            commands_in("yarn vite build"),
            vec![("yarn".into(), Verb::Build)]
        );
        assert_eq!(commands_in("yarn install"), vec![]);
        assert_eq!(commands_in("yarn vite"), vec![]);
        // `npm run` runs scripts only, never a binary.
        assert_eq!(commands_in("npm run vitest"), vec![]);

        // A script named `vitest` is what `yarn vitest` runs.
        let names: BTreeSet<String> = ["vitest".to_string()].into_iter().collect();
        let none = BTreeMap::new();
        let scripts = Scripts {
            verbs: &none,
            unknown: &BTreeMap::new(),
            names: &names,
            targets: &BTreeMap::new(),
            target_unknown: &BTreeMap::new(),
        };
        assert_eq!(commands_with("yarn vitest run", &scripts), vec![]);
        // `npx` runs the binary whatever the scripts are called.
        assert_eq!(
            commands_with("npx vitest run", &scripts),
            vec![("npx".into(), Verb::Test)]
        );

        // In a script body too: `yarn vitest` is vitest unless a script
        // of that name exists.
        let bodies: BTreeMap<&str, &str> =
            [("fmt", "yarn prettier --write .")].into_iter().collect();
        assert_eq!(
            body_verbs("yarn vitest run && yarn fmt", &bodies, true, None).verbs,
            vec![Verb::Test, Verb::Format]
        );
    }

    /// #1392's test: a CLAUDE.md naming `yarn vitest run` covers yarn's
    /// `test` script, and `npx prettier --check .` covers `format`.
    /// `yarn install` still names nothing, so the negative can fail.
    #[test]
    fn a_manager_running_a_binary_covers_the_scripts_verb() {
        let (_t, repo, home) = fixture();
        fs::write(
            repo.join("package.json"),
            r#"{"scripts":{"test":"vitest run","format":"prettier --write ."}}"#,
        )
        .unwrap();
        fs::write(repo.join("yarn.lock"), "").unwrap();

        fs::write(repo.join("CLAUDE.md"), "Run `yarn install`.\n").unwrap();
        let found = toolchain_findings(&run_over(&repo, &home)).len();
        assert_eq!(found, 2, "test and format are both gaps");

        fs::write(
            repo.join("CLAUDE.md"),
            "Run `yarn vitest run` and `npx prettier --check .`.\n",
        )
        .unwrap();
        let report = run_over(&repo, &home);
        assert!(toolchain_findings(&report).is_empty(), "{report:#?}");
    }

    /// #1341: a script body maps by the tools it runs, split at `&&`,
    /// `;` and `|`, through launchers and env assignments; `npm run <s>`
    /// is followed once and no further.
    #[test]
    fn a_script_body_maps_by_the_tools_it_runs() {
        let bodies: BTreeMap<&str, &str> = [
            ("a", "npm run b"),
            ("b", "npm run c"),
            ("c", "eslint ."),
            ("self", "npm run self"),
        ]
        .into_iter()
        .collect();
        let v = |body: &str| body_verbs(body, &bodies, true, None).verbs;
        assert_eq!(
            v("prettier --check $(git diff --name-only) && eslint . && vitest run"),
            vec![Verb::Format, Verb::Lint, Verb::Test]
        );
        assert_eq!(v("echo hi"), vec![]);
        assert_eq!(
            v("CI=1 npx jest; tsc -p . | tee out"),
            vec![Verb::Test, Verb::Build]
        );
        assert_eq!(
            v("biome lint . && playwright test && vite build && mocha"),
            vec![Verb::Lint, Verb::Test, Verb::Build]
        );
        assert_eq!(v("biome format --write ."), vec![Verb::Format]);
        assert_eq!(v("playwright install && vite"), vec![]);
        // A script named for its verb maps by name, without a body.
        assert_eq!(v("yarn lint && npm test"), vec![Verb::Lint, Verb::Test]);
        // Followed once: `b` is read, and what `b` runs through `c` is
        // not.
        assert_eq!(v("npm run c"), vec![Verb::Lint]);
        assert_eq!(v("npm run a"), vec![]);
        assert_eq!(v("pnpm run self"), vec![]);
        assert_eq!(v("npm run missing"), vec![]);
    }

    /// #1341's test: `verify` runs prettier, eslint and vitest, and a
    /// CLAUDE.md naming `npm run verify` names Format, Lint and Test. The
    /// same script running `echo hi` names nothing and is listed as not
    /// mapped.
    #[test]
    fn a_script_named_for_no_verb_is_mapped_by_its_body() {
        let (_t, repo, home) = fixture();
        let manifest = |verify: &str| {
            format!(
                r#"{{"scripts":{{"format":"prettier --write .","lint":"eslint .","test":"vitest","verify":"{verify}"}}}}"#
            )
        };
        fs::write(
            repo.join("package.json"),
            manifest("prettier --check $(git diff --name-only) && eslint . && vitest run"),
        )
        .unwrap();
        fs::write(
            repo.join("CLAUDE.md"),
            "Run `npm run verify` before pushing.\n",
        )
        .unwrap();

        let report = run_over(&repo, &home);
        assert!(toolchain_findings(&report).is_empty(), "{report:#?}");

        // `verify` is offered for each verb it runs, and is a candidate.
        let detection = detect(&repo);
        let npm = &detection.toolchains[0];
        let verify: Vec<Verb> = npm
            .offers
            .iter()
            .filter(|o| o.what == "verify")
            .map(|o| o.verb)
            .collect();
        assert_eq!(verify, vec![Verb::Format, Verb::Lint, Verb::Test]);
        assert!(npm.other.is_empty(), "{:?}", npm.other);

        fs::write(repo.join("package.json"), manifest("echo hi")).unwrap();
        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        assert_eq!(found.len(), 3, "{report:#?}");
        assert!(
            found.iter().all(|f| f
                .evidence
                .iter()
                .any(|e| e.measured == "not mapped to a verb, not counted: `verify`")),
            "{report:#?}"
        );
    }

    /// A package.json whose `verify` runs `body`, beside `test`,
    /// `test:unit`, `lint` and `lint:fix` scripts, and a CLAUDE.md naming
    /// `npm run verify` only.
    fn verify_app(repo: &Path, body: &str) {
        fs::write(
            repo.join("package.json"),
            format!(
                r#"{{"scripts":{{"test":"vitest","test:unit":"vitest run","lint":"eslint .","lint:fix":"eslint --fix .","verify":"{body}"}}}}"#
            ),
        )
        .unwrap();
        fs::write(
            repo.join("CLAUDE.md"),
            "Run `npm run verify` before pushing.\n",
        )
        .unwrap();
    }

    /// #1376's test: `verify` runs `bash tools/v.sh`, which runs prettier
    /// and eslint, so naming `npm run verify` covers format and lint. The
    /// one finding, for test, lists only the test scripts; a comment line
    /// in the file maps nothing.
    #[test]
    fn a_script_body_is_followed_into_a_repository_script_file() {
        let (_t, repo, home) = fixture();
        verify_app(&repo, "bash tools/v.sh");
        fs::create_dir_all(repo.join("tools")).unwrap();
        fs::write(
            repo.join("tools").join("v.sh"),
            "#!/bin/sh\nset -e\n# npx vitest run\nnpx prettier --check $(git diff --name-only)\neslint .\n",
        )
        .unwrap();

        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        assert_eq!(found.len(), 1, "{report:#?}");
        assert_eq!(found[0].severity, Severity::Advice);
        assert_eq!(
            found[0].finding,
            "npm (package.json at root) offers `test`, `test:unit`; none of the 1 file read \
             names `npm run test` or `npm run test:unit`"
        );

        // The negative can fail: without the file, `verify` maps to
        // nothing it can be shown to run, and format is not offered.
        let detection = detect(&repo);
        let verify: Vec<Verb> = detection.toolchains[0]
            .offers
            .iter()
            .filter(|o| o.what == "verify")
            .map(|o| o.verb)
            .collect();
        assert_eq!(verify, vec![Verb::Format, Verb::Lint]);
    }

    /// The launchers followed, each one level: `sh`, `node` and `./`. A
    /// file the followed file runs is not read.
    #[test]
    fn script_files_are_followed_one_level_by_each_launcher() {
        let (_t, repo, _home) = fixture();
        let tools = repo.join("tools");
        fs::create_dir_all(&tools).unwrap();
        fs::write(tools.join("f.sh"), "prettier --write .\n").unwrap();
        fs::write(tools.join("t.mjs"), "// eslint .\nvitest run\n").unwrap();
        fs::write(tools.join("outer.sh"), "bash tools/f.sh\n").unwrap();
        let bodies = BTreeMap::new();
        let at = Files {
            dir: &repo,
            repo: &repo,
        };
        let v = |body: &str| body_verbs(body, &bodies, true, Some(&at));
        assert_eq!(v("sh tools/f.sh").verbs, vec![Verb::Format]);
        assert_eq!(v("bash -e \"tools/f.sh\"").verbs, vec![Verb::Format]);
        assert_eq!(
            v("./tools/f.sh && node tools/t.mjs").verbs,
            vec![Verb::Format, Verb::Test]
        );
        assert_eq!(v("bash tools/outer.sh").verbs, vec![]);
        assert!(v("bash tools/outer.sh").unknown.is_empty());
        assert_eq!(v("bash -c 'prettier .'").verbs, vec![]);
        assert!(v("bash -c 'prettier .'").unknown.is_empty());

        // A symlink under the repository to a file outside it is not
        // read: the check is made through symlinks too.
        #[cfg(unix)]
        {
            let outside = _t.path().join("outside.sh");
            fs::write(&outside, "eslint .\n").unwrap();
            std::os::unix::fs::symlink(&outside, tools.join("link.sh")).unwrap();
            let body = v("bash tools/link.sh");
            assert_eq!(body.verbs, vec![]);
            assert!(
                body.unknown[0].1.contains("outside the repository"),
                "{:?}",
                body.unknown
            );
        }
    }

    /// A script file that cannot be read, or lies outside the repository,
    /// makes the named script's verbs Unknown, never uncovered; the
    /// outside file is not read. A script nothing names changes nothing.
    #[test]
    fn an_unreadable_or_outside_script_file_is_unknown_not_uncovered() {
        for (body, says) in [
            ("bash tools/missing.sh", "could not be read"),
            ("bash ../outside.sh", "outside the repository"),
        ] {
            let (t, repo, home) = fixture();
            // Readable, and would cover lint and test if it were read.
            fs::write(t.path().join("outside.sh"), "eslint .\nvitest\n").unwrap();
            verify_app(&repo, body);

            let report = run_over(&repo, &home);
            let found = toolchain_findings(&report);
            assert_eq!(found.len(), 2, "{body}: {report:#?}");
            for f in &found {
                assert_eq!(f.severity, Severity::Unknown, "{body}: {f:#?}");
                assert!(f.finding.contains("could not be decided"), "{}", f.finding);
                assert!(
                    f.evidence.iter().any(
                        |e| e.measured.contains("script `verify`") && e.measured.contains(says)
                    ),
                    "{body}: {:?}",
                    f.evidence
                );
            }

            // Nothing names `verify`: its file cannot cover anything.
            fs::write(repo.join("CLAUDE.md"), "Be careful.\n").unwrap();
            let report = run_over(&repo, &home);
            let found = toolchain_findings(&report);
            assert_eq!(found.len(), 2, "{body}: {report:#?}");
            assert!(
                found.iter().all(|f| f.severity == Severity::Advice),
                "{body}: {report:#?}"
            );
        }
    }

    /// A root crate and a Makefile, and the findings' sentences when
    /// the CLAUDE.md is `claude`.
    fn crate_and_makefile(repo: &Path, home: &Path, makefile: &str, claude: &str) -> Vec<Finding> {
        fs::write(repo.join("Cargo.toml"), "[package]\nname = \"octocat\"\n").unwrap();
        fs::write(repo.join("Makefile"), makefile).unwrap();
        fs::write(repo.join("CLAUDE.md"), claude).unwrap();
        toolchain_findings(&run_over(repo, home))
            .into_iter()
            .cloned()
            .collect()
    }

    /// #1393's test: a named make target covers what its recipe runs,
    /// through its prerequisites, split at `&&`. Naming `make fmt`
    /// instead leaves the cargo lint gap, so the negative can fail.
    #[test]
    fn a_named_make_target_covers_what_its_recipe_runs() {
        let (_t, repo, home) = fixture();
        let makefile = "lint: lint-rust\nlint-rust:\n\tcd a && cargo clippy\nfmt:\n\ttrue\n";
        let clippy = |found: &[Finding]| {
            found
                .iter()
                .filter(|f| f.finding.contains("`cargo clippy`"))
                .count()
        };

        let found = crate_and_makefile(&repo, &home, makefile, "Run `make lint`.\n");
        assert_eq!(clippy(&found), 0, "{found:#?}");

        let found = crate_and_makefile(&repo, &home, makefile, "Run `make fmt`.\n");
        assert_eq!(clippy(&found), 1, "{found:#?}");
        assert_eq!(found[0].severity, Severity::Advice);
    }

    /// `$(MAKE) <target>` is followed, a literal variable is read, a
    /// cycle ends, a JS binary in a recipe maps (#1392), and a
    /// non-literal command is skipped.
    #[test]
    fn recipes_follow_make_calls_and_variables_and_stop_at_cycles() {
        let (_t, repo, _home) = fixture();
        yarn_app(&repo);
        fs::write(repo.join("Cargo.toml"), "[package]\nname = \"octocat\"\n").unwrap();
        fs::write(
            repo.join("Makefile"),
            "CARGO := cargo\nDYN = $(shell which cargo)\n\
             check: loop\n\t@$(MAKE) --no-print-directory inner\n\t$(DYN) build\n\
             loop: check\n\
             inner:\n\tVITE_TARGET=x $(CARGO) test && yarn vitest run\n\
             elsewhere:\n\t$(MAKE) -C sub fmt\n",
        )
        .unwrap();
        let d = detect(&repo);
        let runs = |t: &str| {
            d.target_runs
                .get(&("make".to_string(), t.to_string()))
                .cloned()
        };
        assert_eq!(
            runs("check"),
            Some(vec![
                ("cargo".to_string(), Verb::Test),
                ("yarn".to_string(), Verb::Test),
            ])
        );
        // `-C sub` is another makefile's target: not followed.
        assert_eq!(runs("elsewhere"), None);
        assert!(d.target_unknown.is_empty(), "{:?}", d.target_unknown);
    }

    /// A prerequisite the parser cannot see is a file when the makefile
    /// is closed, and Unknown when an `include` could define it: the
    /// negative it would decide becomes Unknown, never Advice.
    #[test]
    fn a_missing_prerequisite_of_an_open_ended_makefile_is_unknown() {
        let (_t, repo, home) = fixture();
        let closed = "lint: lint-rust\n\ttrue\n";
        let found = crate_and_makefile(&repo, &home, closed, "Run `make lint`.\n");
        let gap = found
            .iter()
            .find(|f| f.finding.contains("`cargo clippy`"))
            .expect("the cargo lint gap");
        assert_eq!(gap.severity, Severity::Advice, "{gap:#?}");

        let open = "include rules.mk\nlint: lint-rust\n\ttrue\n";
        let found = crate_and_makefile(&repo, &home, open, "Run `make lint`.\n");
        let gap = found
            .iter()
            .find(|f| f.finding.contains("`cargo clippy`"))
            .expect("the cargo lint gap, undecided");
        assert_eq!(gap.severity, Severity::Unknown, "{gap:#?}");
        assert!(
            gap.finding.contains("`lint-rust`") && gap.finding.contains("includes other files"),
            "{}",
            gap.finding
        );

        // Nothing names `make lint`: the open makefile decides nothing.
        let found = crate_and_makefile(&repo, &home, open, "Nothing.\n");
        assert!(
            found.iter().all(|f| f.severity == Severity::Advice),
            "{found:#?}"
        );
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

    /// Four build toolchains: yarn with a `build` script, a binary
    /// crate, a Makefile `build` target and a hand-made Xcode project.
    fn four_builds(repo: &Path) {
        fs::write(
            repo.join("package.json"),
            r#"{"scripts":{"build":"vite build"}}"#,
        )
        .unwrap();
        fs::write(repo.join("yarn.lock"), "").unwrap();
        fs::write(repo.join("Cargo.toml"), "[package]\nname = \"octocat\"\n").unwrap();
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src").join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(repo.join("Makefile"), "build:\n\tyarn build\n").unwrap();
        fs::create_dir_all(repo.join("ios").join("App.xcodeproj")).unwrap();
    }

    /// #1395's test: four toolchains offering `build` and nothing named
    /// is ONE build finding listing four offers, with each toolchain's
    /// evidence. #1327's coverage rules still decide what is listed:
    /// `cargo build` named covers cargo and make (a task runner takes any
    /// manager), and the finding lists the other two.
    #[test]
    fn one_finding_per_uncovered_verb_lists_every_toolchains_offers() {
        let (_t, repo, home) = fixture();
        four_builds(&repo);
        fs::write(repo.join("CLAUDE.md"), "Nothing.\n").unwrap();

        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        let build: Vec<&&Finding> = found
            .iter()
            .filter(|f| f.finding.contains("`build`"))
            .collect();
        assert_eq!(build.len(), 1, "{found:#?}");
        let f = build[0];
        assert_eq!(f.severity, Severity::Advice);
        assert_eq!(
            f.finding,
            "yarn (package.json + yarn.lock at root) offers `build`, cargo (Cargo.toml at root) \
             offers `build`, make (Makefile at root) offers `build` and xcode (App.xcodeproj at \
             ios) offers `build`; none of the 1 file read names `yarn build`, `cargo build`, \
             `make build` or `xcodebuild build`"
        );
        for manifest in ["package.json", "Cargo.toml", "Makefile", "App.xcodeproj"] {
            assert!(
                f.evidence.iter().any(|e| matches!(
                    &e.at,
                    Locator::File { path, .. } if path.ends_with(manifest)
                )),
                "{manifest}: {:?}",
                f.evidence
            );
        }
        // One finding per verb in all: build, test, lint, fmt and run.
        assert_eq!(found.len(), 5, "{found:#?}");

        fs::write(repo.join("CLAUDE.md"), "Run `cargo build`.\n").unwrap();
        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        let build: Vec<&&Finding> = found
            .iter()
            .filter(|f| f.finding.contains("`build`"))
            .collect();
        assert_eq!(build.len(), 1, "{found:#?}");
        assert!(
            build[0].finding.starts_with(
                "yarn (package.json + yarn.lock at root) offers `build` and xcode \
                 (App.xcodeproj at ios) offers `build`;"
            ),
            "{}",
            build[0].finding
        );
    }

    /// #1395: within one verb, a toolchain whose negative can be decided
    /// and one whose cannot are two findings, Advice and Unknown, never
    /// one: the npm test gap depends on a script file nobody could read,
    /// the cargo one does not.
    #[test]
    fn decided_and_undecided_toolchains_are_not_merged() {
        let (_t, repo, home) = fixture();
        verify_app(&repo, "bash tools/missing.sh");
        fs::write(repo.join("Cargo.toml"), "[package]\nname = \"octocat\"\n").unwrap();
        let report = run_over(&repo, &home);
        let test: Vec<&Finding> = toolchain_findings(&report)
            .into_iter()
            .filter(|f| f.finding.contains("`cargo test`") || f.finding.contains("`npm run test`"))
            .collect();
        assert_eq!(test.len(), 2, "{test:#?}");
        let advice = test
            .iter()
            .find(|f| f.severity == Severity::Advice)
            .unwrap();
        let unknown = test
            .iter()
            .find(|f| f.severity == Severity::Unknown)
            .unwrap();
        assert!(
            advice.finding.starts_with("cargo (") && !advice.finding.contains("npm"),
            "{}",
            advice.finding
        );
        assert!(
            unknown.finding.starts_with("npm (") && !unknown.finding.contains("cargo"),
            "{}",
            unknown.finding
        );
    }

    /// #1395: `cargo run` is offered only for a crate with a binary
    /// target (`src/main.rs`, `src/bin/`, `[[bin]]`), and a finding's
    /// label names only the crates that offer its verb.
    #[test]
    fn a_library_crate_offers_no_run_and_is_not_labelled_for_it() {
        let (_t, repo, home) = fixture();
        for (dir, manifest, file) in [
            (
                "app",
                "[package]\nname = \"app\"\n",
                Some(("src", "main.rs")),
            ),
            (
                "tool",
                "[package]\nname = \"tool\"\n",
                Some(("src/bin", "x.rs")),
            ),
            (
                "declared",
                "[package]\nname = \"d\"\n[[bin]]\nname = \"d\"\n",
                None,
            ),
            (
                "lib",
                "[package]\nname = \"lib\"\n",
                Some(("src", "lib.rs")),
            ),
        ] {
            let d = repo.join(dir);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("Cargo.toml"), manifest).unwrap();
            if let Some((sub, name)) = file {
                let at = sub.split('/').fold(d.clone(), |p, c| p.join(c));
                fs::create_dir_all(&at).unwrap();
                fs::write(at.join(name), "").unwrap();
            }
        }
        fs::write(repo.join("CLAUDE.md"), "Nothing.\n").unwrap();

        let d = detect(&repo);
        let runs: Vec<String> = d
            .toolchains
            .iter()
            .filter(|t| t.offers.iter().any(|o| o.verb == Verb::Run))
            .map(|t| t.label.clone())
            .collect();
        assert_eq!(
            runs,
            vec![
                "Cargo.toml at app",
                "Cargo.toml at declared",
                "Cargo.toml at tool"
            ]
        );

        let report = run_over(&repo, &home);
        let found = toolchain_findings(&report);
        let run = found
            .iter()
            .find(|f| f.finding.contains("`cargo run`"))
            .expect("the run gap");
        assert!(
            run.finding.starts_with(
                "cargo (Cargo.toml at app, Cargo.toml at declared, Cargo.toml at tool) offers \
                 `run`;"
            ),
            "{}",
            run.finding
        );
        let build = found
            .iter()
            .find(|f| f.finding.contains("`cargo build`"))
            .expect("the build gap");
        assert!(
            build.finding.contains("Cargo.toml at lib"),
            "{}",
            build.finding
        );
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
        // --lib` are named, so none of these is a gap. `yarn vitest run`
        // names yarn's test (#1392). `make lint` runs `lint-rust`, which
        // runs `cargo clippy` (#1393).
        for named in [
            "`make lint`",
            "`make test-mobile`",
            "`cargo fmt`",
            "`cargo test`",
            "`yarn test`",
            "`cargo clippy`",
        ] {
            assert!(
                !found.iter().any(|f| f.finding.contains(named)),
                "{named} is named in a CLAUDE.md here: {:?}",
                found.iter().map(|f| &f.finding).collect::<Vec<_>>()
            );
        }
        // `cargo build` and `make dev` exist and nothing names them.
        for gap in ["`cargo build`", "`make dev`"] {
            assert!(
                found.iter().any(|f| f.finding.contains(gap)),
                "{gap} is a gap here: {:?}",
                found.iter().map(|f| &f.finding).collect::<Vec<_>>()
            );
        }
        // Build and dev are each ONE finding listing every offer (#1395),
        // and the step-up crate, a library, offers no `cargo run`.
        let sentences: Vec<&String> = found.iter().map(|f| &f.finding).collect();
        let build: Vec<&&String> = sentences
            .iter()
            .filter(|s| s.contains("`cargo build`"))
            .collect();
        assert_eq!(build.len(), 1, "{sentences:#?}");
        let run: Vec<&&String> = sentences
            .iter()
            .filter(|s| s.contains("`make dev`"))
            .collect();
        assert_eq!(run.len(), 1, "{sentences:#?}");
        assert!(
            run[0].contains("`yarn dev`") && run[0].contains("`cargo run`"),
            "{}",
            run[0]
        );
        assert!(!run[0].contains("headstate-stepup"), "{}", run[0]);
        // The verify skill is read (#1394).
        assert!(run[0].contains(" skills names "), "{}", run[0]);
    }
}
