//! Content shape: the rules from published guidance that a deterministic
//! check can enforce over one CLAUDE.md or one launch set.
//!
//! The research behind every rule is
//! `docs/superpowers/specs/2026-09-21-claude-md-practices-research.md`.
//! It marks each source *measured* or *asserted*, and that sets the
//! severity here:
//!
//! - **Behaviour facts** from Claude Code's memory docs
//!   (`code.claude.com/docs/en/memory`) and changelog say what the tool
//!   DOES, and are the only grounds for [`Severity::Problem`].
//! - **Advice** from Anthropic's guidance pages, HumanLayer, Cursor and
//!   the published linters is asserted, not measured, and is
//!   [`Severity::Advice`]. The brief quotes the source; the finding
//!   never predicts an effect.
//! - **Measurements.** ETH Zurich (arXiv:2602.11988): context files raise
//!   inference cost by over 20% on average. McMillan (arXiv:2605.10039):
//!   1,650 sessions, file size varied 25 to 500 lines, no detectable
//!   adherence difference. UFMG (arXiv:2606.15828): 91% of 100 popular
//!   repositories carry a content smell; files of 200+ lines in 42%.
//!
//! So a size finding states the cost and Anthropic's target, never
//! "adherence will drop". The bill is measured; the compliance loss is
//! not.
//!
//! # The rules, with source and threshold
//!
//! | rule | source | threshold | severity |
//! |---|---|---|---|
//! | [`Rule::LineTarget`] | memory docs "target under 200 lines per CLAUDE.md file"; features-overview "Keep CLAUDE.md under 200 lines"; UFMG's context-bloat threshold | 200 lines. The brief names the community range: 60 (HumanLayer's own root), 300 (HumanLayer's cap), 500 (Cursor rules) | Advice |
//! | [`Rule::LaunchSet`] | memory docs: imported files "still load and enter the context window at launch"; subdirectory files load "when Claude reads files in those subdirectories" | none: one informational row per report, every file loaded before the first prompt with lines and est. tokens; "at least" when the scan is partial | Advice |
//! | [`Rule::HardSkip`] | memory docs "loads a CLAUDE.md file of up to 4 MiB in full and skips a larger file" | 4 MiB, on-disk bytes | Problem |
//! | [`Rule::Secret`] | cclint's secret rule; changelog: the feedback share uploads "the system prompt (which includes your CLAUDE.md instructions)" | `sk-ant-`, `ghp_`, `github_pat_`, a PEM private-key header, `AKIA` + 16; placeholders (`xxx`, `your`, `example`, one repeated character) skipped. The finding carries the line and a masked prefix, never the value | Problem |
//! | [`Rule::Emphasis`] | best-practices "add emphasis such as 'IMPORTANT' to that line alone. If you emphasize many lines, none of them stands out." | 2 or more prose lines in one file carrying all-caps `IMPORTANT`, `YOU MUST`, `NEVER` or `ALWAYS`. Caps only: bold prose does not count, or this repository's own house style would trip it | Advice |
//! | [`Rule::HookRule`] | memory docs: Claude treats CLAUDE.md as "context, not enforced configuration … use a PreToolUse hook instead"; features-overview "Put guardrails in hooks" | a tool verb (`edit`, `write`, `delete`, `rm`, `commit`, `push`, `force`), any case, in the clause a modal opens: after `never`, `always`, `must not` or `do not`, before the next `;` or sentence-ending `.`, `!`, `?` on the same line (#1374). A tool verb before the modal or in another sentence ("fails any loosening edit. Never loosen a baseline.") is not the rule's verb. Words are read outside inline code spans. A hyphenated compound counts only when every part is a tool verb (`force-push` fires; `slow-write`, `write-ahead` do not), a word directly after `@` is a tag, and a word directly after a determiner, quantifier or possessive (`a`, `an`, `any`, `the`, `each`, `every`, `this`, `that`, `no`, `some`, `my`, `your`, `his`, `her`, `its`, `our`, `their`, `'s`) is a noun. A code span in the clause counts only when it is itself a tool-action command, by its first words: `git <verb>` names the verb, `rm` names `rm`; any other span (`8 write`) names nothing (#1322). Still not parsed: a noun after an adjective ("never make any loosening edit" fires), a verb in a subordinate clause ("always run the gate before you commit" fires), a rule wrapped across lines, `don't`, and an abbreviation's `.` read as a sentence break | Advice |
//! | [`Rule::Conflict`] | memory docs "if two rules contradict each other, Claude may pick one arbitrarily"; UFMG: conflicting instructions in 28% | two files in one launch set whose named package managers (`npm`/`pnpm`/`yarn`/`bun`), lint entry points (`make lint` vs `yarn lint` …) or default branches are non-empty and disjoint | Advice |
//! | [`Rule::TreeListing`] | `/doctor` "cuts content Claude can derive from the codebase, such as directory layouts"; best-practices' exclude table | a fenced block with 3 or more lines starting `├`, `└` or `│` | Advice |
//! | [`Rule::InitSkeleton`] | UFMG: init fossilization in 24%; `/doctor` removes architecture overviews; best-practices "There's no required format" | the `/init` skeleton headings `Project Overview`, `Development Commands` and `Architecture` all present | Advice |
//! | [`Rule::AgentsMd`] | memory docs (2.1.277): with both files present Claude Code reads "Your CLAUDE.md files only"; a CLAUDE.md naming AGENTS.md in prose → "Claude sees AGENTS.md only if it decides to open the file" | `AGENTS.md` beside a repo-scope CLAUDE.md whose imports do not resolve to it | Advice |
//! | [`Rule::LocalNotIgnored`] | memory docs: `CLAUDE.local.md`, "add to `.gitignore`" | `git check-ignore` exits 1 for it | Problem |
//!
//! Rules the research hands to other producers, so they are not
//! unowned: lint leakage to toolchain; blind references and dated facts
//! to rot; duplicate content across scopes and an always-rule in a
//! lazily loaded subdirectory file to placement; import depth and
//! external imports to the imports seed (`advice/imports.rs`); the skills
//! authoring page's rules to skills.
//!
//! # What is read, and how
//!
//! Every CLAUDE.md the scan found -- repo, global and local scope -- is
//! read once here, since [`crate::claudemd::ClaudeFile`] carries no
//! text (it is on the wire, and `claude_md_content` exists so a file's
//! text crosses the bridge only when shown). A file the scan read and
//! this producer cannot is `Err`: the check is Unknown, never clean.
//! Prose rules read through `text::prose_lines`, so a fence and a
//! block-level HTML comment are invisible to them, as they are to Claude
//! (a comment) or are somebody's syntax (a fence). The secret rule reads
//! every raw line, comments and fences included: a secret in a committed
//! file is a secret whatever Claude sees.
//!
//! The launch set is what the memory docs say loads at launch from the
//! repository root: `~/.claude/CLAUDE.md`, `./CLAUDE.md`,
//! `./.claude/CLAUDE.md`, `./CLAUDE.local.md`, and each one's resolved
//! import tree. Subdirectory files load only after a Read there and are
//! measured by the per-file rules but not counted here. `.claude/rules/`
//! is not scanned (a non-goal of the design), so a repository that uses
//! it has a launch set larger than this row, which is one reason the
//! row's figures are estimates and say so.
//!
//! # Claude Code's own "too long" warning is not reproduced
//!
//! Its threshold "scales with the model's context window" (changelog
//! 2.1.169), and Headstate cannot know which model a session will use.
//!
//! # Rejected: a required-sections template
//!
//! cclint and carlrannaberg/cclint require `Project Overview`,
//! `Development Commands` and `Architecture`. Anthropic says "There's no
//! required format", `/doctor` deletes architecture overviews, and UFMG
//! names the `/init` skeleton as a smell. So the template's presence is
//! a finding here, and its absence never is.
//!
//! # Measured on this repository (2026-09-21)
//!
//! `CLAUDE.md` 60, `src/CLAUDE.md` 82, `src-tauri/CLAUDE.md` 48,
//! `scripts/CLAUDE.md` 29, `.github/CLAUDE.md` 81 lines; no `@` imports,
//! no `AGENTS.md`, no `CLAUDE.local.md`. Every emphasis in these files
//! is bold prose, not caps. The one line-wrapped "Never work directly on
//! `main`" sentence puts its tool verb on the next line, so the hook
//! rule, which reads one line at a time, is silent on it. The producer
//! emits the launch-set row and nothing else here; the `#[ignore]`d
//! test at the bottom prints the report so the figure can be re-taken.

use super::{Check, Context, Evidence, Finding, Locator, Producer, Severity, Subject};
use crate::claudemd::imports::parse_imports;
use crate::claudemd::{text, ClaudeFile, ImportNode, Scope};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Anthropic's line target per file. Memory docs: "target under 200
/// lines per CLAUDE.md file".
const LINE_TARGET: usize = 200;

