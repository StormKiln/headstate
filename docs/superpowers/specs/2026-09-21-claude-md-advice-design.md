# CLAUDE.md Advice Design

**Date:** 2026-09-21
**Status:** Proposed for Headstate 7.1 (epic #1255; sub-issues #1256–#1263)

Written as a design for the 7.1 epic *CLAUDE.md and skills, from inventory
to advice*. Each numbered producer below is one sub-issue and one pull
request; the model, command and panel are the first. *Sequencing* at the end
says what lands before what, and why.

## Problem

The CLAUDE.md page is an inventory. It lists every CLAUDE.md the walk found,
its estimated tokens, the tree it imports, and what could not be read
(`src-tauri/src/claudemd/mod.rs`; #846, #972, #1131, #1236). It answers "what
is here and what does it cost". The question a user opens it with is
different: **is this the right content, in the right place, and what is
missing?**

Nothing in the app reads the *content* of a CLAUDE.md for anything but
`@import` lines. The only check that exists is
`confighealth::Check::ClaudeMdImport` (broken or circular imports). Measured
on this repository's own three files: the Makefile has 28 targets, the three
files name `make lint`, `make test-mobile`, `cargo fmt`, `cargo test --lib`,
`yarn lint` and `yarn vitest run`, and nothing names how to build, run or
deploy. The root file cites `claude/sessions.rs:444` for a sentence that now
sits at line 436. The `check-runs?filter=all` command is written out three
times across a CLAUDE.md and two skills. None of that is visible on the page.

Skills are in the same state one page over: `definitions.rs` inventories
every skill, subagent and slash command across user, project and plugin
scope (#1129, #1215) and checks nothing about them.

## Decisions

The questions that shaped this design, and their answers.

**One model, one command, one panel.** Seven producers emit findings. They
share one Rust type, run under one Tauri command, and render in one panel on
the CLAUDE.md page. A producer never ships its own panel. This is the seam
the `epic` skill warns about: #1038 built a table and #1039 built its host,
both green, nothing mounted. Here the panel lands first with a seed producer
that computes nothing new, so every later producer plugs into a host that
already renders.

**Advice is not a config-health check.** `confighealth.rs:16-30` states its
rule: "Findings are CHECKS, never opinions … no heuristic, no style
judgement". A text match for `make test` is a heuristic by construction. So
the advice model is a sibling of `confighealth`, borrows its shape
(`Verdict`, `Severity`, a verbatim `proof`), and adds a third severity.
`confighealth` stays what it is.

**Deterministic only.** No producer runs Claude over repository content.
#1198 records why: prompts built from repository content are
attacker-influenceable, and an app that spawns an agent on the user's behalf
changes what kind of app it is. Every signal here is a string or structure
test over files and JSONL records, and every finding's evidence is a file
and a line, or a session and a record index.

**Read-only, with the edit handed off.** Headstate never writes to a
CLAUDE.md or a skill. Each finding carries a *brief*: markdown an agent can
be given, in the shape `packages::markdown` already renders for outdated
dependencies, ending with the `assess.rs` idiom "Change only the file named
above. Show me the diff and let me decide."

**Three states, never collapsed.** A check not yet run is a skeleton. A check
that ran and could not decide is `Unknown` with the producer's own reason. A
check that ran clean says "checked, nothing found". `src/CLAUDE.md` names the
defect (#1042) that collapsing them caused.

**Findings state facts, not judgements.** The placement producer says
"names only paths under `src-tauri/`", never "belongs in". The rot producer
says "does not exist in this repository", never "is stale". A heuristic knows
where a section points; it does not know where a rule applies.

## Non-goals

- **Writing to a CLAUDE.md or a skill.** The ownership ledger #1199 built for
  permission rules would have to be rebuilt for prose, and prose has no
  stable key to own.
- **Running Claude to produce advice.** #1198, above.
- **Re-implementing Claude Code's precedence rules** for which CLAUDE.md or
  skill wins. `definitions.rs` shows collisions rather than resolving them,
  for the reason in its header; the same holds here.
- **A confident token count.** Estimates stay labelled "est." and floors stay
  "at least". Chars/4 is the whole method (`claudemd/tokens.rs`).
- **A score or grade.** `ClaudeCoveragePanel.tsx`'s header says why: qualify,
  or suppress.
- **A machine-wide sweep.** One repository per call. `confighealth` owns the
  sweep, and its CLAUDE.md walk cost 4.7 s over 39 repositories before #1236.
- **`.claude/rules/` and `AGENTS.md`.** Both change what "covered" means.
  Deferred, and the module docs say so, so the omission is deliberate.

## Architecture

```
src-tauri/src
├── claudemd/
│   ├── mod.rs              unchanged scan; `pub mod advice; pub mod text;`
│   ├── text.rs             NEW shared parsing: fence-aware line walker, sections, backtick spans
│   ├── refs.rs             NEW reference extraction from a section (path, path:line, make, yarn, skill, symbol)
│   └── advice/
│       ├── mod.rs          Check, Severity, Subject, Locator, Evidence, Finding, CheckRun, Report, PRODUCERS, run()
│       ├── brief.rs        brief::render(&Finding) and Report::brief; no wildcard arm on Check
│       ├── imports.rs      seed producer: Check::Imports from ImportNode.problem
│       ├── toolchain.rs    1. toolchain coverage
│       ├── transcripts.rs  2. transcript-derived advice
│       ├── gaps.rs         3. missing subdirectory CLAUDE.md
│       ├── placement.rs    4. content in the wrong file
│       ├── rot.rs          5. rot
│       ├── skills.rs       6. skills alongside CLAUDE.md
│       └── shape.rs        7. content-shape rules distilled from external practice
├── packages/scripts.rs     NEW shared Makefile-target and package.json-script parser
├── commands.rs             claude_md_advice(repoPath) -> Report; spawn_blocking
├── remote/surface.rs       ("claude_md_advice", Class::Read) + dispatch arm
└── invariants.rs           no `_ =>` arm in brief::render's match on Check

src-mobile/src/surface.rs   the same row, same position

src
├── types/pr.ts             ClaudeMdAdviceReport, ClaudeMdAdviceFinding, ClaudeMdAdviceCheck, ClaudeMdAdviceCoverage
├── api/tauri.ts            claudeMdAdvice(repoPath) wrapper; row in transport.test.ts
├── api/hooks.ts            useClaudeMdAdvice(repoPath, enabled)
└── components/
    ├── ClaudeMdAdvicePanel.tsx   the panel; collapsed by default; copy brief
    └── ClaudeMdPage.tsx          mounts the panel in the rail below the combined-total line
```

## The model

`src-tauri/src/claudemd/advice/mod.rs`, every type `#[serde(rename_all =
"camelCase")]` like `claudemd::Scope`:

```rust
/// One per producer. `ALL` is asserted against the enum's source so a
/// variant cannot be added without an entry (the "derived, not
/// enumerated" rule in invariants.rs).
pub enum Check { Imports, Toolchain, Transcripts, Gaps, Placement, Rot, Skills, Shape }

/// Ranked worst-first like confighealth::Verdict. Unknown ranks ABOVE
/// nothing, never among clean.
pub enum Severity { Problem, Advice, Unknown }

/// What the finding is about. An enum because the skills producer's
/// subject is not a CLAUDE.md.
#[serde(tag = "kind")]
pub enum Subject {
    ClaudeMd { path: String, scope: Scope, section: Option<String> },
    Directory { path: String },
    Skill { path: String, name: String },
}

/// Where the evidence is. A session locator carries a record index, never
/// user text (see Privacy).
#[serde(tag = "kind")]
pub enum Locator {
    File { path: String, line: Option<u32> },
    Session { session_id: String, record: Option<u64> },
}

pub struct Evidence { pub at: Locator, pub measured: String }

pub struct Finding {
    pub check: Check,
    pub severity: Severity,
    pub subject: Subject,
    pub evidence: Vec<Evidence>,
    /// One sentence, a fact. Rendered as the row.
    pub finding: String,
    /// Markdown for an agent. Rendered by brief::render at construction.
    pub brief: String,
}

/// Per-check coverage. There is NO Pending variant: pending is the
/// absence of a Report, the `null`-means-not-computed rule
/// AllRepositoriesTable states.
#[serde(tag = "state")]
pub enum CheckRun { Ran { findings: usize }, Unknown { reason: String } }

pub struct CheckCoverage { pub check: Check, pub run: CheckRun }

pub struct Report {
    pub repo: String,
    /// In the backend's rank order. The frontend never re-sorts.
    pub findings: Vec<Finding>,
    /// Derived from Check::ALL, so every check appears exactly once.
    pub checks: Vec<CheckCoverage>,
    /// Every brief, plus a `_Could not check: {reason}_` line per Unknown.
    pub brief: String,
}
```

Each producer implements one trait:

```rust
pub trait Producer {
    fn check(&self) -> Check;
    /// Err becomes CheckRun::Unknown { reason }. One producer failing never
    /// rejects the command (partial is not nothing, #1044).
    fn run(&self, cx: &Context) -> Result<Vec<Finding>, String>;
}

/// What every producer may read. Built once per run so the CLAUDE.md walk
/// and the definitions inventory happen once, not once per producer
/// (#1246 is what a second walk costs).
pub struct Context<'a> {
    pub repo: &'a Path,
    pub home: &'a Path,
    pub scan: &'a EffectiveScan,
    pub definitions: &'a Inventory,
    pub conn: Option<&'a Connection>,
}
```

The TypeScript mirror lives beside `ClaudeMdEffectiveScan` in
`src/types/pr.ts` with the same field names. **The frontend recomputes
nothing.** `combinedTokens` and `combinedPartial` in `ClaudeMdPage.tsx` are
the counter-example: hand-mirrored from `EffectiveScan`, kept in step by
comment. Here the counts, the order and both briefs come off the wire, and
the panel maps over `report.checks` and `report.findings` without filtering,
sorting or concatenating.

## The command

`claude_md_advice(repoPath) -> Result<Report, String>`. `async`, one
`spawn_blocking`, like `claude_md_effective`. `Class::Read` in both surface
tables: it reads local disk and writes only Headstate's own cache. Home is a
parameter of the inner function for `confighealth::check_repo`'s reason: a
test that changes `$HOME` races every other test in the binary.

Inside: build the `Context` once, then run `PRODUCERS` in order. No
`tokio::time::timeout` anywhere on the path: it drops the future and
everything it owns (#1044). A producer that needs a bound (transcripts) bounds
itself and reports what it covered.

One command rather than one per check. Each command costs five wiring points
twice over for the phone, and per-check results make "grouped by file" a
frontend join, which is the `combinedTokens` mirror again. The CLAUDE.md walk
for one repository is a fraction of a second after #1236. The transcript
producer is the unmeasured one; its section says what to measure, and the
escape hatch, if it proves slow, is a `checks: Option<Vec<Check>>` argument on
this same command, never a second command.

Frontend: `useClaudeMdAdvice(repoPath, enabled)` beside `useClaudeMdEffective`
with `retry: false`, `staleTime: 30_000`, and `enabled` only while the panel
is open, so the page's file list and content pane never wait on advice.

## The panel and the brief

The panel goes in the rail of `ClaudeMdPage.tsx`, below the combined-total
line, collapsed by default like `ConfigHealthPanel`. On a phone the rail is
the list screen, so the panel is reachable without a third screen. A finding
whose subject is a file is a button that calls `setSelected(path)`, which on a
phone navigates to the file screen by the rule already in the file. Nothing in
the panel has a fixed width; the 384px rail at 390px is the lesson recorded at
`ClaudeMdPage.tsx:197-203`.

Three renders:

1. **Query in flight.** Skeleton rows under "Checking…". Never "no advice".
2. **Report with an Unknown check.** `PartialScanNotice` with
   `unreadable = ["<check>: <reason>"]` and the consequence "the N findings
   below are at least the findings; K of M checks could not run", plus each
   Unknown check listed in amber as "could not check". The reason is the
   producer's own sentence, with no "so that…" tail (`src/CLAUDE.md`, "A
   warning states a fact").
3. **Report, every check ran, zero findings.** "M checks ran; nothing found."
   Reachable only when every `run.state === "ran"`, ordered after the partial
   arm, the rule at `ClaudeMdPage.tsx:170-191`.

A rejected command is `QueryError` with a retry: the whole run failed, which
is a different thing from a run that came back short.

Each row shows the one-line finding, its evidence, severity in text as well
as colour, and a "Copy brief" button. File-subject buttons carry
`aria-current={current(...)}`; "Copy brief" is a plain button. The brief is
not rendered inline: it is for an agent, not the reader.

The brief per finding:

```markdown
## <one-line finding>
Subject: `src-tauri/CLAUDE.md`, section `## Platform`
Evidence: `src-tauri/CLAUDE.md:38` — 4 of 4 bullets name paths under `src-tauri/`
Suggested change: <what to move, add or delete, naming the target file>
Change only the file named above. Show me the diff and let me decide.
```

Copy is `copyText(finding.brief)` then a toast, the `PathMenu` shape already
in the page. "Copy all briefs" copies `report.brief`.

**Starting a session with the brief** is a second, optional PR under the
panel sub-issue. #1214's launch path accepts a session id, a cwd, a model and
a permission mode, never a command string; the precedent for a prompt is
`claudify_command` building `cd '<path>' && claude '<prompt>'` with
`shell_quote`. So it would be two `Class::Local` commands,
`claude_launch_advice` and its `_preview`, that rebuild the `Report` in Rust
and hand the line to `launch_in_terminal`, gated as `ClaudeCodePage` gates its
launch control. The wire carries a repo path and two tokens, never text.

## Shared parsing

Three producers read CLAUDE.md prose and two read build manifests. One parser
each, landed with the model so no producer writes its own.

**`claudemd/text.rs`.** A fence-aware line walker in the shape of
`imports::parse_imports` (toggles on ```` ``` ```` and `~~~`), yielding:
sections split at column-0 ATX headings (`#`…`######`) with text before the
first heading as a section; inline backtick spans per line; fenced blocks with
their info string. Setext headings are deliberately unsupported: the naive
detector would turn the YAML frontmatter every `SKILL.md` opens with into an
h2. Failure modes pinned by test: `#` inside a fence is not a heading; a
4-space-indented line is not a heading.

**`claudemd/refs.rs`.** Reference extraction over spans:

| kind | rule | resolves against |
|---|---|---|
| path | `^[A-Za-z0-9_./-]+$` and (contains `/` or has a known extension or is `Makefile`/`CLAUDE.md`) | the file's directory, then the repo root, then a unique suffix match; two matches is `Unknown` |
| `path:line` | path followed by `:(\d+)` | the file, then its line count |
| make target | `^make ([A-Za-z0-9_-]+)$` | `packages::scripts::targets` |
| yarn/npm script | `^yarn (\S+)`, `^npm run (\S+)` | `scripts`, then `node_modules/.bin/<x>` as a binary |
| cargo | `^cargo ` | counted, never resolved |
| skill | the `` `X` `` skill; a `## Skills` list item | `Kind::Skill` names across every scope |
| symbol | `A::b`, `name()`, `SCREAMING_CASE` only | whole-word grep of the last segment over `src/`, `src-tauri/src/`, `src-mobile/src/` |
| `#NNNN` | counted, never resolved | needs GitHub; `claudemd` never talks to GitHub |

Bare words are never symbols: measured, `main` hits 100 files and `false`
355, so a bare word proves nothing. `-D warnings`, `\r\n` and
`format!("{}/…")` are not paths.

**`packages/scripts.rs`.** `targets(&Path) -> Result<Vec<Target>, String>`
over `Makefile`/`GNUmakefile`/`makefile` (lines matching
`^[A-Za-z0-9_.-]+:`, `.PHONY` excluded, with the line number) and
`justfile`; `scripts(&Path) -> Result<Vec<String>, String>` over
`package.json`. `Result`, because an unreadable manifest must reach the
producer as Unknown, not as an empty list.

## The producers

Each producer states its signal, its evidence, its threshold, its Unknown
case and its cost. Each is one sub-issue and one PR.

### 1. Toolchain coverage (`advice/toolchain.rs`)

**Question.** Which package managers and build systems are on disk, and does
any CLAUDE.md a session loads name a command for each of six verbs: build,
test, lint, format, run, deploy.

**Detection.** `packages::detect::projects` already finds npm, yarn, poetry,
uv, dotnet, CocoaPods, Terraform, Swift and Cargo by marker file. It cannot be
extended in place: `Ecosystem` is the Packages page's contract and
`run::check_repo` spawns a tool per variant, so `Make`, `Just` and `Go` must
not join it. The producer wraps it in its own `Toolchain` enum and adds
`Makefile`/`justfile` targets, `package.json` scripts, Cargo workspace
members, `go.mod`, `Gemfile`, Gradle, Xcode, and `pyproject.toml` owned by an
unrecognised tool. Two of `detect.rs`'s helpers swallow read errors
(`has_xcode_spm`, `has_project_file` return `false` on `read_dir` failure);
the producer reads markers through its own path that returns the error.

**"Documented" means "named".** Match only inside backtick spans and fenced
blocks whose first token is the manager (`make`, `yarn`, `npm`, `cargo`,
`just`, `go`, `bundle`, `./gradlew`, `xcodebuild`, `poetry`, `uv`, `pytest`,
`ruff`) followed by a target, script or subcommand mapped to a verb. Prose
never counts. Both directions of error are real on this very repository:
`make lint` at `CLAUDE.md:13` is an inline span, so a fenced-only matcher
would call lint undocumented; the same line names `yarn lint` to say *not*
that, so the wording is "names", never "recommends". Target-to-verb mapping is
by name (`test*`, `lint*`, `fmt|format*`, `build*`, `dev|run|start|serve`,
`deploy|release|publish`); targets that map to nothing are listed under
"other" and never counted for or against a verb.

**Finding.** `Severity::Advice`, subject the root CLAUDE.md (or `Directory`
when no CLAUDE.md exists), one per (toolchain, verb): "yarn (package.json +
yarn.lock at root) offers `test`, `lint`, `build`; none of the 3 CLAUDE.md
files read names a test command." Evidence names the manifest and line, and
the count of files and spans searched, never a percentage.

**Unknown.** A manifest that could not be read is Unknown with the io error,
not "no scripts". A *positive* stands whatever else was unreadable; a
*negative* is a finding only when the scan is complete (no unreadable scope,
directory, file or import). `skipped_dirs` qualifies nothing. No toolchain
under the walk's depth is "no build system found under depth 3", not a pass.

**Out of scope.** Running a command; checking that a named target exists
(that is rot's job); which verbs a *skill* documents (skills producer).

### 2. Transcript-derived advice (`advice/transcripts.rs`)

**Question.** In sessions that worked under this repository or one of its
subdirectories, what kept going wrong, and is it written in the CLAUDE.md
that directory loads?

**What exists.** `preview.rs:1367-1499` already parses `tool_use` (name, id,
input with `file_path` for Edit/MultiEdit/Write/Read, `command` for Bash,
`pattern`/`path` for Grep/Glob), `tool_result` (`tool_use_id`, `is_error:
Option<bool>`) and the `toolUseResult` sibling, but only for the one session
open in the preview pane. `search.rs` (#1203) indexes human text only and
skips tool calls by design. `claude_hook_event` holds `PermissionDenied` rows
with a tool name and no cwd column, though the hook payload carries one.
Nothing joins tool calls to a directory across sessions.

**Attribution.** Measured on a local corpus: `cwd` and `gitBranch` are on
every `user`, `assistant` and `attachment` record, and on every subagent
record with `agentId`. Whether the directory still exists (83% do not, #919)
is irrelevant: the recorded string prefix-matches the selected repo. An agent
worktree cwd `<repo>/.claude/worktrees/agent-<id>` re-roots by stripping the
last three components, the shape `subagent::Kind::classify` already matches.
A tool call's absolute `file_path` places a finding in a *subdirectory's*
CLAUDE.md: the deepest CLAUDE.md-bearing ancestor from the tree
`scan_repo` already walks. The project slug is lossy (`/` becomes `-`) and is
a pre-filter only, never evidence. Session selection is one SQL query over
`claude_session.cwd`.

**Signals.** All deterministic; `is_error` absent is not `false`; a count is
distinct sessions, never records.

| # | signal | detection | finding when | dedup key |
|---|---|---|---|---|
| S1 | corrected command | Bash A with `is_error:true` (or stderr and no stdout), then Bash B in the same session whose first two tokens differ and share ≥1 non-flag token | same A-head → B-head in ≥2 sessions | B-head |
| S2 | user correction | a `user` text record whose first 6 words match a fixed negation list, within 5 records of an assistant `tool_use` | ≥2 sessions correcting the same tool + head/path | the tool's command head or file path |
| S3 | denied tool call | `tool_result` error text matching Claude Code's denial phrasing, plus `claude_hook_event` PermissionDenied rows joined by session | same tool + head in ≥3 sessions | head |
| S4 | repeated search | identical Grep/Glob pattern, or a `Read.file_path` within the first 10 tool calls | ≥3 sessions | pattern / path |
| S5 | repeated error | `is_error:true` text normalised (digits, hex ids, absolute paths, `\r\n` stripped) | identical in ≥3 sessions | first 80 chars |
| S6 | task census | `claude_session.opening_prompt` grouped by attributed directory | always shown as a count | — |

**Already written is not a finding.** Before a finding is emitted its dedup
key is tested verbatim (whitespace-collapsed) against every CLAUDE.md from the
repo root to the attributed directory, imports resolved, global and local
scopes included. A hit downgrades the row to *written*, naming the file, and
stays visible so the reader sees the rule doing its job. A paraphrased rule is
missed, and the section says so; the alternative is semantic matching, which
is #1198.

**Cost and bound.** A whole-body read, like `subagent::build` (1.37 s over
0.86 GB) and `plugins::count_line`, not a head read. Scope first by SQL, then
8 MB per file (`INDEX_BUDGET_BYTES`, read `limit+1` as #1213 recommends),
a per-pass cap with a `(size, mtime)` ledger like `claude_index_ledger`, and
results stored in a new `claude_advice_signal` table so a re-open re-reads
only changed files. Runs on demand behind the panel, never on the live pass
(#1246). Before merge the PR must print, from a `real_corpus`-style test,
wall time cold and warm for the largest real repository's session set, the
number of sessions truncated at 8 MB, and findings per signal at the
thresholds above, because the thresholds cannot be validated on a
one-session machine.

**Coverage.** The answer carries `Coverage { analysed, total, unreadable,
truncated, last_pass_at }` in `search.rs`'s shape. Counts are "at least N"
while `analysed < total` or `unreadable` is non-empty. `Verdict::NoSessions`
is its own variant: no sessions under a directory is not "nothing went
wrong".

**Privacy.** A row carries the key, the count and session ids. User text
(S2) and error text (S5) stay behind a click, clamped to 300 characters as
`clamp_prompt` does. Nothing leaves the machine and nothing is logged.

**Out of scope.** Subagent transcripts (they roughly double the read; a
follow-up); anything semantic.

### 3. Missing subdirectory CLAUDE.md (`advice/gaps.rs`)

**Rule, cited.** Anthropic's memory docs: "CLAUDE.md and CLAUDE.local.md
files in the directory hierarchy above the working directory are loaded at
launch. Files in subdirectories load on demand when Claude reads files in
those directories." A convention that lives only in the root is paid for by
every session; one in `crates/foo/CLAUDE.md` is loaded exactly when a session
touches `crates/foo/`.

**Signal.** A directory with at least one role signal and no CLAUDE.md on the
path from it up to (excluding) the root. Role signals, recorded inside
`scan_repo`'s own loop rather than by a second walk (#1236 halved that cost
once): a manifest present (nested `package.json`, `Cargo.toml`,
`pyproject.toml`, `go.mod`, `Package.swift`); workspace membership (Cargo
`[workspace] members`, `package.json` `workspaces`); a count of `*.test.*`,
`tests/`, `__tests__/`, `spec/` entries; well-known role names (`src-*`,
`packages/*`, `crates/*`, `apps/*`, `docs/`, `.github/workflows`,
`scripts/`); and, when the transcript producer supplies it, the directories
sessions edit heavily. `packages::detect::projects` is reused for manifests
with two caveats named in the module docs: it skips dot-directories and stops
at depth 3, and it drops an unreadable directory silently, so the gaps
producer calls `ecosystems()` from the walk that already reports the failure.

**Strength.** Strong: a manifest, workspace membership, ≥ N test files (N to
be recorded with its reason; measured spread here is 5 to 58), or the
session-edit signal. Weak: role name only. Suppressed when the nearest
ancestor CLAUDE.md has a heading naming the directory; downgraded when it
merely references the path. The placement producer owns judging that
section; this one only detects that it exists. Both share one
`text::sections_naming(dir)` helper.

**Grouping.** Findings are keyed by parent and reason, so twelve workspace
members under `packages/` are one finding with the member list, not twelve.
Group at ≥ 3 siblings; a member with a distinct extra signal stays its own
row. Worked example, this repository: six CLAUDE.md files, ten manifests, two
candidates (`crates/headstate-stepup/` strong, `docs/` weak) out of about
twenty directories. That ratio is the target.

**Unknown.** No gap is emitted for a directory the walk could not list, nor
for anything under it. A manifest that will not parse is Unknown for the
member question. A walk cut short carries `partial` and the count it covered,
and nothing found before the cut is discarded.

**Brief.** "Add `crates/headstate-stepup/CLAUDE.md` covering: how it is built
and tested on its own (it has its own `Cargo.lock` and `deny.toml`); what it
is for and which side depends on it; the rules from the root file that apply
here with a different twist. Keep it to what is true only here."

### 4. Content in the wrong file (`advice/placement.rs`)

**Rule, cited.** The same docs page: subdirectory files "are included when
Claude reads files in those subdirectories", the target is "under 200 lines
per CLAUDE.md file", and imports do not help ("imported files still load and
enter the context window at launch").

**Signal.** For each readable CLAUDE.md, split into sections with
`text::sections`. Per section collect path candidates: backtick spans,
`@imports`, bare tokens containing `/`, trailing `:line` stripped. Resolve
each **only** against the file's own directory and the repository root with
`is_file() || is_dir()`, never by suffix search. Each resolved path votes for
its first segment below the file's directory; a path at the file's level
votes *stay*; a path outside the file's directory votes *elsewhere*;
not-found does not vote and is listed.

**Finding when** ≥ 2 distinct resolved paths, all voting for one segment
`d`, `d` exists as a directory, and there are no stay or elsewhere votes.
Proof: "Section `<heading>` (~N est. tokens) names only paths under `<d>/`:
<list>." If `<d>/CLAUDE.md` exists the suggested change is the move and the
token saving; if not, the finding is *qualified* ("would need
`<d>/CLAUDE.md`, which does not exist") and the gaps producer owns whether
creating one is advised.

**False positives, and what suppresses each.** A repo-wide rule that names
`src/` once among others: votes spread, suppressed. A seam between two
directories: an elsewhere vote, suppressed. A rule with one example path: the
≥ 2 floor. The root file's "Rules" section names `claude/cli.rs:128`, which
resolves only under `src-tauri/src/`: a suffix search would file a repo-wide
section under `src-tauri`, the wrong answer, so it is reported as not found
from this file and does not vote.

**Hand-run on this repository.** Root: 0 findings. `src-tauri/CLAUDE.md`: 0.
`src/CLAUDE.md`: one qualified candidate, the `useIsMobile()` vs
`IS_MOBILE_BUILD` section (~125 est. tokens, names only `src/lib/useIsMobile.ts`
and `src/lib/target.ts`; `src/lib/CLAUDE.md` does not exist). The hit is
arguable, which is why it renders as a qualified fact and not an instruction.

**Unknown.** A `read_dir` or metadata error while resolving is Unknown for
that section. Not-found is not Unknown: there is nothing to place. The
"nothing to move" line is gated on `!scan.is_partial()`.

### 5. Rot (`advice/rot.rs`)

**Signal.** Every reference `refs::extract` finds, resolved per the table
above. Measured on this repository's three files: 16 path references (all
exist; 4 only by unique suffix), 2 `path:line`, 2 `make` targets, 2 `yarn`
(one a script, one a binary), 4 skill names, 18 qualified symbols, 10 issue
numbers. Zero certain rot today, and one real drift case the check must not
report: `claude/sessions.rs:444` no longer holds the cited sentence, but the
file has 2829 lines, so the line exists.

**Verdicts.** `Missing`: resolves to nothing; certain; `Severity::Problem`.
`LinePastEof { lines }`: file exists, line beyond its count; certain, lower
severity, a separate row never in the "gone" count. A line within EOF is not
reported at all, because the check cannot know what the author meant to
point at. `Unknown`: a directory in `unreadable_dirs`, an ambiguous suffix, a
kind this build does not index. The report carries `refs_checked` and
`unchecked` with reasons: a scan that checked 0 of 41 must read differently
from a clean one.

**Out of scope.** Line-content fingerprints; resolving `#NNNN`, SHAs and
tags; `@import` targets (`imports.rs` already reports those).

**Brief.** "`src/CLAUDE.md:15` names `src/lib/target.ts`, which does not
exist in this repository. 3 of 41 references could not be checked
(`src/legacy/` is unreadable)."

### 6. Skills alongside CLAUDE.md (`advice/skills.rs`)

**What exists.** `definitions::frontmatter` reads `name:` and `description:`
only when line 1 is `---`, strips quotes, skips every other line. Nothing is
parsed from the body. Rendered once, by `DefinitionsSection` in
`ClaudePluginsPage.tsx`.

**Rules, with the surface that enforces each.** The two Anthropic docs
disagree, and a finding names which surface it is quoting. Claude Code's
skills reference: all fields optional, `name` defaults to the directory,
`description` to the first non-empty body line; description plus
`when_to_use` truncated at 1,536 characters in the listing; frontmatter read
only when `---` is line 1; booleans accept `yes/no/on/off/1/0`; the field list
including `disable-model-invocation`, `user-invocable`, `allowed-tools`,
`context: fork`, `paths`, `compatibility` (≤ 500 chars). The Agent Skills
spec as the API enforces it: `name` required, ≤ 64 chars, lowercase, digits,
hyphens, not "anthropic" or "claude"; `description` required, non-empty,
≤ 1,024, no XML tags. Best practices: body under 500 lines; about 100 tokens
of metadata per skill loaded at startup. The unquoted-colon YAML pitfall
could not be cited from this session (the spec host is unreachable) and
ships as a warning that names the line, never as "Claude Code will reject
this", until it is verified.

**Checks.** (a) Frontmatter: absent `---` on line 1 with one later; name over
the limit or with the wrong character set; description absent, empty, over
1,024, or with `when_to_use` over 1,536; an unrecognised boolean spelling.
(b) Cross-references: a CLAUDE.md naming a skill (the `` `X` `` skill, a
`## Skills` list) that no scope holds is `Problem`; a skill nothing names is
advice only, because Claude Code triggers it from its description. (c)
Procedures that belong in a skill: a section with a `bash` fence, a heading
starting "Before", "Gates", "Releasing", an ordered list of commands, or a
fence whose text also appears verbatim in a SKILL.md. This repository is the
worked example: the `check-runs?filter=all` command is in `.github/CLAUDE.md`
and two skills; `cargo fmt` then `cargo test --lib` is in `src-tauri/CLAUDE.md`
and `verify`. (d) Cost: body tokens per skill via `tokens::estimate`, and the
per-session description total per scope (measured here: about 167 est. tokens
paid by every session for four skills). (e) Usage: `plugins.rs` already reads
`tool_use` blocks named `Skill` but attributes only `<plugin>:<name>`; the
count is `Option<u64>`, `None` unless the scan was complete, and renders as
"no call observed in N sessions scanned", never "unused" (#1207).

**Unknown.** A `ScopeRefusal` makes every "not held" a "not found in the
scopes that could be read", and every count "at least".

### 7. Content shape (`advice/shape.rs`)

Rules distilled from published guidance that a deterministic check can
enforce over one file. The research behind them is
`2026-09-21-claude-md-practices-research.md` beside this document; it marks
each source *measured* or *asserted*, and that distinction sets severity:

- **Behaviour facts** from Claude Code's own memory docs and changelog say
  what the tool *does* and are the only grounds for `Severity::Problem`.
- **Advice** from Anthropic's guidance pages, HumanLayer, Cursor and the
  published linters is asserted, not measured, and is `Severity::Advice`;
  the finding quotes the source rather than predicting an effect.
- **Measurements.** Three 2026 studies counted something: context files
  raise inference cost by over 20% on average; a controlled test over 1,650
  sessions varied file size between 25 and 500 lines and detected no
  adherence difference, while sequence position within the session did
  degrade adherence; and a survey of 100 popular repositories found a
  content smell in 91% of them, lint leakage (62%) and files of 200+ lines
  (42%) the most common.

The consequence for wording: the docs say "target under 200 lines" and that
longer files "reduce adherence"; the one controlled test could not detect
that. So a size finding states the cost and Anthropic's target, never
"adherence will drop". The bill is measured; the compliance loss is not.

**Loading facts the rules depend on** (memory docs, verbatim where quoted):
files are "concatenated into context rather than overriding each other",
root first, "instructions closer to where you launched Claude are read
last"; subdirectory files load "when Claude reads files in those
subdirectories", "not at launch and not when writing or creating files
there", and "the built-in Explore and Plan agents skip CLAUDE.md";
imports have a "maximum depth of four hops", their parsing "skips Markdown
code spans and fenced code blocks", and imported files "still load and enter
the context window at launch"; Claude Code "loads a CLAUDE.md file of up to
4 MiB in full and skips a larger file"; "block-level HTML comments are
stripped before the content is injected"; `.claude/rules/*.md` load at
launch with the project file's priority; `AGENTS.md` is read only when no
CLAUDE.md exists. Two consequences for existing code, verified by fixture
before any count from this producer is shown: `tokens::estimate` receives
the text with block-level HTML comments removed, and `imports::parse_imports`
skips code spans as well as fences.

**Rules this producer owns**, each with its source and threshold recorded
in the module docs:

| rule | threshold | severity |
|---|---|---|
| file over the docs' line target | 200 lines; the brief names the community range (60 to 500) | Advice |
| the launch set as a whole: every file loaded before the first prompt | informational; est. tokens, "at least" when partial | Advice |
| file over the hard skip | 4 MiB | Problem |
| secret-shaped token (`sk-ant-`, `ghp_`, `github_pat_`, a private-key header, an AWS key id); obvious placeholders skipped | the finding carries the line and a masked prefix, never the value | Problem |
| emphasis on more than one line (all-caps IMPORTANT, YOU MUST, NEVER, ALWAYS) | ≥ 2 lines in one file, with the best-practices sentence quoted | Advice |
| a hard rule a hook could enforce: `never`/`must not`/`always` with a tool verb | the line, quoted | Advice |
| conflicting instruction, narrow class: two files in one launch set naming different package managers, lint entry points or default branches | both lines | Advice |
| derivable content: a fenced tree listing, or the `/init` skeleton headings all present | block line range | Advice |
| `AGENTS.md` beside a CLAUDE.md that does not import it | paths | Advice |
| `CLAUDE.local.md` not git-ignored | `git check-ignore` fails | Problem |

Rules the research hands to other producers, which must list them or they
are unowned: lint leakage (a style line plus a formatter config present) to
toolchain; blind references (a doc path named with "see"/"read" and neither
imported nor conditioned) and dated facts to rot; duplicate content across
scopes (≥ 3 identical normalised lines between two files in one launch set)
and an always-rule in a lazily loaded subdirectory file to placement; import
depth over four hops and an import resolving outside the working directory
to the imports seed; the skills authoring page's rules (body over 500 lines,
reference chains deeper than one level, description not in third person or
lacking a "use when" clause) to skills.

**Guidance, never a finding**, shown as attributed text in the panel: "would
removing this cause Claude to make mistakes? If not, cut it"; add a rule
when Claude gets a convention wrong twice, and capture a procedure as a skill
on the third paste; WHY/WHAT/HOW and file:line pointers over pasted
snippets; revisit after major model releases; process and gating rules are
the ones that fail late in a session, so put gates in hooks. A "required
sections" template is rejected: Anthropic says there is no required format,
and `/doctor` removes architecture overviews.

**Unknown.** A rule whose input could not be read (a manifest, `git
check-ignore` unavailable) returns `Err` and the check is Unknown, never
zero findings. Headstate cannot know the session's model, so it never
reproduces Claude Code's model-scaled "too long" warning.

## Honesty rules, mapped

| rule | where it bites here |
|---|---|
| Absent is not zero | an unreadable manifest, scope or transcript is Unknown with its reason, never an empty list; no sessions under a directory is `NoSessions`, not "nothing went wrong" |
| Partial is not nothing | a producer's `Err` becomes `CheckRun::Unknown` and the other producers' findings stand; a transcript pass that hits its cap returns what it read with `remaining` set |
| Qualify, or suppress | counts are "at least N" while coverage is short; a finding below threshold is not shown at lower confidence, it is not shown; the token figure always carries "est." |
| Pending and Unknown are different | pending is the absence of a `Report` (skeleton); Unknown is a `CheckRun` variant rendered in amber with its reason |
| Never "we did not ask" as "they did not answer" | the panel fetches only while open; a closed panel shows no state, and the query's `isError` is a rejection of the whole command, distinct from a short run |
| A warning states a fact | the finding sentence is the producer's own measurement; the rationale lives in module docs |

## Privacy

Transcripts hold the user's own text. The wire carries keys (command heads,
paths, patterns, tool names), counts and session ids. Quoted user or error
text appears only behind a click, clamped to 300 characters. No producer has
network access. Nothing is written to the diagnostic log; `redact.rs` guards
only that log and must not be relied on here.

Fixtures use synthetic paths only (`/home/octocat/hello-world`). The privacy
guard scans commit messages as well as files.

## Testing

**Rust, model.** `every_check_variant_renders_a_brief_that_names_its_subject`
over `Check::ALL` with a fixture per variant;
`a_failing_producer_is_reported_unknown_not_dropped`;
`the_report_lists_every_check_exactly_once`;
`the_brief_never_claims_a_line_it_does_not_have`. An invariant in
`invariants.rs` scans `advice/brief.rs` with comments stripped and asserts
the `match` on `Check` has no `_ =>` arm, sabotage-proven both ways per the
`guard` skill, and negative-proved against the existing match in
`confighealth.rs`. A unit test asserts `Check::ALL.len()` equals the variant
count derived from the enum's source block.

**Rust, per producer.** A synthetic `tempfile` fixture, the finding it must
emit, and at least one thing it must *not* emit, each asserted by absence and
then flipped to prove the negative assertion can fail. Every producer has an
`Unknown` test: a `chmod 000` directory on unix, restored before asserting,
skipped on Windows as `confighealth.rs:628` does.

**Frontend.** A new `ClaudeMdAdvicePanel.test.tsx` mocking `useClaudeMdAdvice`:
skeleton while loading; an Unknown check renders its reason and the "at
least" notice and not "nothing found"; all-ran-zero-findings renders
"nothing found"; findings render in wire order (fed out of rank order, DOM
order asserted equal to the wire); "Copy brief" calls `copyText` with
`finding.brief` verbatim and toasts the failure; a file-subject button
carries `aria-current` only when it is the active file. In
`ClaudeMdPage.mobile.test.tsx`: tapping a file-subject finding shows the file
screen. `src/` changes reach the iOS companion, so `make test-mobile` runs
with `yarn vitest run`.

**Local gate.** The unreadable-directory tests cannot fail as root, which is
how this container runs; `capsh --drop=cap_dac_override,cap_dac_read_search
-- -c 'cargo test --lib'` restores the permission checks and is what the
baseline below was measured under: 2176 passed, 0 failed, 44 ignored.
Frontend baseline: 3311 passed across 218 files.

## Sequencing and seams

1. **Model, command, panel, seed** (`Check::Imports`), plus `text.rs`,
   `refs.rs` and `packages/scripts.rs`. Lands first. The seed makes the panel
   render a real row on day one, from `ImportNode.problem` the scan already
   carries and today shows as a red pill with no remedy.
2. **Producers, in parallel, each from the model branch:** toolchain, gaps,
   placement, rot, skills, shape. Each adds one `Check` variant, one
   `Producer` in `PRODUCERS`, one `brief::render` arm, its fixture tests, its
   Unknown path, and a measured cost in the PR body. The only shared lines
   they touch are the enum, `ALL`, `PRODUCERS` and the brief match; conflicts
   there are additions and resolve mechanically. Semantic conflicts are the
   ones to look for, as 7.0 found three times: the integration branch is
   compiled and tested with every producer merged before any is called done.
3. **Transcripts** last among the producers, because it is the only one with
   a store migration and an unmeasured cost, and because gaps takes its
   edit-signal as an optional input rather than a dependency.
4. **Launch with the brief**, optional, after the panel has shipped.

Before any producer closes:

```bash
grep -rn 'ClaudeMdAdvicePanel' src --include='*.tsx' | grep -v 'ClaudeMdAdvicePanel.tsx\|\.test\.'
grep -rn 'Check::<Variant>' src-tauri/src --include='*.rs' | grep -v '_test\|tests::'
```

Zero hits for the first means the panel has no host; zero hits outside the
producer's own module for the second means no producer is registered.

## What to measure before calling 7.1 done

- Wall time of `claude_md_advice` on the largest real repository, cold and
  warm, with and without the transcripts producer, printed by a test in the
  style of `transcript.rs::tests::real_corpus`.
- Findings per producer on a real machine, to check the thresholds are not
  noise. The gaps producer's target ratio is stated above; the placement
  producer's hand-run found one qualified hit in three files; the transcript
  thresholds are unvalidated until a corpus with more than one session is
  measured.
- Whether any finding on this repository's own files is wrong. The design
  says the root file must score clean on placement; if it does not, the check
  is measuring the wrong thing.
