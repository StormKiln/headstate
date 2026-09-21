# CLAUDE.md Advice Implementation Plan

> **For agentic workers:** one task per worktree, one pull request per task,
> the full gate before any push. Steps use checkbox (`- [ ]`) syntax for
> tracking. Task 1 lands before any other task starts; tasks 2–7 branch from
> task 1's branch and run in parallel; task 8 runs last.

**Goal:** Turn the CLAUDE.md page from an inventory into advice: one finding
model, one Tauri command that runs every producer, one panel with a copyable
brief per finding, and seven deterministic producers over CLAUDE.md files,
build manifests, transcripts and skills.

**Architecture:** `src-tauri/src/claudemd/advice/` holds the model, the
brief renderer and one module per producer. `claudemd/text.rs`,
`claudemd/refs.rs` and `packages/scripts.rs` are the shared parsers. One
command, `claude_md_advice`, builds a `Context` once and runs `PRODUCERS` in
order; a producer's `Err` becomes `CheckRun::Unknown`. The frontend mounts
`ClaudeMdAdvicePanel` in the CLAUDE.md page's rail and recomputes nothing.

**Tech Stack:** Rust (serde, rusqlite for the transcripts producer), React
19, TanStack Query 5, Tailwind 4, vitest.

**Spec:** `docs/superpowers/specs/2026-09-21-claude-md-advice-design.md`

**Epic:** #1255, with one sub-issue per task (numbers in the task headings).

## Global Constraints