/// The hard skip. Memory docs: "loads a CLAUDE.md file of up to 4 MiB in
/// full and skips a larger file".
const HARD_SKIP_BYTES: u64 = 4 * 1024 * 1024;

/// Emphasis, all caps only: the best-practices sentence is about the
/// word `IMPORTANT`, and this repository's own house style is bold
/// prose, which must not count.
static EMPHASIS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(IMPORTANT|YOU MUST|NEVER|ALWAYS)\b").unwrap());
/// A hard rule's modal, any case.
static MODAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(never|must not|do not|always)\b").unwrap());
/// A tool action a `PreToolUse` hook can block.
const TOOL_VERBS: [&str; 7] = ["edit", "write", "delete", "rm", "commit", "push", "force"];
/// A word, or words joined by single hyphens (`slow-write`,
/// `force-push`): the unit a tool verb is judged as. Leftmost-longest,
/// so a compound is never matched as its parts, and a leading `--` is
/// not a join (`--force` is the word `force`).
static COMPOUND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+(?:-\w+)*").unwrap());
static PACKAGE_MANAGER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(npm|pnpm|yarn|bun)\b").unwrap());
static LINT_ENTRY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(make lint|yarn lint|npm run lint|pnpm lint|pnpm run lint|bun lint|bun run lint|just lint)\b")
        .unwrap()
});
static DEFAULT_BRANCH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)default branch").unwrap());
static BRANCH_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(main|master|develop|trunk)\b").unwrap());

/// Secret shapes: the pattern, the prefix the finding may print, and
/// what the token is called. The minimum tail lengths keep a sentence
/// like "keys start with `sk-ant-`" from firing.
static SECRETS: LazyLock<Vec<(Regex, &'static str, &'static str)>> = LazyLock::new(|| {
    vec![
        (
            Regex::new(r"sk-ant-[A-Za-z0-9_-]{20,}").unwrap(),
            "sk-ant-",
            "an Anthropic API key",
        ),
        (
            Regex::new(r"\bghp_[A-Za-z0-9]{20,}").unwrap(),
            "ghp_",
            "a GitHub personal access token",
        ),
        (
            Regex::new(r"\bgithub_pat_[A-Za-z0-9_]{20,}").unwrap(),
            "github_pat_",
            "a GitHub fine-grained token",
        ),
        (
            Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap(),
            "-----BEGIN",
            "a private key",
        ),
        (
            Regex::new(r"\bAKIA[0-9A-Z]{16}\b").unwrap(),
            "AKIA",
            "an AWS access key id",
        ),
    ]
});

/// The `/init` skeleton, as `/init` writes it.
const INIT_HEADINGS: [&str; 3] = ["Project Overview", "Development Commands", "Architecture"];

/// Judgement only: shown as attributed text at the end of every brief,
/// never a finding, because none of it has a mechanical test. (text,
/// source).
pub const GUIDANCE: &[(&str, &str)] = &[
    (
        "For each line, ask: would removing this cause Claude to make mistakes? If not, cut it.",
        "Anthropic, Best practices for Claude Code (code.claude.com/docs/en/best-practices)",
    ),
    (
        "Human-written context files should describe only minimal requirements: in the one \
         study that measured it, context files raised inference cost by over 20% on average \
         and moved task success by about 4% either way.",
        "Gloaguen, Mündler et al., ETH Zurich, arXiv:2602.11988 (Feb 2026)",
    ),
    (
        "Add a rule when Claude gets a convention or command wrong twice; capture a procedure \
         as a skill when it is pasted into chat for the third time.",
        "Anthropic, Extend Claude Code (code.claude.com/docs/en/features-overview)",
    ),
    (
        "Structure a file as WHY, WHAT and HOW; point at file:line rather than pasting \
         snippets; keep the root file short and move the rest to documents Claude reads on \
         demand.",
        "HumanLayer, Writing a good CLAUDE.md (Nov 2025)",
    ),
    (
        "Revisit after major model releases: instructions that worked around an older model's \
         limitation may become overhead once a newer model handles the case on its own.",
        "Anthropic, Set up Claude Code in a monorepo or large codebase \
         (code.claude.com/docs/en/large-codebases)",
    ),
    (
        "Process and gating rules are the ones not followed, and adherence decays with \
         position in the session rather than with the file's shape; a rule that must hold \
         belongs in a Stop or PreToolUse hook, not a louder sentence.",
        "anthropics/claude-code#46724; McMillan, arXiv:2605.10039 (May 2026)",
    ),
];

/// One rule this producer enforces. The id rides on
/// [`Finding::rule`] so the brief's suggestion is the rule's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    LineTarget,
    LaunchSet,
    HardSkip,
    Secret,
    Emphasis,
    HookRule,
    Conflict,
    TreeListing,
    InitSkeleton,
    AgentsMd,
    LocalNotIgnored,
}

impl Rule {
    pub const ALL: &'static [Rule] = &[
        Rule::LineTarget,
        Rule::LaunchSet,
        Rule::HardSkip,
        Rule::Secret,
        Rule::Emphasis,
        Rule::HookRule,
        Rule::Conflict,
        Rule::TreeListing,
        Rule::InitSkeleton,
        Rule::AgentsMd,
        Rule::LocalNotIgnored,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Rule::LineTarget => "shape.line-target",
            Rule::LaunchSet => "shape.launch-set",
            Rule::HardSkip => "shape.hard-skip",
            Rule::Secret => "shape.secret",
            Rule::Emphasis => "shape.emphasis",
            Rule::HookRule => "shape.hook-rule",
            Rule::Conflict => "shape.conflict",
            Rule::TreeListing => "shape.tree-listing",
            Rule::InitSkeleton => "shape.init-skeleton",
            Rule::AgentsMd => "shape.agents-md",
            Rule::LocalNotIgnored => "shape.local-not-ignored",
        }
    }

    fn from_id(id: &str) -> Option<Rule> {
        Rule::ALL.iter().copied().find(|r| r.id() == id)
    }

    /// What to change, quoting the source the rule rests on.
    fn suggestion(self, f: &Finding) -> String {
        let path = f.subject.path();
        match self {
            Rule::LineTarget => format!(
                "Move the sections of `{path}` that apply to one part of the codebase into a \
                 path-scoped `.claude/rules/*.md` or that directory's own CLAUDE.md, and cut \
                 lines whose removal would not cause a mistake, until the file is under 200 \
                 lines (memory docs: \"target under 200 lines per CLAUDE.md file\"). Published \
                 ceilings range from 60 lines (HumanLayer's own root file) through 300 \
                 (HumanLayer's cap) to 500 (Cursor rules); the one controlled study found no \
                 adherence difference between 25 and 500 lines, so the case for cutting is the \
                 context cost, which is measured."
            ),
            Rule::LaunchSet => "None from this row. It states what loads before the first \
                 prompt, with each file's lines and est. tokens, so the other findings' edits \
                 can be read against the bill they reduce. Nothing here is a defect."
                .to_string(),
            Rule::HardSkip => format!(
                "Split `{path}` so no single file exceeds 4 MiB. Memory docs: Claude Code \
                 \"loads a CLAUDE.md file of up to 4 MiB in full and skips a larger file\", so \
                 today none of this file loads."
            ),
            Rule::Secret => format!(
                "Remove the token at the line named in the evidence from `{path}` and rotate \
                 the credential it belongs to; a CLAUDE.md enters every session's context and \
                 is included when feedback is shared. Name the environment variable that holds \
                 the secret instead."
            ),
            Rule::Emphasis => format!(
                "Keep the emphasis on the one line in `{path}` Claude keeps skipping and write \
                 the others in plain imperative. Best practices: \"add emphasis such as \
                 'IMPORTANT' to that line alone. If you emphasize many lines, none of them \
                 stands out.\""
            ),
            Rule::HookRule => format!(
                "Enforce the line quoted in the evidence with a PreToolUse hook in \
                 `.claude/settings.json` that blocks the tool call, then keep or shorten the \
                 sentence in `{path}`. Memory docs: Claude treats CLAUDE.md as \"context, not \
                 enforced configuration. To block an action regardless of what Claude decides, \
                 use a PreToolUse hook instead.\""
            ),
            Rule::Conflict => format!(
                "Make the two lines in the evidence agree, or delete the one that is no longer \
                 true, starting with `{path}`, which is read last. Memory docs: \"if two rules \
                 contradict each other, Claude may pick one arbitrarily.\""
            ),
            Rule::TreeListing => format!(
                "Delete the tree from `{path}`, or replace it with one sentence naming the \
                 directories that matter and why. `/doctor` cuts \"directory layouts, \
                 dependency lists, and architecture overviews\" as content Claude can derive \
                 from the codebase."
            ),
            Rule::InitSkeleton => format!(
                "Rewrite `{path}` around what Claude gets wrong: replace the overview and \
                 architecture sections with the pitfalls, rationale and conventions that differ \
                 from tool defaults, which is what `/doctor` keeps. Best practices: \"There's no \
                 required format.\""
            ),
            Rule::AgentsMd => format!(
                "Add a line `@AGENTS.md` to `{path}` so its content loads at launch, or delete \
                 `{path}` so Claude Code reads `AGENTS.md` directly (since 2.1.277). Naming the \
                 file in prose is not enough: Claude \"sees AGENTS.md only if it decides to \
                 open the file\"."
            ),
            Rule::LocalNotIgnored => "Add `CLAUDE.local.md` to the repository's `.gitignore`. \
                 Memory docs list it as the file to \"add to .gitignore\": it holds one \
                 person's overrides and is not meant to be committed."
                .to_string(),
        }
    }
}

/// The brief's suggestion for a shape finding, by its rule.
///
/// Every finding this module builds goes through [`emit`], which sets
/// the rule, so the `None` arm is reached only by a finding built with
/// `Finding::new` outside this module. It names the file and stops
/// rather than inventing a remedy for a rule it cannot identify.
pub fn suggestion(f: &Finding) -> String {
    match f.rule.and_then(Rule::from_id) {
        Some(rule) => rule.suggestion(f),
        None => format!("Edit `{}` as the finding states.", f.subject.path()),
    }
}

pub struct Shape;

impl Producer for Shape {
    fn check(&self) -> Check {
        Check::Shape
    }

    fn run(&self, cx: &Context) -> Result<Vec<Finding>, String> {
        run_with_git(cx, crate::auth::git_program())
    }
}

/// One CLAUDE.md, read.
struct Loaded<'a> {
    file: &'a ClaudeFile,
    scope: Scope,
    text: String,
    /// Lines in the file as written, `wc -l` style. The line target is
    /// an authoring target, so it counts the file the author sees;
    /// the token estimate counts what Claude sees.
    lines: usize,
    /// Whether the file loads at launch from the repository root.
    launch: bool,
}

/// [`Producer::run`] with the git binary injected, so a test can prove
/// what a missing one produces.
fn run_with_git(cx: &Context, git: &Path) -> Result<Vec<Finding>, String> {
    let loaded = load(cx)?;
    let mut out = vec![launch_set(cx, &loaded)?];
    for l in &loaded {
        per_file(cx, l, &mut out);
    }
    conflicts(cx, &loaded, &mut out);
    agents_md(cx, &loaded, &mut out);
    local_ignored(cx, git, &loaded, &mut out)?;
    Ok(out)
}

/// Read every CLAUDE.md the scan found, in load order: global, the
/// repository's launch files, local, then the subdirectory files.
fn load<'a>(cx: &Context<'a>) -> Result<Vec<Loaded<'a>>, String> {
    fn read(file: &ClaudeFile, scope: Scope, launch: bool) -> Result<Loaded<'_>, String> {
        let text = std::fs::read_to_string(&file.path)
            .map_err(|e| format!("{}: could not be read: {e}", file.path))?;
        let lines = text.lines().count();
        Ok(Loaded {
            file,
            scope,
            text,
            lines,
            launch,
        })
    }
    let mut out = Vec::new();
    for s in cx.scan.extra.iter().filter(|s| s.scope == Scope::Global) {
        out.push(read(&s.file, s.scope, true)?);
    }
    let dot_claude = cx.repo.join(".claude");
    let at_root = |f: &ClaudeFile| {
        let parent = Path::new(&f.path).parent();
        parent == Some(cx.repo) || parent == Some(dot_claude.as_path())
    };
    for f in cx.scan.repo.files.iter().filter(|f| at_root(f)) {
        out.push(read(f, Scope::Repo, true)?);
    }
    for s in cx.scan.extra.iter().filter(|s| s.scope == Scope::Local) {
        out.push(read(&s.file, s.scope, true)?);
    }
    for f in cx.scan.repo.files.iter().filter(|f| !at_root(f)) {
        out.push(read(f, Scope::Repo, false)?);
    }
    Ok(out)
}

/// A path as the finding's sentence names it: relative to the
/// repository when inside it, whole otherwise (the global file).
fn label(path: &str, repo: &Path) -> String {
    Path::new(path)
        .strip_prefix(repo)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}

/// `1234567` as `1,234,567`, the way the spec's example rows print.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn subject(l: &Loaded) -> Subject {
    Subject::ClaudeMd {
        path: l.file.path.clone(),
        scope: l.scope,
        section: None,
    }
}

fn at(path: &str, line: Option<usize>) -> Locator {
    Locator::File {
        path: path.to_string(),
        line: line.map(|n| n as u32),
    }
}

fn evidence(path: &str, line: Option<usize>, measured: String) -> Evidence {
    Evidence {
        at: at(path, line),
        measured,
    }
}

/// The one constructor this module uses, so every finding carries its
/// rule and the brief's suggestion is the rule's.
fn emit(
    rule: Rule,
    severity: Severity,
    subject: Subject,
    evidence: Vec<Evidence>,
    finding: String,
) -> Finding {
    Finding::with_rule(
        Check::Shape,
        rule.id(),
        severity,
        subject,
        evidence,
        finding,
    )
}

/// The informational row: every file loaded before the first prompt.
fn launch_set(cx: &Context, loaded: &[Loaded]) -> Result<Finding, String> {
    let mut files = 0u64;
    let mut lines = 0u64;
    let mut tokens = 0u64;
    let mut ev = Vec::new();
    for l in loaded.iter().filter(|l| l.launch) {
        files += 1;
        lines += l.lines as u64;
        tokens += l.file.tokens;
        let scope = match l.scope {
            Scope::Global => "global",
            Scope::Repo => "repo",
            Scope::Local => "local",
        };
        ev.push(evidence(
            &l.file.path,
            None,
            format!(
                "{} lines, est. {} tokens ({scope})",
                l.lines,
                grouped(l.file.tokens)
            ),
        ));
        count_imports(
            &l.file.path,
            &l.file.imports,
            &mut files,
            &mut lines,
            &mut tokens,
            &mut ev,
        )?;
    }
    // Qualified whenever the scan is: an unreadable scope, an unreadable
    // import or an unset home each hide weight that would add to these
    // figures.
    let floor = cx.scan.combined_partial();
    let finding = if files == 0 {
        let lazy = loaded.iter().filter(|l| !l.launch).count();
        format!(
            "no CLAUDE.md loads before the first prompt in this repository; {lazy} under \
             subdirectories load only after a Read there"
        )
    } else {
        format!(
            "{q}{files} file{s}, {lines} lines and est. {} tokens load before the first prompt",
            grouped(tokens),
            q = if floor { "at least " } else { "" },
            s = if files == 1 { "" } else { "s" },
        )
    };
    Ok(emit(
        Rule::LaunchSet,
        Severity::Advice,
        Subject::Directory {
            path: cx.repo.to_string_lossy().to_string(),
        },
        ev,
        finding,
    ))
}

/// Add every resolved import under `nodes` to the launch-set figures.
/// A broken node is not loaded and not counted; an unreadable one is
/// already what makes the scan partial.
fn count_imports(
    importer: &str,
    nodes: &[ImportNode],
    files: &mut u64,
    lines: &mut u64,
    tokens: &mut u64,
    ev: &mut Vec<Evidence>,
) -> Result<(), String> {
    for n in nodes {
        let (Some(path), None) = (&n.path, &n.problem) else {
            continue;
        };
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("{path}: could not be read: {e}"))?;
        let n_lines = text.lines().count() as u64;
        *files += 1;
        *lines += n_lines;
        *tokens += n.tokens;
        ev.push(evidence(
            path,
            None,
            format!(
                "{n_lines} lines, est. {} tokens, imported by `{importer}`",
                grouped(n.tokens)
            ),
        ));
        count_imports(path, &n.children, files, lines, tokens, ev)?;
    }
    Ok(())
}