- Read-only. No producer writes to a CLAUDE.md, a skill, or `~/.claude`.
- No producer runs Claude (#1198). Every signal is a string or structure test.
- Absent is not zero; partial is not nothing; qualify or suppress; pending and
  unknown are different states. Each producer has an `Unknown` path and a
  test that exercises it (`chmod 000` on unix, restored before asserting,
  skipped on Windows as `confighealth.rs:628` does).
- `PathBuf::join`, never string concatenation. Normalise `\r\n` before any
  `\n`-anchored pattern.
- Every token figure is an estimate and says "est."; every floor says "at
  least".
- A new Tauri command needs all five wiring points (`commands.rs` + `lib.rs`;
  `remote/surface.rs` row and dispatch arm; `src-mobile/src/surface.rs` same
  row, same position; `src/api/tauri.ts` wrapper and a `transport.test.ts`
  row). Only task 1 adds a command.
- Synthetic fixtures only (`/home/octocat/hello-world`); the privacy guard
  scans commit messages too. `git add -N` new files before `make lint`.
- The gate, with counts: `cargo fmt`; `cargo test --lib` (baseline 2176
  passed, 0 failed, 44 ignored, measured under
  `capsh --drop=cap_dac_override,cap_dac_read_search` when running as root);
  `yarn vitest run` (baseline 3311); `make lint`; `make test-mobile` when
  `src/` changed.
- Before a task closes, both call-site greps in the spec's *Sequencing*
  section hit outside the producer's own module.

---

### Task 1: Model, command, panel, seed, shared parsers (#1262)

**Files:**
- Create: `src-tauri/src/claudemd/advice/{mod,brief,imports}.rs`,
  `src-tauri/src/claudemd/text.rs`, `src-tauri/src/claudemd/refs.rs`,
  `src-tauri/src/packages/scripts.rs`,
  `src/components/ClaudeMdAdvicePanel.tsx`,
  `src/components/ClaudeMdAdvicePanel.test.tsx`
- Modify: `src-tauri/src/claudemd/mod.rs` (module list, an *Advice* note
  saying advice is opinion and `confighealth` is not), `src-tauri/src/commands.rs`,
  `src-tauri/src/lib.rs`, `src-tauri/src/remote/surface.rs`,
  `src-mobile/src/surface.rs`, `src-tauri/src/invariants.rs`,
  `src/api/tauri.ts`, `src/api/transport.test.ts`, `src/api/hooks.ts`,
  `src/types/pr.ts`, `src/components/ClaudeMdPage.tsx`,
  `src/components/ClaudeMdPage.mobile.test.tsx`

**Interfaces:**
- `advice::{Check, Severity, Subject, Locator, Evidence, Finding, CheckRun, CheckCoverage, Report, Producer, Context, PRODUCERS, run}` as the spec's *The model* section defines them.
- `brief::render(&Finding) -> String`; `brief::render_report(&Report) -> String`.
- `text::{sections, spans, fences}`; `refs::extract`; `scripts::{targets, scripts}` returning `Result` with absent distinguished from unreadable.
- Command `claude_md_advice(repoPath) -> Report`, `Class::Read`.
- `useClaudeMdAdvice(repoPath, enabled)`; `<ClaudeMdAdvicePanel repoPath activePath onSelectFile />`.

**Steps:**
- [ ] Model and `run()`; a failing producer is `Unknown`, never a rejection; findings sorted by `rank()` in Rust; `checks` derived from `Check::ALL`.
- [ ] Brief renderer with no wildcard arm; `render_report` with `_Could not check_` lines.
- [ ] Seed producer `Check::Imports` from `ImportNode.problem`; evidence carries no invented line.
- [ ] `text.rs`, `refs.rs`, `scripts.rs` with the fixture tests the spec lists (fenced `#` is not a heading; indented line is not a heading; `src-tauri/Cargo.toml` is a path; `-D warnings` is not; bare words are not symbols).
- [ ] Command and the five wiring points.
- [ ] Types, wrapper, hook, panel, mount; the three renders; copy brief; `aria-current` via `current()`.
- [ ] Invariant: no `_ =>` arm in `brief::render`'s match on `Check`, sabotage-proven both ways, negative-proved against `confighealth.rs`; unit test `Check::ALL.len()` equals the source-derived variant count.
- [ ] Rust tests: `every_check_variant_renders_a_brief_that_names_its_subject`, `a_failing_producer_is_reported_unknown_not_dropped`, `the_report_lists_every_check_exactly_once`, `the_brief_never_claims_a_line_it_does_not_have`, a seed test on a tempdir with `@./missing.md`.
- [ ] Frontend tests per the spec's *Testing* section; mobile test that a file-subject finding opens the file screen.
- [ ] Gate with counts; call-site grep on `ClaudeMdAdvicePanel`.

---

### Task 2: Toolchain coverage producer (#1256)

**Files:** create `src-tauri/src/claudemd/advice/toolchain.rs`; modify `advice/mod.rs` (variant, `ALL`, `PRODUCERS`), `advice/brief.rs` (arm).

**Interfaces:** `enum Toolchain { Ecosystem(packages::Ecosystem), Make, Just, Go, Bundler, Gradle, Xcode, PyprojectUnknown }`; `enum Verb { Build, Test, Lint, Format, Run, Deploy }`; `struct DetectedToolchain { toolchain, path, offers: Vec<(Verb, String, line)> }`; `fn detect(repo) -> (Vec<DetectedToolchain>, Vec<String>)`; `fn documented(scan: &EffectiveScan) -> Result<BTreeMap<Verb, Vec<Evidence>>, Vec<String>>`; `impl Producer`.

**Steps:**
- [ ] Detection over `packages::detect::projects` plus the new markers, reading each marker through a path that returns the io error; `packages::Ecosystem` is not extended.
- [ ] "Named" matching over `text::spans` and `text::fences`, first-token rule, verb map by target name; unmapped targets listed as "other" and never counted.
- [ ] Finding per (toolchain, verb) gap with evidence naming the manifest, its line, and the count of files and spans searched. Negative findings only when the scan is complete; otherwise `Unknown` naming the unreadable path.
- [ ] Tests: the six fixture cases in the sub-issue (yarn scripts gap; Makefile targets; global scope counts; unreadable import → Unknown not Gap; unreadable `package.json` → Unknown with the io error; fenced vs inline vs path-token).
- [ ] Worked example recorded in the PR body: this repository's own result.
- [ ] Gate with counts; call-site grep on `Check::Toolchain`.

---

### Task 3: Missing subdirectory CLAUDE.md producer (#1258)

**Files:** create `src-tauri/src/claudemd/advice/gaps.rs`; modify `claudemd/mod.rs` (`DirFacts` recorded inside `scan_repo`'s loop, behind a field on `Scan` that existing callers ignore), `advice/mod.rs`, `advice/brief.rs`, `text.rs` (`sections_naming(dir)`).

**Interfaces:** `struct DirFacts { path, manifests: Vec<Ecosystem-or-marker>, workspace_member: bool, test_files: usize, role: Option<Role> }`; `enum Strength { Strong, Weak }`; `struct Gap { dir, strength, reasons, grouped: Vec<String>, brief }`; `fn gaps(scan: &Scan, edited: &[PathBuf]) -> GapReport { gaps, unknown_dirs, partial }`; `impl Producer`.

**Steps:**
- [ ] Record `DirFacts` in the existing walk, not a second walk (#1236). Call `packages::detect::ecosystems` per directory from that loop; workspace membership from `packages/cargo.rs` `members` (promote to `pub(crate)`) and `package.json` `workspaces`.
- [ ] Candidate/strong/weak/suppressed rules; the test-file threshold recorded with its reason in the module docs.
- [ ] Grouping by (parent, reason) at ≥ 3 siblings.
- [ ] No gap for an unreadable directory or anything under it; unparseable manifest is Unknown for membership; `partial` with the covered count.
- [ ] `edited: &[PathBuf]` accepted as an optional input; empty until task 8 supplies it.
- [ ] Tests: the fixture in the sub-issue (`packages/octocat-{a,b,c}`, ten `tests/*.test.ts`, `docs/`, one unlistable dir), grouping, strength, Unknown, suppression on adding `packages/CLAUDE.md`, downgrade on adding `## packages` to the root; sabotage both directions.
- [ ] Gate with counts; call-site grep on `Check::Gaps`.

---

### Task 4: Content in the wrong file producer (#1259)

**Files:** create `src-tauri/src/claudemd/advice/placement.rs`; modify `advice/mod.rs`, `advice/brief.rs`.

**Interfaces:** `enum Resolved { Under(String), Stay, Elsewhere, NotFound, Unreadable(String) }`; `fn path_candidates(&Section) -> Vec<String>`; `fn resolve(candidate, file_dir, repo_root) -> Resolved`; `fn assess(file: &ClaudeFile, repo: &Path) -> Vec<Finding>`; `impl Producer`.

**Steps:**
- [ ] Sections via `text::sections`; candidates from spans, `@imports` (via `imports::parse_imports`) and bare tokens containing `/`; trailing `:line` stripped.
- [ ] Resolution against the file's directory and the repo root only; never suffix search.
- [ ] Finding when ≥ 2 distinct resolved paths, unanimous segment, no stay/elsewhere; qualified when `<d>/CLAUDE.md` is absent; token figure from `tokens::estimate` of the section with "est.".
- [ ] Unknown on a `read_dir`/metadata error during resolution; not-found listed, never Unknown.
- [ ] Tests: the seven cases in the sub-issue (finding; qualified; stay vote; single path; `#` in a fence; unreadable → Unknown; not-found listed); a test that this repository's root file scores clean (fixture copied from its text, paths stubbed).
- [ ] Gate with counts; call-site grep on `Check::Placement`.

---

### Task 5: Rot producer (#1260)

**Files:** create `src-tauri/src/claudemd/advice/rot.rs`; modify `advice/mod.rs`, `advice/brief.rs`, `refs.rs` if extraction needs a kind it lacks.

**Interfaces:** `enum Verdict { Missing, LinePastEof { lines: u64 }, Unknown(String) }`; `struct Rot { findings, refs_checked: usize, unchecked: Vec<String> }`; `fn check(repo, file: &ClaudeFile, inventory: &Inventory) -> Rot`; `impl Producer`.

**Steps:**
- [ ] Resolution per the spec's reference table; skills against `Kind::Skill` across scopes; symbols by whole-word grep of the last segment over the three source roots; two suffix matches is `Unknown`.
- [ ] `Missing` is `Severity::Problem`; `LinePastEof` is `Severity::Advice` and a separate row; a line within EOF is silent.
- [ ] `refs_checked` and `unchecked` with reasons.
- [ ] Tests: the fixture in the sub-issue with four `Missing`, one `LinePastEof`, `refs_checked` = 9; the four must-not-report cases asserted by absence and flipped; `chmod 000` → Unknown naming the directory.
- [ ] Gate with counts; call-site grep on `Check::Rot`.

---

### Task 6: Skills producer (#1261)

**Files:** create `src-tauri/src/claudemd/advice/skills.rs`; modify `advice/mod.rs`, `advice/brief.rs`; `definitions.rs` unchanged except promoting `frontmatter` helpers to `pub(crate)` if needed.

**Interfaces:** `fn advise(inv: &Inventory, scan: &EffectiveScan) -> SkillsAdvice { findings, partial }`; `impl Producer`; subject `Subject::Skill { path, name }` for (a), (d), (e) and `Subject::ClaudeMd` for (b), (c).

**Steps:**
- [ ] (a) Frontmatter checks, each naming the surface that enforces the limit; the unquoted-colon case as a warning naming the line until the spec text is verified.
- [ ] (b) Cross-references both ways; "not held" downgraded to "not found in the scopes that could be read" when any `ScopeRefusal` exists.
- [ ] (c) Procedure signals; a fence whose text appears verbatim in a SKILL.md names that skill.
- [ ] (d) Body tokens per skill and the per-session description total per scope, "at least" under a refusal.
- [ ] (e) Usage count as `Option<u64>` from the existing `plugins.rs` pass or `None`; rendered "no call observed in N sessions scanned", never "unused".
- [ ] Tests: `octocat-deploy` fixture (70-char name, `: ` in the description, `disable-model-invocation: maybe`, 501-line body); a CLAUDE.md naming `octocat-verify` with no such skill; a walled-off `skills/` asserting no "not held" finding.
- [ ] Gate with counts; call-site grep on `Check::Skills`.

---

### Task 7: Content-shape producer (#1263)

**Files:** create `src-tauri/src/claudemd/advice/shape.rs`; modify `advice/mod.rs`, `advice/brief.rs`; `src/components/ClaudeMdAdvicePanel.tsx` gains a "Guidance" footer listing the judgement-only advice as text.

**Interfaces:** `struct Rule { id, source: &'static str, threshold }`; `fn assess(file: &ClaudeFile, text: &str) -> Vec<Finding>`; `impl Producer`; `const GUIDANCE: &[(&str, &str)]` (text, source) shipped to the panel through `Report`.

**Steps:**
- [ ] The ten rules in the spec's producer 7 table, each with its source and threshold in the module docs; behaviour facts are `Problem`, advice is `Advice` and quotes its source.
- [ ] `tokens::estimate` fed text with block-level HTML comments removed; `imports::parse_imports` skipping code spans as well as fences; both pinned by fixture.
- [ ] Guidance text shipped through `Report` and rendered as attributed text, never as a row.
- [ ] Tests: a 201-line file fires and a 200-line one does not; three `IMPORTANT` lines fire and one does not; `sk-ant-` inside a code block fires masked and `<your-token>` does not; `AGENTS.md` beside a non-importing CLAUDE.md; `CLAUDE.local.md` with no `.gitignore` entry; an HTML comment whose removal changes the estimate; an unreadable input → Unknown, not clean.
- [ ] Gate with counts; call-site grep on `Check::Shape`.

---

### Task 8: Transcript-derived advice producer (#1257)

**Files:** create `src-tauri/src/claudemd/advice/transcripts.rs`; modify `advice/mod.rs`, `advice/brief.rs`, `src-tauri/src/store/schema.rs` (migration: `claude_advice_signal`, `claude_advice_ledger`), `src-tauri/src/claude/preview.rs` (promote `tool_args`/`file_change` to `pub(crate)`; do not copy them), `advice/gaps.rs` (accept the edit signal).

**Interfaces:** `fn analyse(conn: &mut Connection, repo: &Path, budget: Budget) -> TranscriptAdvice { verdict: Verdict::{Findings, NoSessions{dir}}, findings, coverage: Coverage }`; `impl Producer` (Unknown when `conn` is `None`); `pub fn edited_dirs(conn, repo) -> Vec<PathBuf>` for gaps.

**Steps:**
- [ ] Session selection by SQL over `claude_session.cwd` with agent-worktree re-rooting; file-path attribution to the deepest CLAUDE.md-bearing ancestor.
- [ ] Signals S1–S6 with the thresholds in the spec; `is_error` absent is not `false`; counts are distinct sessions.
- [ ] Dedup against every CLAUDE.md on the path, imports resolved, global and local included; hits render as *written*.
- [ ] Bounds: 8 MB per file read `limit+1`; per-pass cap with a `(size, mtime)` ledger; results in `claude_advice_signal`; never on the live pass.
- [ ] Coverage in `search.rs`'s shape; `NoSessions` its own variant; "at least" while short.
- [ ] Privacy: user and error text only behind a click, clamped to 300 chars; nothing logged.
- [ ] Tests: the fixture in the sub-issue (S1 across two sessions; a worktree cwd re-rooted; a CLAUDE.md that already holds the key → *written*; an unreadable transcript → coverage.unreadable and no `None` verdict; zero sessions → `NoSessions`; a 9 MB fixture whose only pair sits past the cap → `truncated: 1` and "at least").
- [ ] Measured before merge, printed by a `real_corpus`-style test and recorded in the PR body: wall time cold and warm for the largest real repository's session set; sessions truncated; findings per signal.
- [ ] Gate with counts; call-site grep on `Check::Transcripts`.

---

### Task 9: Integration and release notes

- [ ] Merge every producer branch onto an integration branch; compile and run the full gate there; look for the semantic conflicts 7.0 found three times (a signature changed in one branch, a caller added in another).
- [ ] Run `claude_md_advice` against this repository and record what each producer says; the root file must score clean on placement.
- [ ] Release notes entry for 7.1 per the `release` skill.