/// The rules over one file.
fn per_file(cx: &Context, l: &Loaded, out: &mut Vec<Finding>) {
    let name = label(&l.file.path, cx.repo);
    let path = l.file.path.as_str();

    if l.file.bytes > HARD_SKIP_BYTES {
        out.push(emit(
            Rule::HardSkip,
            Severity::Problem,
            subject(l),
            vec![evidence(
                path,
                None,
                format!("{} bytes on disk", grouped(l.file.bytes)),
            )],
            format!(
                "`{name}` is {:.1} MiB; Claude Code skips a CLAUDE.md over 4 MiB entirely",
                l.file.bytes as f64 / (1024.0 * 1024.0)
            ),
        ));
    }

    if l.lines > LINE_TARGET {
        out.push(emit(
            Rule::LineTarget,
            Severity::Advice,
            subject(l),
            vec![evidence(
                path,
                Some(LINE_TARGET + 1),
                format!(
                    "{} lines; est. {} tokens; line {} is the first past the target",
                    l.lines,
                    grouped(l.file.tokens),
                    LINE_TARGET + 1
                ),
            )],
            format!(
                "`{name}` runs to {} lines; Anthropic's target is under {LINE_TARGET} (est. {} tokens)",
                l.lines,
                grouped(l.file.tokens)
            ),
        ));
    }

    // Secrets: every raw line, fences and comments included.
    for (i, line) in l.text.lines().enumerate() {
        let n = i + 1;
        for (re, prefix, kind) in SECRETS.iter() {
            for m in re.find_iter(line) {
                if is_placeholder(m.as_str(), prefix) {
                    continue;
                }
                out.push(emit(
                    Rule::Secret,
                    Severity::Problem,
                    subject(l),
                    vec![evidence(
                        path,
                        Some(n),
                        format!(
                            "line {n}: {} characters beginning `{prefix}`; the value is not reproduced",
                            m.as_str().chars().count()
                        ),
                    )],
                    format!("`{name}` line {n} holds a token shaped like {kind} (`{prefix}…`)"),
                ));
            }
        }
    }

    let prose = text::prose_lines(&l.text);

    // Emphasis: two or more lines, caps only.
    let emphasised: Vec<(usize, &str)> = prose
        .iter()
        .filter_map(|(n, line)| EMPHASIS.find(line).map(|m| (*n, m.as_str())))
        .collect();
    if emphasised.len() >= 2 {
        out.push(emit(
            Rule::Emphasis,
            Severity::Advice,
            subject(l),
            emphasised
                .iter()
                .map(|(n, token)| evidence(path, Some(*n), format!("`{token}`")))
                .collect(),
            format!(
                "`{name}` carries IMPORTANT, YOU MUST, NEVER or ALWAYS in caps on {} lines",
                emphasised.len()
            ),
        ));
    }

    // A hard rule a hook could enforce: one finding per line.
    for (n, line) in &prose {
        if !MODAL.is_match(line) {
            continue;
        }
        let Some(verb) = tool_action(line) else {
            continue;
        };
        out.push(emit(
            Rule::HookRule,
            Severity::Advice,
            subject(l),
            vec![evidence(path, Some(*n), clamp(line.trim(), 160))],
            format!("`{name}` line {n} states a never/always rule naming a tool action (`{verb}`)"),
        ));
    }

    // Derivable content (a): a fenced tree listing.
    for fence in text::fences(&l.text) {
        let body: Vec<&str> = fence.body.lines().collect();
        let connectors = body
            .iter()
            .filter(|b| {
                let t = b.trim_start();
                t.starts_with('├') || t.starts_with('└') || t.starts_with('│')
            })
            .count();
        if connectors < 3 {
            continue;
        }
        let first = fence.line + 1;
        let last = fence.line + body.len();
        out.push(emit(
            Rule::TreeListing,
            Severity::Advice,
            subject(l),
            vec![evidence(
                path,
                Some(fence.line),
                format!(
                    "{connectors} of {} lines in the block are tree connectors (lines {first}–{last})",
                    body.len()
                ),
            )],
            format!("`{name}` lines {first}–{last} hold a fenced directory tree"),
        ));
    }

    // Derivable content (c): the `/init` skeleton, all three headings.
    let sections = text::sections(&l.text);
    let found: Vec<(usize, &str)> = INIT_HEADINGS
        .iter()
        .filter_map(|h| {
            sections
                .iter()
                .find(|s| {
                    s.heading
                        .as_deref()
                        .is_some_and(|t| t.eq_ignore_ascii_case(h))
                })
                .map(|s| (s.line, *h))
        })
        .collect();
    if found.len() == INIT_HEADINGS.len() {
        out.push(emit(
            Rule::InitSkeleton,
            Severity::Advice,
            subject(l),
            found
                .iter()
                .map(|(n, h)| evidence(path, Some(*n), format!("heading `{h}`")))
                .collect(),
            format!("`{name}` carries all three `/init` skeleton headings"),
        ));
    }
}

fn is_tool_verb(word: &str) -> bool {
    TOOL_VERBS.iter().any(|v| word.eq_ignore_ascii_case(v))
}

/// The first tool action a prose line's rule governs, as written.
///
/// Tied to the modal (#1374): for each `never`, `always`, `must not` or
/// `do not` in the prose, only the clause it opens is read -- after the
/// modal, up to the next sentence break (`;`, or `.`, `!`, `?` before
/// whitespace or the end of the line). A tool verb before the modal, or
/// in another sentence, is not the rule's verb.
///
/// Inside the clause, inline code spans are blanked for the words: what
/// is in one is somebody's syntax (a config value, a worker-pool spec),
/// not a word of the rule. Each word or hyphenated compound counts only
/// when every part of it is a tool verb, so `force-push` is the action
/// itself while `slow-write` and `write-ahead` name something else. A
/// word directly after `@` is a tag or an address (`@write`), not an
/// action; one directly after a determiner, quantifier or possessive
/// (`any edit`, `the commit`, `their push`) is a noun. Then a code span
/// in the clause counts when it is itself a command (#1322).
fn tool_action(line: &str) -> Option<String> {
    let prose = text::blank_spans(line);
    for modal in MODAL.find_iter(&prose) {
        let end = clause_end(&prose, modal.end());
        let clause = &prose[modal.end()..end];
        let mut previous: Option<&str> = None;
        for m in COMPOUND.find_iter(clause) {
            let before = &clause[..m.start()];
            let word = m.as_str();
            let noun = previous.is_some_and(is_noun_marker) || is_possessive(before);
            if !before.ends_with('@') && !noun && word.split('-').all(is_tool_verb) {
                return Some(word.to_string());
            }
            previous = Some(word);
        }
        // `blank_spans` keeps char positions, not byte positions, so the
        // clause is cut from the original line by chars. A break is never
        // inside a span (a span is blank in `prose`), so the cut holds
        // whole spans only.
        let from = prose[..modal.end()].chars().count();
        let to = prose[..end].chars().count();
        let original: String = line.chars().skip(from).take(to - from).collect();
        if let Some(action) = text::spans(&original)
            .iter()
            .find_map(|s| command_action(&s.text))
        {
            return Some(action);
        }
    }
    None
}

/// The byte where the clause starting at `from` ends: the next `;`, or
/// `.`, `!` or `?` followed by whitespace or the end of the line.
fn clause_end(prose: &str, from: usize) -> usize {
    let rest = &prose[from..];
    let mut chars = rest.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let at_break = match c {
            ';' => true,
            '.' | '!' | '?' => chars.peek().is_none_or(|(_, n)| n.is_whitespace()),
            _ => false,
        };
        if at_break {
            return from + i;
        }
    }
    prose.len()
}

/// Whether the text before a word ends in a possessive (`user's`,
/// `users'`), straight or curly apostrophe.
fn is_possessive(before: &str) -> bool {
    let b = before.trim_end();
    ["'s", "\u{2019}s", "s'", "s\u{2019}"]
        .iter()
        .any(|p| b.ends_with(p))
}

/// A word after which a tool verb is a noun: a determiner, a quantifier
/// or a possessive pronoun. A possessive `'s` is [`is_possessive`].
fn is_noun_marker(word: &str) -> bool {
    const MARKERS: [&str; 17] = [
        "a", "an", "any", "the", "each", "every", "this", "that", "no", "some", "my", "your",
        "his", "her", "its", "our", "their",
    ];
    MARKERS.iter().any(|m| word.eq_ignore_ascii_case(m))
}

/// The tool action a code span performs when it is a shell command
/// that is one: `git <verb> …` names the git verb, and `rm …` names
/// `rm`. Structural, by the span's first words only; any other span
/// (`8 write`, `@write`) names nothing.
fn command_action(span: &str) -> Option<String> {
    let mut words = span.split_whitespace();
    match words.next()? {
        "rm" => Some("rm".to_string()),
        "git" => words
            .next()
            .filter(|verb| is_tool_verb(verb))
            .map(str::to_string),
        _ => None,
    }
}

/// A placeholder rather than a value: `xxx`, `your`, `example`, or a tail
/// of one or two distinct characters (`sk-ant-aaaaaaaaaaaaaaaaaaaa`).
fn is_placeholder(token: &str, prefix: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    if ["xxx", "your", "example", "placeholder", "redacted"]
        .iter()
        .any(|p| lower.contains(*p))
    {
        return true;
    }
    let tail = token.strip_prefix(prefix).unwrap_or(token);
    let mut distinct: Vec<char> = tail.chars().collect();
    distinct.sort_unstable();
    distinct.dedup();
    !tail.is_empty() && distinct.len() <= 2
}

/// The first `max` characters, with an ellipsis when cut.
fn clamp(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// The values one file names in one conflict class, each with the first
/// prose line naming it.
type Named = Vec<(String, usize)>;

/// What one launch-set file names in each conflict class.
#[derive(Default)]
struct Names {
    package_managers: Named,
    lint_entries: Named,
    default_branches: Named,
}

fn names(text: &str) -> Names {
    let mut out = Names::default();
    let add = |into: &mut Named, value: &str, line: usize| {
        if !into.iter().any(|(v, _)| v == value) {
            into.push((value.to_string(), line));
        }
    };
    for (n, line) in text::prose_lines(text) {
        for m in PACKAGE_MANAGER.find_iter(line) {
            add(&mut out.package_managers, m.as_str(), n);
        }
        for m in LINT_ENTRY.find_iter(line) {
            add(&mut out.lint_entries, m.as_str(), n);
        }
        if DEFAULT_BRANCH.is_match(line) {
            if let Some(m) = BRANCH_NAME.find(line) {
                add(&mut out.default_branches, m.as_str(), n);
            }
        }
    }
    out
}

/// The narrow conflict class: two launch-set files whose named values
/// in one class are both non-empty and share nothing. Broader conflicts
/// are judgement, and are not detected.
fn conflicts(cx: &Context, loaded: &[Loaded], out: &mut Vec<Finding>) {
    let launch: Vec<(&Loaded, Names)> = loaded
        .iter()
        .filter(|l| l.launch)
        .map(|l| (l, names(&l.text)))
        .collect();
    for (i, (a, an)) in launch.iter().enumerate() {
        for (b, bn) in &launch[i + 1..] {
            let classes: [(&str, &Named, &Named); 3] = [
                (
                    "package managers",
                    &an.package_managers,
                    &bn.package_managers,
                ),
                ("lint entry points", &an.lint_entries, &bn.lint_entries),
                (
                    "default branches",
                    &an.default_branches,
                    &bn.default_branches,
                ),
            ];
            for (class, av, bv) in classes {
                if av.is_empty() || bv.is_empty() {
                    continue;
                }
                if av.iter().any(|(v, _)| bv.iter().any(|(w, _)| v == w)) {
                    continue;
                }
                let (va, la) = &av[0];
                let (vb, lb) = &bv[0];
                out.push(emit(
                    Rule::Conflict,
                    Severity::Advice,
                    // The later-loaded file: "instructions closer to
                    // where you launched Claude are read last".
                    subject(b),
                    vec![
                        evidence(&a.file.path, Some(*la), format!("names `{va}`")),
                        evidence(&b.file.path, Some(*lb), format!("names `{vb}`")),
                    ],
                    format!(
                        "`{}` and `{}` name different {class}: `{va}` and `{vb}`",
                        label(&a.file.path, cx.repo),
                        label(&b.file.path, cx.repo)
                    ),
                ));
            }
        }
    }
}

/// `AGENTS.md` beside a repo-scope CLAUDE.md that does not import it.
fn agents_md(cx: &Context, loaded: &[Loaded], out: &mut Vec<Finding>) {
    for l in loaded.iter().filter(|l| l.scope == Scope::Repo) {
        let file = Path::new(&l.file.path);
        let Some(dir) = file.parent() else {
            continue;
        };
        // `.claude/CLAUDE.md` sits one level below the AGENTS.md it
        // would be beside.
        let mut candidates = vec![dir.join("AGENTS.md")];
        if dir.file_name().is_some_and(|n| n == ".claude") {
            if let Some(up) = dir.parent() {
                candidates.push(up.join("AGENTS.md"));
            }
        }
        let Some(agents) = candidates.into_iter().find(|p| p.is_file()) else {
            continue;
        };
        let target = agents.canonicalize().unwrap_or_else(|_| agents.clone());
        let imported = parse_imports(&l.text).iter().any(|raw| {
            let p = if raw.starts_with('/') {
                PathBuf::from(raw)
            } else {
                dir.join(raw)
            };
            p.canonicalize().is_ok_and(|c| c == target)
        });
        if imported {
            continue;
        }
        let name = label(&l.file.path, cx.repo);
        let mut ev = vec![evidence(
            &agents.to_string_lossy(),
            None,
            format!("exists beside `{name}`, whose imports do not resolve to it"),
        )];
        for (n, line) in text::prose_lines(&l.text) {
            if line.contains("AGENTS.md") {
                ev.push(evidence(
                    &l.file.path,
                    Some(n),
                    "names AGENTS.md in prose".to_string(),
                ));
            }
        }
        out.push(emit(
            Rule::AgentsMd,
            Severity::Advice,
            subject(l),
            ev,
            format!(
                "`AGENTS.md` sits beside `{name}`, which does not import it; Claude Code reads \
                 only the CLAUDE.md"
            ),
        ));
    }
}

/// `CLAUDE.local.md` not git-ignored, by `git check-ignore`.
///
/// Git runs only when a local file exists, so the common case spawns
/// nothing. A git that could not run or did not answer is `Err`: the
/// check is Unknown. A git that ran and exited with anything but 0 or 1
/// (128: not a repository) is a per-file Unknown finding with git's own
/// words, and the other rules' findings stand.
fn local_ignored(
    cx: &Context,
    git: &Path,
    loaded: &[Loaded],
    out: &mut Vec<Finding>,
) -> Result<(), String> {
    for l in loaded.iter().filter(|l| l.scope == Scope::Local) {
        let rel = label(&l.file.path, cx.repo);
        let output = crate::worktrees::scan::git_output_with(
            git,
            cx.repo,
            &["check-ignore", "-q", "--", rel.as_str()],
        )
        .map_err(|e| format!("git check-ignore could not run: {e}"))?;
        match output.status.code() {
            Some(0) => {}
            Some(1) => out.push(emit(
                Rule::LocalNotIgnored,
                Severity::Problem,
                subject(l),
                vec![evidence(
                    &l.file.path,
                    None,
                    "`git check-ignore` exit status 1: not ignored".to_string(),
                )],
                format!("`{rel}` is not ignored by git in this repository"),
            )),
            code => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                out.push(emit(
                    Rule::LocalNotIgnored,
                    Severity::Unknown,
                    subject(l),
                    vec![evidence(
                        &l.file.path,
                        None,
                        format!(
                            "`git check-ignore` exit status {}: {stderr}",
                            code.map(|c| c.to_string()).unwrap_or_else(|| "none".into())
                        ),
                    )],
                    format!("`{rel}` could not be checked against `.gitignore`"),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claudemd::scan_effective_opt;
    use std::fs;
    use std::process::Command;

    /// A home and a repo under one tempdir; the home is passed
    /// explicitly, never via `$HOME`.
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let repo = t.path().join("repo");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(&repo).unwrap();
        (t, home, repo)
    }

    /// This producer's findings over a repository, with every one
    /// checked to carry its rule so the brief's suggestion is specific.
    fn shape(repo: &Path, home: Option<&Path>) -> Vec<Finding> {
        let scan = scan_effective_opt(repo, home);
        let cx = Context {
            repo,
            home,
            scan: &scan,
            definitions: None,
            conn: None,
        };
        let found = Shape.run(&cx).expect("the check runs");
        for f in &found {
            assert_eq!(f.check, Check::Shape);
            assert!(f.rule.is_some(), "a shape finding without a rule: {f:?}");
            assert!(
                !f.brief.contains("as the finding states"),
                "the generic suggestion is never used: {}",
                f.brief
            );
        }
        found
    }

    fn by_rule(found: &[Finding], rule: Rule) -> Vec<&Finding> {
        found.iter().filter(|f| f.rule == Some(rule.id())).collect()
    }

    fn lines(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    /// 201 lines fires, 200 does not, and the sentence states the docs'
    /// target and the est. cost, never an adherence claim: the one
    /// controlled study could not detect one.
    #[test]
    fn a_file_over_two_hundred_lines_quotes_the_docs_not_an_adherence_claim() {
        let (_t, _home, repo) = fixture();
        fs::write(repo.join("CLAUDE.md"), lines(200)).unwrap();
        assert!(by_rule(&shape(&repo, None), Rule::LineTarget).is_empty());

        fs::write(repo.join("CLAUDE.md"), lines(201)).unwrap();
        let found = shape(&repo, None);
        let hits = by_rule(&found, Rule::LineTarget);
        assert_eq!(hits.len(), 1, "{found:?}");
        let f = hits[0];
        assert_eq!(f.severity, Severity::Advice);
        assert!(f.finding.contains("201 lines"), "{}", f.finding);
        assert!(f.finding.contains("target is under 200"), "{}", f.finding);
        assert!(f.finding.contains("est."), "{}", f.finding);
        assert_eq!(
            f.evidence[0].at,
            Locator::File {
                path: repo.join("CLAUDE.md").to_string_lossy().to_string(),
                line: Some(201)
            }
        );
        for text in [&f.finding, &f.brief] {
            assert!(!text.contains("adherence will"), "{text}");
            assert!(!text.contains("ignore"), "{text}");
        }
        assert!(
            f.brief.contains("60 lines"),
            "the range is named: {}",
            f.brief
        );
        assert!(f.brief.contains("500"), "{}", f.brief);
    }

    /// The row lists the global file, the root file and its import, not
    /// the subdirectory file, with lines and est. tokens each; exact
    /// when the scan is whole, "at least" when it is not.
    #[test]
    fn the_launch_set_row_lists_every_file_loaded_before_the_first_prompt() {
        let (_t, home, repo) = fixture();
        fs::write(home.join(".claude").join("CLAUDE.md"), "global\nrules\n").unwrap();
        fs::write(repo.join("CLAUDE.md"), "@./shared.md\nroot\n").unwrap();
        fs::write(repo.join("shared.md"), "a\nb\nc\n").unwrap();
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src").join("CLAUDE.md"), "NEVER do this\nlazy\n").unwrap();

        let found = shape(&repo, Some(&home));
        let rows = by_rule(&found, Rule::LaunchSet);
        assert_eq!(rows.len(), 1, "one row per report: {found:?}");
        let row = rows[0];
        assert_eq!(row.severity, Severity::Advice);
        let expected = crate::claudemd::tokens::estimate("global\nrules\n")
            + crate::claudemd::tokens::estimate("@./shared.md\nroot\n")
            + crate::claudemd::tokens::estimate("a\nb\nc\n");
        assert_eq!(
            row.finding,
            format!("3 files, 7 lines and est. {expected} tokens load before the first prompt"),
        );
        assert_eq!(row.evidence.len(), 3, "{:?}", row.evidence);
        let paths: Vec<&str> = row
            .evidence
            .iter()
            .map(|e| match &e.at {
                Locator::File { path, line: None } => path.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(
            Path::new(paths[0]).ends_with("home/.claude/CLAUDE.md"),
            "global first: {paths:?}"
        );
        assert!(Path::new(paths[1]).ends_with("repo/CLAUDE.md"), "{paths:?}");
        assert!(
            paths[2].ends_with("shared.md"),
            "then its import: {paths:?}"
        );
        assert!(
            row.evidence[2].measured.contains("imported by"),
            "{:?}",
            row.evidence[2]
        );
        assert!(
            !paths.iter().any(|p| p.contains("src")),
            "a subdirectory file loads only after a Read there: {paths:?}"
        );
        assert!(
            row.evidence[1].measured.starts_with("2 lines, est. "),
            "{:?}",
            row.evidence[1]
        );
        assert!(
            matches!(row.subject, Subject::Directory { .. }),
            "{:?}",
            row.subject
        );
        // The subdirectory file's one NEVER is not an emphasis finding
        // (that needs two lines), and its placement is another
        // producer's rule.
        assert!(by_rule(&found, Rule::Emphasis).is_empty(), "{found:?}");

        // No home: the global scope could not be looked for, so the row
        // is a floor.
        let floor = shape(&repo, None);
        let row = &by_rule(&floor, Rule::LaunchSet)[0];
        assert!(
            row.finding.starts_with("at least 2 files"),
            "{}",
            row.finding
        );
    }

    /// A repository whose only CLAUDE.md files sit in subdirectories
    /// loads nothing at launch, and the row says so rather than "0".
    #[test]
    fn a_repository_with_only_subdirectory_files_loads_nothing_at_launch() {
        let (_t, _home, repo) = fixture();
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src").join("CLAUDE.md"), "lazy\n").unwrap();
        let found = shape(&repo, None);
        let row = &by_rule(&found, Rule::LaunchSet)[0];
        assert!(
            row.finding
                .starts_with("no CLAUDE.md loads before the first prompt"),
            "{}",
            row.finding
        );
        assert!(
            row.finding.contains("1 under subdirectories"),
            "{}",
            row.finding
        );
        assert!(row.evidence.is_empty());
    }

    /// The hard skip is a behaviour fact: a Problem, by on-disk bytes.
    #[test]
    fn a_file_over_four_mib_is_a_problem_stating_the_skip() {
        let (_t, _home, repo) = fixture();
        let big = "x".repeat(HARD_SKIP_BYTES as usize + 1);
        fs::write(repo.join("CLAUDE.md"), &big).unwrap();
        let found = shape(&repo, None);
        let hits = by_rule(&found, Rule::HardSkip);
        assert_eq!(hits.len(), 1, "{}", found.len());
        assert_eq!(hits[0].severity, Severity::Problem);
        assert!(
            hits[0].finding.contains("skips a CLAUDE.md over 4 MiB"),
            "{}",
            hits[0].finding
        );
        assert!(hits[0].finding.contains("4.0 MiB"), "{}", hits[0].finding);

        fs::write(repo.join("CLAUDE.md"), &big[..HARD_SKIP_BYTES as usize]).unwrap();
        assert!(
            by_rule(&shape(&repo, None), Rule::HardSkip).is_empty(),
            "exactly 4 MiB loads"
        );
    }

    /// A secret inside a code block fires, masked to its prefix; a
    /// placeholder does not. The value never appears in the finding, the
    /// evidence or the brief.
    #[test]
    fn a_secret_in_a_code_block_fires_masked_and_a_placeholder_does_not() {
        let (_t, _home, repo) = fixture();
        let value = format!("sk-ant-{}", "Zq9kL2mN4pR7sT1vW3xY5bC8dF0gH6jK");
        let text = format!(
            "# Setup\n```bash\nexport ANTHROPIC_API_KEY={value}\n```\nUse `sk-ant-<your-token>` \
             or sk-ant-xxxxxxxxxxxxxxxxxxxxxxxx as a placeholder.\nKeys start with `sk-ant-`.\n"
        );
        fs::write(repo.join("CLAUDE.md"), text).unwrap();
        let found = shape(&repo, None);
        let hits = by_rule(&found, Rule::Secret);
        assert_eq!(hits.len(), 1, "{found:?}");
        let f = hits[0];
        assert_eq!(f.severity, Severity::Problem);
        assert!(f.finding.contains("line 3"), "{}", f.finding);
        assert!(f.finding.contains("`sk-ant-…`"), "{}", f.finding);
        assert!(f.finding.contains("Anthropic API key"), "{}", f.finding);
        for text in [&f.finding, &f.brief, &f.evidence[0].measured] {
            assert!(!text.contains(&value[7..]), "the value leaked: {text}");
        }
        assert_eq!(
            f.evidence[0].at,
            Locator::File {
                path: repo.join("CLAUDE.md").to_string_lossy().to_string(),
                line: Some(3)
            }
        );
    }

    /// Other secret shapes, and the placeholder forms each skips.
    #[test]
    fn other_secret_shapes_fire_and_their_placeholders_do_not() {
        let (_t, _home, repo) = fixture();
        let text = "\
token ghp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789
pat github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz
-----BEGIN RSA PRIVATE KEY-----
AKIAIOSFODNN7EXAMPLE
AKIAJ4Y6X2Q7B3K9M1P5
ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
";
        fs::write(repo.join("CLAUDE.md"), text).unwrap();
        let found = shape(&repo, None);
        let kinds: Vec<&str> = by_rule(&found, Rule::Secret)
            .iter()
            .map(|f| f.finding.split("shaped like ").nth(1).unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "a GitHub personal access token (`ghp_…`)",
                "a GitHub fine-grained token (`github_pat_…`)",
                "a private key (`-----BEGIN…`)",
                "an AWS access key id (`AKIA…`)",
            ],
            "{found:?}"
        );
    }

    /// Three IMPORTANT lines fire, one does not, and bold prose is not
    /// emphasis.
    #[test]
    fn emphasis_on_three_lines_fires_and_on_one_does_not() {
        let (_t, _home, repo) = fixture();
        fs::write(
            repo.join("CLAUDE.md"),
            "IMPORTANT: run the gate.\n**Never** skip it.\n**Always** report counts.\n",
        )
        .unwrap();
        assert!(by_rule(&shape(&repo, None), Rule::Emphasis).is_empty());

        fs::write(
            repo.join("CLAUDE.md"),
            "IMPORTANT: run the gate.\nIMPORTANT: report counts.\n```\nIMPORTANT in a fence\n```\n\
             <!-- IMPORTANT in a comment -->\nIMPORTANT: never chain them.\n",
        )
        .unwrap();
        let found = shape(&repo, None);
        let hits = by_rule(&found, Rule::Emphasis);
        assert_eq!(hits.len(), 1, "{found:?}");
        let f = hits[0];
        assert_eq!(f.severity, Severity::Advice);
        assert!(f.finding.contains("on 3 lines"), "{}", f.finding);
        let lines: Vec<u32> = f
            .evidence
            .iter()
            .map(|e| match &e.at {
                Locator::File { line: Some(n), .. } => *n,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(lines, [1, 2, 7], "the fence and the comment do not count");
        assert!(
            f.brief.contains("to that line alone"),
            "the source is quoted: {}",
            f.brief
        );
    }

    /// A never/always line with a tool verb is quoted; one without a tool
    /// verb is not a hook's business.
    #[test]
    fn a_never_rule_with_a_tool_verb_is_quoted_and_one_without_is_not() {
        let (_t, _home, repo) = fixture();
        fs::write(
            repo.join("CLAUDE.md"),
            "Never edit `.env` directly.\n`PathBuf::join`, never string concatenation.\n\
             Always push after the gate.\nAlways be kind.\n",
        )
        .unwrap();
        let found = shape(&repo, None);
        let hits = by_rule(&found, Rule::HookRule);
        let sentences: Vec<&str> = hits.iter().map(|f| f.finding.as_str()).collect();
        assert_eq!(hits.len(), 2, "{sentences:?}");
        assert!(sentences[0].contains("line 1"), "{sentences:?}");
        assert!(sentences[0].contains("(`edit`)"), "{sentences:?}");
        assert!(sentences[1].contains("line 3"), "{sentences:?}");
        assert_eq!(hits[0].evidence[0].measured, "Never edit `.env` directly.");
        assert!(
            hits[0].brief.contains("PreToolUse hook"),
            "{}",
            hits[0].brief
        );
    }

    /// #1322: a tool verb inside a code span that is not a tool-action
    /// command, directly after `@`, or in a hyphenated compound with a
    /// word that is not a tool verb names something else (a worker pool,
    /// a test tag), not a tool action.
    #[test]
    fn a_tool_verb_in_a_code_span_a_tag_or_a_compound_is_not_a_hook_rule() {
        let (_t, _home, repo) = fixture();
        fs::write(
            repo.join("CLAUDE.md"),
            "Worker pools: `8 read / 4 write / 2 slow-write`; never override them in a spec.\n\
             `@write` specs need `ALLOW_WRITES`; never set it in a local payload.\n\
             `8 write` workers; never change it.\n\
             Tag them @write; never run them locally.\n\
             The slow-write pool is never resized.\n\
             Never disable the write-ahead log.\n",
        )
        .unwrap();
        let found = shape(&repo, None);
        let hits = by_rule(&found, Rule::HookRule);
        assert!(hits.is_empty(), "{hits:?}");
    }

    /// The other direction of #1322: a verb in prose still fires beside
    /// a code span, a compound made only of tool verbs (`force-push`) is
    /// the tool action itself, a `--force` flag is not a compound, and a
    /// skipped `@write` tag does not hide a real verb later on the line.
    /// A code span that is itself a tool-action command (`git push`,
    /// `rm`) names that action.
    #[test]
    fn a_tool_verb_in_prose_or_a_verb_only_compound_still_fires() {
        let (_t, _home, repo) = fixture();
        fs::write(
            repo.join("CLAUDE.md"),
            "Never force-push to main.\n\
             Never push `--force` to main.\n\
             Never commit `.env`.\n\
             Always run the gate; never use --force.\n\
             Tag them @write; always delete scratch files.\n\
             Never `git push --force` to the default branch.\n\
             Never `rm -rf` the cache.\n",
        )
        .unwrap();
        let found = shape(&repo, None);
        let hits = by_rule(&found, Rule::HookRule);
        let sentences: Vec<&str> = hits.iter().map(|f| f.finding.as_str()).collect();
        assert_eq!(hits.len(), 7, "{sentences:?}");
        for (i, verb) in [
            "force-push",
            "push",
            "commit",
            "force",
            "delete",
            "push",
            "rm",
        ]
        .iter()
        .enumerate()
        {
            assert!(
                sentences[i].contains(&format!("line {}", i + 1)),
                "{sentences:?}"
            );
            assert!(
                sentences[i].contains(&format!("(`{verb}`)")),
                "{sentences:?}"
            );
        }
    }

    /// #1374: the tool verb must be in the clause the modal opens --
    /// after it, before a sentence break -- and not a noun after a
    /// determiner, quantifier or possessive.
    #[test]
    fn a_tool_verb_outside_the_modals_clause_or_used_as_a_noun_is_not_a_hook_rule() {
        let (_t, _home, repo) = fixture();
        fs::write(
            repo.join("CLAUDE.md"),
            "The budget check fails any loosening edit. Never loosen a baseline.\n\
             Commit early; never skip the gate.\n\
             Never revert the edit.\n\
             Never squash a commit or rewrite any push.\n\
             Always keep each write small.\n\
             Never undo the user's edit or their commit.\n\
             Do not rewrite this push.\n\
             Never use `git push` as a noun; it's `git push` the tool does.\n",
        )
        .unwrap();
        let found = shape(&repo, None);
        let hits = by_rule(&found, Rule::HookRule);
        let sentences: Vec<&str> = hits.iter().map(|f| f.finding.as_str()).collect();
        assert_eq!(hits.len(), 1, "{sentences:?}");
        assert!(
            sentences[0].contains("line 8") && sentences[0].contains("(`push`)"),
            "a span command in the modal's clause still counts: {sentences:?}"
        );
    }

    /// #1374, the other direction: the issue's firing cases, and `do not`
    /// as a modal.
    #[test]
    fn a_tool_verb_the_modal_governs_still_fires() {
        let (_t, _home, repo) = fixture();
        fs::write(
            repo.join("CLAUDE.md"),
            "Never commit `.env`.\n\
             Never force-push to main.\n\
             Always run the gate; never `git push --force`.\n\
             Do not edit generated files.\n\
             The gate is slow. You must not delete its cache.\n",
        )
        .unwrap();
        let found = shape(&repo, None);
        let hits = by_rule(&found, Rule::HookRule);
        let sentences: Vec<&str> = hits.iter().map(|f| f.finding.as_str()).collect();
        assert_eq!(hits.len(), 5, "{sentences:?}");
        for (i, verb) in ["commit", "force-push", "push", "edit", "delete"]
            .iter()
            .enumerate()
        {
            assert!(
                sentences[i].contains(&format!("line {}", i + 1))
                    && sentences[i].contains(&format!("(`{verb}`)")),
                "{sentences:?}"
            );
        }
    }

    /// Two launch-set files naming different package managers conflict;
    /// two naming the same one, or one naming both, do not.
    #[test]
    fn two_launch_files_naming_different_package_managers_conflict() {
        let (_t, home, repo) = fixture();
        fs::write(
            home.join(".claude").join("CLAUDE.md"),
            "Install with pnpm.\n",
        )
        .unwrap();
        fs::write(
            repo.join("CLAUDE.md"),
            "Use `yarn install`, and `make lint`.\n",
        )
        .unwrap();
        fs::create_dir_all(repo.join("packages")).unwrap();
        // A subdirectory file is not in the launch set, so it cannot
        // conflict here.
        fs::write(repo.join("packages").join("CLAUDE.md"), "Use bun here.\n").unwrap();

        let found = shape(&repo, Some(&home));
        let hits = by_rule(&found, Rule::Conflict);
        assert_eq!(hits.len(), 1, "{found:?}");
        let f = hits[0];
        assert!(
            f.finding.contains("different package managers"),
            "{}",
            f.finding
        );
        assert!(f.finding.contains("`pnpm` and `yarn`"), "{}", f.finding);
        assert!(
            Path::new(f.subject.path()).ends_with("repo/CLAUDE.md"),
            "the later file: {:?}",
            f.subject
        );
        assert_eq!(f.evidence.len(), 2);

        // Naming the same manager somewhere resolves it.
        fs::write(
            home.join(".claude").join("CLAUDE.md"),
            "Install with pnpm or yarn.\n",
        )
        .unwrap();
        assert!(by_rule(&shape(&repo, Some(&home)), Rule::Conflict).is_empty());

        // Lint entry points and default branches are the other two
        // classes.
        fs::write(
            home.join(".claude").join("CLAUDE.md"),
            "Run `yarn lint`. The default branch is `master`.\n",
        )
        .unwrap();
        fs::write(
            repo.join("CLAUDE.md"),
            "`make lint`, not `yarn lint`. The default branch is `main`.\n",
        )
        .unwrap();
        let found = shape(&repo, Some(&home));
        let classes: Vec<&str> = by_rule(&found, Rule::Conflict)
            .iter()
            .map(|f| {
                f.finding
                    .split("name different ")
                    .nth(1)
                    .unwrap()
                    .split(':')
                    .next()
                    .unwrap()
            })
            .collect();
        assert_eq!(
            classes,
            ["default branches"],
            "lint entries overlap on `yarn lint`: {found:?}"
        );
    }

    /// A fenced tree listing and the `/init` skeleton are content Claude
    /// can derive; a two-line tree and a partial skeleton are not called.
    #[test]
    fn a_fenced_tree_listing_and_the_init_skeleton_are_derivable() {
        let (_t, _home, repo) = fixture();
        fs::write(
            repo.join("CLAUDE.md"),
            "# Project Overview\nA thing.\n\n## Development Commands\n`make lint`\n\n\
             ## Architecture\n```\nsrc/\n├── a.rs\n├── b.rs\n└── c.rs\n```\n\n\
             ```\n├── two\n└── lines\n```\n",
        )
        .unwrap();
        let found = shape(&repo, None);
        let trees = by_rule(&found, Rule::TreeListing);
        assert_eq!(
            trees.len(),
            1,
            "the two-line block is not a tree: {found:?}"
        );
        assert!(
            trees[0].finding.contains("lines 9–12"),
            "{}",
            trees[0].finding
        );
        assert!(
            trees[0].evidence[0].measured.starts_with("3 of 4 lines"),
            "{:?}",
            trees[0].evidence
        );
        let init = by_rule(&found, Rule::InitSkeleton);
        assert_eq!(init.len(), 1, "{found:?}");
        assert_eq!(init[0].evidence.len(), 3);
        assert!(
            init[0].brief.contains("no required format"),
            "{}",
            init[0].brief
        );

        fs::write(
            repo.join("CLAUDE.md"),
            "# Project Overview\n## Architecture\n",
        )
        .unwrap();
        assert!(
            by_rule(&shape(&repo, None), Rule::InitSkeleton).is_empty(),
            "two of three"
        );
    }

    /// `AGENTS.md` beside a CLAUDE.md that does not import it is not read
    /// by Claude Code; one that imports it is fine, and a prose mention
    /// is cited as evidence, not as an import.
    #[test]
    fn agents_md_beside_a_non_importing_claude_md_is_advice() {
        let (_t, _home, repo) = fixture();
        fs::write(repo.join("AGENTS.md"), "shared agent rules").unwrap();
        fs::write(repo.join("CLAUDE.md"), "# Rules\nAlso read AGENTS.md.\n").unwrap();
        let found = shape(&repo, None);
        let hits = by_rule(&found, Rule::AgentsMd);
        assert_eq!(hits.len(), 1, "{found:?}");
        let f = hits[0];
        assert!(f.finding.contains("does not import it"), "{}", f.finding);
        assert_eq!(f.evidence.len(), 2, "{:?}", f.evidence);
        assert!(at_path(&f.evidence[0]).ends_with("AGENTS.md"));
        assert_eq!(f.evidence[1].measured, "names AGENTS.md in prose");
        assert!(f.brief.contains("@AGENTS.md"), "{}", f.brief);

        fs::write(repo.join("CLAUDE.md"), "@AGENTS.md\n# Rules\n").unwrap();
        assert!(by_rule(&shape(&repo, None), Rule::AgentsMd).is_empty());
        fs::write(repo.join("CLAUDE.md"), "@./AGENTS.md\n").unwrap();
        assert!(by_rule(&shape(&repo, None), Rule::AgentsMd).is_empty());
    }

    fn git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// `CLAUDE.local.md` with no `.gitignore` entry is a Problem; with
    /// one it is nothing; and git is consulted, not `.gitignore` parsed.
    #[test]
    fn a_local_file_not_git_ignored_is_a_problem() {
        let (_t, home, repo) = fixture();
        assert!(git(&repo, &["init", "-q"]), "git init");
        fs::write(repo.join("CLAUDE.md"), "root\n").unwrap();
        fs::write(repo.join("CLAUDE.local.md"), "mine\n").unwrap();

        let found = shape(&repo, Some(&home));
        let hits = by_rule(&found, Rule::LocalNotIgnored);
        assert_eq!(hits.len(), 1, "{found:?}");
        assert_eq!(hits[0].severity, Severity::Problem);
        assert!(
            hits[0].finding.contains("is not ignored by git"),
            "{}",
            hits[0].finding
        );
        assert!(hits[0].brief.contains(".gitignore"), "{}", hits[0].brief);

        fs::write(repo.join(".gitignore"), "CLAUDE.local.md\n").unwrap();
        assert!(by_rule(&shape(&repo, Some(&home)), Rule::LocalNotIgnored).is_empty());

        // The local file is in the launch set, whatever git says.
        let again = shape(&repo, Some(&home));
        let row = &by_rule(&again, Rule::LaunchSet)[0];
        assert!(
            row.evidence.iter().any(|e| e.measured.ends_with("(local)")),
            "{:?}",
            row.evidence
        );
    }

    /// A git that could not run makes the whole check Unknown, in git's
    /// own words; it does not make the local file look ignored.
    #[test]
    fn a_missing_git_binary_makes_the_check_unknown() {
        let (_t, home, repo) = fixture();
        fs::write(repo.join("CLAUDE.local.md"), "mine\n").unwrap();
        let scan = scan_effective_opt(&repo, Some(&home));
        let cx = Context {
            repo: &repo,
            home: Some(&home),
            scan: &scan,
            definitions: None,
            conn: None,
        };
        let err = run_with_git(&cx, Path::new("/home/octocat/no-such-git"))
            .expect_err("a git that cannot run is not a git that said yes");
        assert!(err.starts_with("git check-ignore could not run:"), "{err}");
    }

    /// Outside a git repository the rule cannot decide, and says so per
    /// file rather than calling the file ignored or not.
    #[test]
    fn a_local_file_outside_a_git_repository_is_unknown_per_file() {
        let (_t, home, repo) = fixture();
        fs::write(repo.join("CLAUDE.local.md"), "mine\n").unwrap();
        let found = shape(&repo, Some(&home));
        let hits = by_rule(&found, Rule::LocalNotIgnored);
        assert_eq!(hits.len(), 1, "{found:?}");
        assert_eq!(hits[0].severity, Severity::Unknown);
        assert!(
            hits[0].finding.contains("could not be checked"),
            "{}",
            hits[0].finding
        );
        assert!(
            hits[0].evidence[0].measured.contains("128"),
            "{:?}",
            hits[0].evidence
        );
    }

    /// A file the scan read and this producer cannot is Unknown, never
    /// clean. `#[cfg(unix)]`: `chmod 000` is the mechanism, and Windows
    /// does not honour it. Run under `capsh --drop=cap_dac_override` as
    /// root, or root reads the file regardless.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_makes_the_check_unknown_not_clean() {
        use std::os::unix::fs::PermissionsExt;
        let (_t, _home, repo) = fixture();
        let file = repo.join("CLAUDE.md");
        fs::write(&file, "root\n").unwrap();
        let scan = scan_effective_opt(&repo, None);
        assert_eq!(scan.repo.files.len(), 1, "the scan read it");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

        let cx = Context {
            repo: &repo,
            home: None,
            scan: &scan,
            definitions: None,
            conn: None,
        };
        let got = Shape.run(&cx);

        // Restored before any assertion can panic.
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();

        let err = got.expect_err("an unreadable input is Unknown, not zero findings");
        assert!(err.contains("could not be read"), "{err}");
        assert!(err.contains("CLAUDE.md"), "{err}");
    }

    /// Every rule id round-trips, and every rule's suggestion names the
    /// subject, so an agent handed the brief alone knows what to open.
    #[test]
    fn every_rule_round_trips_and_suggests_against_its_subject() {
        for rule in Rule::ALL {
            assert_eq!(Rule::from_id(rule.id()), Some(*rule));
            let f = emit(
                *rule,
                Severity::Advice,
                Subject::ClaudeMd {
                    path: "/home/octocat/hello-world/CLAUDE.md".into(),
                    scope: Scope::Repo,
                    section: None,
                },
                vec![],
                "a sentence".into(),
            );
            assert_eq!(f.rule, Some(rule.id()));
            assert!(f.brief.contains("Suggested change: "), "{}", f.brief);
            assert!(
                !f.brief.contains("as the finding states"),
                "{rule:?}: {}",
                f.brief
            );
        }
        assert_eq!(Rule::ALL.len(), 11, "one per row in the module docs' table");
        // Every guidance line is attributed.
        for (text, source) in GUIDANCE {
            assert!(!text.is_empty() && !source.is_empty());
        }
    }

    #[test]
    fn digits_are_grouped_in_threes() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(6400), "6,400");
        assert_eq!(grouped(1234567), "1,234,567");
    }

    fn at_path(e: &Evidence) -> String {
        match &e.at {
            Locator::File { path, .. } => path.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// The figure in the module docs, re-taken: this repository's own
    /// files. Prints every finding; run with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn this_repository_measured() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let found = shape(&repo, None);
        println!("{} finding(s) over {}", found.len(), repo.display());
        for f in &found {
            println!("- [{:?}] {}", f.severity, f.finding);
            for e in &f.evidence {
                println!("    {:?}: {}", e.at, e.measured);
            }
        }
    }
}
