//! Transcript-derived advice: what recurred in the sessions recorded
//! under a repository, and whether a CLAUDE.md on the path already
//! states it.
//!
//! # The question, and what answers it
//!
//! A CLAUDE.md author wants to know what kept going wrong in this
//! directory. A command that failed and was then corrected (`yarn lint`
//! then `make lint`), a user saying "no, use X", a call auto mode refused,
//! the same grep run in ten sessions: each is a rule that could exist in
//! the CLAUDE.md that directory loads. Every signal here is a string or
//! structure test over JSONL records. No model runs on anything (#1198).
//!
//! | signal | detection | finding when |
//! |---|---|---|
//! | S1 corrected command | Bash A with `is_error: true` (or stderr and no stdout), then within the next [`CORRECTION_LOOKAHEAD`] Bash calls a B whose head differs, shares a non-flag token, and has `is_error: false` | same A-head → B-head in ≥ [`MIN_SESSIONS_CORRECTED`] sessions |
//! | S2 user correction | a `user` text record whose first 6 words hold a word from [`NEGATIONS`], within [`CORRECTION_WINDOW`] records after a `tool_use` | same tool + head/path in ≥ [`MIN_SESSIONS_USER_CORRECTION`] sessions |
//! | S3 denied call | `is_error: true` whose text carries one of [`DENIAL_PHRASES`], plus `claude_hook_event` `PermissionDenied` rows | same tool + head in ≥ [`MIN_SESSIONS_DENIED`] sessions |
//! | S4 repeated search | identical Grep/Glob `pattern`, or a `Read` within the first [`EARLY_CALLS`] tool calls, after a Grep, Glob or failed Read, of a file the session does not also Edit, Write or MultiEdit | ≥ [`MIN_SESSIONS_SEARCH`] sessions |
//! | S5 repeated error | `is_error: true` text normalised by [`normalise_error`] | identical first [`ERROR_KEY_CHARS`] chars in ≥ [`MIN_SESSIONS_ERROR`] sessions |
//! | S6 task census | sessions per attributed directory, from `claude_session` | always, as a count |
//!
//! S1, S2 and S3 are [`Severity::Advice`]: each carries a rule a session
//! had to learn -- the command that worked after the one that failed,
//! the user's stated fix, the call not to make. S4 and S5 are
//! [`Severity::Note`]s: observations, not advice. S5 (#1368), worded
//! "recurring error: `<tool>` on `<key>`: …", shows only that something
//! failed repeatedly, which is often nothing CLAUDE.md wording controls
//! (a sub-agent's schema, a search tool's timeout). An error that a later
//! call in the same session corrected is already S1, and that stays
//! Advice. S4 (#1369) shows where sessions spent their first calls; a CI
//! task that globs `.github/workflows/*.yml` and reads one is doing its
//! job, and nothing links the count to a line a CLAUDE.md could hold. A
//! Note carries no suggestion and no Claudify, and whether its key is
//! already written cannot make it wrong, so an unreadable corpus file
//! leaves it a Note rather than Unknown.
//!
//! Every count states its denominator (#1369): "in 3 of 137 analysed
//! sessions under `<repo>`", qualified "at least 3" while the pass is
//! short. The denominator is in the numerator's unit -- distinct tasks
//! among the analysed sessions -- and is the repository's, so a finding
//! placed on a subdirectory says ", attributed to `<dir>`" rather than
//! implying the sessions ran there.
//!
//! A count is DISTINCT TASKS, never records: distinct sessions, with the
//! sessions that share an opening prompt counted once (#1337), because
//! an automated `claude -p` task replayed seven times is one task, not
//! seven sessions agreeing. The fingerprint is [`task_fingerprint`] of
//! the first user text, stored on the ledger; a session with no opening
//! prompt is its own task, and the finding says how many runs it folded
//! ("3 sessions (9 runs; replays of one task counted once)").
//!
//! `is_error` absent is not `false`: a result that did not say is
//! neither a failure (for A) nor a success (for B), and a test below
//! pins both directions. The
//! thresholds are unvalidated on a real corpus -- this machine holds one
//! session -- and are constants so a corpus can move them without a
//! migration.
//!
//! # Attribution
//!
//! A tool call's absolute `file_path` places the finding on the deepest
//! CLAUDE.md-bearing ancestor directory from the repository scan, else
//! the repository root; a Bash command has no path and attributes to the
//! record's `cwd`. A path inside a linked git worktree of the repository
//! re-roots to the repository: `<wt>/src/a.ts` is `<repo>/src/a.ts` for
//! attribution, and a file path under the repository is keyed relative
//! to it (`src/a.ts`), so the same file read in two worktrees is one key
//! and a finding never names a disposable worktree. A worktree is
//! identified by its `.git` file, not its directory name; [`Worktrees`]
//! carries the rule, including what happens once the worktree has been
//! deleted: a deleted checkout is identified by the sessions' own cwds
//! and the repository's git ignore rules (#1335). A path-keyed finding
//! whose path no longer exists after re-rooting is not advice: it is left
//! out and counted in one coverage [`Severity::Note`] ("N findings named
//! paths that no longer exist"; a count, not advice, #1354). The stored row carries the attributed DIRECTORY,
//! and which CLAUDE.md it maps to is decided at read time against the
//! current scan, so a CLAUDE.md added since the pass moves the finding
//! without a re-read.
//!
//! # Already written is not a gap
//!
//! Before a finding is emitted its dedup key is tested verbatim, with
//! whitespace collapsed and case kept, against every CLAUDE.md from the
//! repository root to the attributed directory, their resolved imports,
//! the global and local scopes the effective scan carries, each of
//! the repository's `.claude/rules` a session in that directory would
//! load (`claudemd::rules`: unconditional, or `paths:` reaching it), and
//! every skill: `SKILL.md` at any depth under the repository's
//! `.claude/skills` and the user's `~/.claude/skills` (#1370). A path a
//! skill runs is the skill doing its job, and a hit names it: "already
//! written in skill `<name>` (`<path>`)", the name being the directory
//! holding the file. Plugin skills are out of scope; they are not the
//! repository owner's to change. The report cache sees these files
//! through the definitions inventory (one level under each `skills/`)
//! and, for the repository's, through git's HEAD and status; a user
//! skill nested deeper than one level is not a cache input, so editing
//! one alone does not invalidate a cached report. A
//! file that does not exist holds nothing. A file that exists and could
//! not be read, or a rule `claudemd::rules` could not read (whose
//! `paths:` are unknown too), is not "not written" (#1351): when no
//! readable file holds the key, the finding is [`Severity::Unknown`],
//! its sentence ends "whether `<key>` is already written could not be
//! checked", and its evidence names each such file with its io error.
//! Unknown rather than a qualified Advice, because the file that was not
//! read is exactly where the rule would be written: the advice to add it
//! may be wrong, and a possibly-wrong claim is suppressed, not
//! qualified. A hit in a readable file still settles it. A hit is kept visible as a
//! [`Severity::Note`] worded "already written in `<file>`", so the reader
//! sees the rule doing its job without being told to change anything
//! (#1339). A paraphrased rule is missed by
//! this; semantic matching is #1198.
//!
//! # Bounds
//!
//! Sessions are chosen by one query over `claude_session.cwd`; nothing
//! walks the corpus to choose. Each transcript is read to
//! [`BUDGET_BYTES`], reading `limit + 1` bytes so truncation is a fact
//! of the read rather than a stat/read race on a live file (#1213). At
//! most [`SESSIONS_PER_PASS`] transcripts are READ per pass; a session
//! whose `(size_bytes, mtime_ms)` matches `claude_advice_ledger` is
//! served from `claude_advice_signal` without being opened, so a re-open
//! re-reads only what changed. The ledger row also carries
//! [`RULE_VERSION`]; a row from an older extraction rule is a miss, so a
//! rule change re-reads every session once rather than serving rows the
//! new rule would not have stored. This runs on demand behind the panel, on
//! the command's `spawn_blocking`, and never on the live pass (#1246).
//!
//! # Coverage travels as findings
//!
//! The report model has no per-check coverage struct beyond
//! [`super::CheckRun`], so what the spec calls `Coverage` travels here as
//! findings: one [`Severity::Note`] stating "analysed N of M sessions
//! under `<repo>`; K truncated at 8 MB" whenever the pass is short or a
//! read was cut, and one stating "no Claude Code sessions were recorded
//! under `<repo>`" when there are none -- never an empty list, because
//! no sessions is not "nothing went wrong". While the pass is short,
//! every count says "at least". These and the S6 census are Notes: they
//! state what was measured and recommend nothing (#1339).
//!
//! A session whose transcript is not on disk -- no path recorded, or a
//! path that is `NotFound` because its worktree's project directory was
//! deleted after the merge -- is gone, and says nothing about any
//! CLAUDE.md (#1367). It is counted in that coverage Note ("K had no
//! transcript on disk and were skipped"), with a sample of the sessions
//! as its evidence. A transcript that exists and could not be read
//! (permission, I/O) is different: it might hold a signal. Those are ONE
//! [`Severity::Unknown`] listing the sessions and their io errors, never
//! one row per session, whose brief says what to make readable. Both
//! make the pass short, so the "at least" stays.
//!
//! # Privacy
//!
//! A finding carries keys (a command head, a path, a pattern, a tool
//! name), counts and session ids. User text (S2) and error text (S5, and
//! the failed command's error in S1) appear only in ONE evidence row's
//! `measured`, clamped to [`DETAIL_CHARS`]. Nothing here logs, and
//! nothing leaves the machine.

use super::{Check, Context, Evidence, Finding, Locator, Producer, Severity, Subject};
use crate::claude::preview::{blocks_of, Block, ToolArgs};
use crate::claudemd::{EffectiveScan, ImportNode, Scope};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Bytes read per transcript. `search.rs`'s budget, shared so the two
/// whole-body readers cannot disagree about what "truncated" means.
pub const BUDGET_BYTES: u64 = crate::claude::search::INDEX_BUDGET_BYTES;

/// Transcripts READ per pass. Sessions the ledger already covers do not
/// count against it. On the measured read rate of `subagent::build`
/// (0.86 GB in 1.37 s, warm cache) 200 files at the 8 MB cap is a few
/// seconds; a repository with more changed sessions than this answers
/// with a floor and finishes on the next open.
pub const SESSIONS_PER_PASS: usize = 200;

/// The extraction rule's version, stored on every `claude_advice_ledger`
/// row. A ledger row at any other version is a miss: its session is
/// re-read and its signal rows replaced, because rows extracted under an
/// older rule still decode and no longer mean the same thing. Bump it
/// whenever [`extract`] would store different rows for the same bytes.
///
/// 1: worktree paths re-rooted structurally and keyed repo-relative; S4
/// no longer counts a read of a file the session changes (#1324).
/// Before it, every row is version 0 (migration 26's default).
///
/// 2: a path under a deleted checkout that the sessions' own cwds
/// identify, and the repository's git ignores, re-roots too (#1335).
///
/// 3: an S4 early read is stored only when a Grep, a Glob or a failed
/// Read came before it in the session, and carries that call (#1336).
///
/// 4: the ledger carries the session's task fingerprint (migration 28),
/// so replays of one task count once (#1337). A row stored before it has
/// none, and would count every replay again.
///
/// 5: an S5 row carries the failing call's key (migration 29), so the
/// finding can name the file a `Read` failed on (#1338).
pub const RULE_VERSION: i64 = 5;

/// S1: sessions showing the same A-head → B-head correction.
const MIN_SESSIONS_CORRECTED: usize = 2;
/// S2: sessions correcting the same tool and head/path.
const MIN_SESSIONS_USER_CORRECTION: usize = 2;
/// S3: sessions in which the same tool and head was denied.
const MIN_SESSIONS_DENIED: usize = 3;
/// S4: sessions running the same search, or reading the same file early.
const MIN_SESSIONS_SEARCH: usize = 3;
/// S5: sessions recording the same normalised error.
const MIN_SESSIONS_ERROR: usize = 3;
/// S4: a `Read` counts as an orientation read only this early in the
/// session's tool calls.
const EARLY_CALLS: usize = 10;
/// S1: how many Bash calls after the failed one may carry the correction.
const CORRECTION_LOOKAHEAD: usize = 5;
/// S2: how many records after a `tool_use` a user text may sit in.
const CORRECTION_WINDOW: u64 = 5;
/// S2: a user text whose first six words hold one of these is a
/// correction. `do not` matches through `not`.
const NEGATIONS: &[&str] = &["no", "don't", "dont", "never", "stop", "instead", "not"];
/// S3: Claude Code's own denial phrasings, as observed in results with
/// `is_error: true`. The first is the auto-mode classifier's sentence
/// measured on this machine's corpus; the rest are the interactive
/// refusal and the permission gate.
const DENIAL_PHRASES: &[&str] = &[
    "denied by the Claude Code auto mode classifier",
    "The user doesn't want to proceed with this tool use",
    "Permission to use",
    "has been denied",
    "requires approval",
];
/// S5: the dedup key is this many characters of the normalised text.
const ERROR_KEY_CHARS: usize = 80;
/// The most characters of user or error text one evidence row carries,
/// the same bound `transcript::clamp_prompt` applies.
const DETAIL_CHARS: usize = 300;
/// Evidence rows per finding. The finding sentence carries the full
/// count; the rows are the first sessions in id order.
const MAX_EVIDENCE: usize = 20;
/// `edited_dirs`: Edit/Write calls a directory needs across sessions.
const MIN_EDITS_PER_DIR: usize = 3;

/// The `signal` column's values.
const SIG_CORRECTED: &str = "corrected";
const SIG_USER_CORRECTION: &str = "user_correction";
const SIG_DENIED: &str = "denied";
const SIG_SEARCH: &str = "search";
const SIG_ERROR: &str = "error";
const SIG_EDIT: &str = "edit";

pub struct Transcripts;

impl Producer for Transcripts {
    fn check(&self) -> Check {
        Check::Transcripts
    }

    fn run(&self, cx: &Context) -> Result<Vec<Finding>, String> {
        let conn = cx
            .conn
            .ok_or_else(|| "no session store was available to this run".to_string())?;
        analyse(conn, cx, SESSIONS_PER_PASS)
    }
}

/// One session under the repository, as `claude_session` holds it.
struct SessionRow {
    session_id: String,
    /// The recorded cwd, re-rooted out of a linked worktree.
    dir: PathBuf,
    transcript_path: Option<String>,
    has_prompt: bool,
}

/// One extracted signal occurrence: the row shape of `claude_advice_signal`.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    session_id: String,
    signal: &'static str,
    /// The attributed directory, re-rooted.
    dir: PathBuf,
    /// The dedup key: a command head, a path, a pattern, or normalised
    /// error text.
    key: String,
    /// S1: the failed head. S2–S5: the tool name.
    aux: Option<String>,
    /// 1-based record index, as `sed -n Np` counts lines.
    record: Option<u64>,
    /// S1: the correcting call's record.
    record_2: Option<u64>,
    /// User or error text, clamped to [`DETAIL_CHARS`]. For an S4 early
    /// read, the search that preceded it instead (#1336): a key, not
    /// text anyone typed, so it is shown in every evidence row.
    detail: Option<String>,
    /// S3: the denied call's `tool_use.id`, so a hook row for the same
    /// call is not counted twice.
    tool_use_id: Option<String>,
    /// S5: the failing call's own key -- a path, a command head, a
    /// pattern -- beside the normalised error text that groups it
    /// (#1338). A `Read`'s path is in its input, never its error text.
    call_key: Option<String>,
}

/// Run the pass over the sessions under `cx.repo`, reading at most
/// `cap` transcripts, and assemble the findings.
fn analyse(conn: &Connection, cx: &Context, cap: usize) -> Result<Vec<Finding>, String> {
    let repo = cx.repo;
    // One resolver for the pass, so each worktree root is probed once.
    let mut worktrees = Worktrees::new(repo);
    let sessions = sessions_under(conn, repo, &mut worktrees)?;
    let root_subject = subject_for(repo, cx.scan);
    // Git could not say whether some deleted cwds were checkouts: their
    // paths stay absolute, and the reader is told why (#1335).
    let checkout_unknown = worktrees.unknown.clone().map(|(n, why)| {
        let what = if n == 1 {
            "1 deleted session directory was a checkout".to_string()
        } else {
            format!("{n} deleted session directories were checkouts")
        };
        let sentence = format!("could not tell whether {what} of this repository: {why}");
        Finding::new(
            Check::Transcripts,
            Severity::Unknown,
            root_subject.clone(),
            vec![Evidence {
                at: Locator::File {
                    path: repo.to_string_lossy().into_owned(),
                    line: None,
                },
                measured: format!(
                    "{n} recorded cwd{} under this path no longer exist{}; their paths are not re-rooted",
                    if n == 1 { "" } else { "s" },
                    if n == 1 { "s" } else { "" }
                ),
            }],
            sentence,
        )
    });

    if sessions.is_empty() {
        return Ok(vec![Finding::new(
            Check::Transcripts,
            Severity::Note,
            root_subject,
            vec![Evidence {
                at: Locator::File {
                    path: repo.to_string_lossy().into_owned(),
                    line: None,
                },
                measured: "0 rows in claude_session with a cwd under this path".into(),
            }],
            format!(
                "no Claude Code sessions were recorded under `{}`",
                repo.display()
            ),
        )]);
    }

    // The ledger, read once. A session whose transcript is unchanged is
    // served from its stored rows without being opened.
    // A row written under another rule version is not known.
    let mut known: HashMap<String, (i64, i64, bool)> = HashMap::new();
    {
        let mut q = conn
            .prepare(
                "SELECT session_id, size_bytes, mtime_ms, truncated FROM claude_advice_ledger
                  WHERE rule_version = ?1",
            )
            .map_err(|e| format!("claude_advice_ledger: {e}"))?;
        let rows = q
            .query_map([RULE_VERSION], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)? != 0,
                ))
            })
            .map_err(|e| format!("claude_advice_ledger: {e}"))?;
        for row in rows {
            let (id, size, mtime, truncated) =
                row.map_err(|e| format!("claude_advice_ledger: {e}"))?;
            known.insert(id, (size, mtime, truncated));
        }
    }

    // A transcript that is not on disk is a session that is gone, not a
    // question about a CLAUDE.md (#1367): `missing`, counted in the
    // coverage Note. Any other failure to read one is `unreadable`, and
    // the pass reports those as ONE grouped Unknown.
    let mut unreadable: Vec<(String, String)> = Vec::new();
    let mut missing: Vec<(String, String)> = Vec::new();
    let mut analysed: HashSet<String> = HashSet::new();
    let mut truncated = 0usize;
    let mut todo: Vec<(&SessionRow, i64, i64)> = Vec::new();
    for s in &sessions {
        let Some(path) = &s.transcript_path else {
            missing.push((
                s.session_id.clone(),
                "no transcript path is recorded for this session".into(),
            ));
            continue;
        };
        let meta = std::fs::metadata(Path::new(path)).and_then(|m| {
            let mtime = m
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            Ok((m.len() as i64, mtime))
        });
        let (size, mtime) = match meta {
            Ok(v) => v,
            Err(e) if is_gone(&e) => {
                missing.push((s.session_id.clone(), format!("{path}: no longer exists")));
                continue;
            }
            Err(e) => {
                unreadable.push((
                    s.session_id.clone(),
                    format!("{path}: could not read its size: {e}"),
                ));
                continue;
            }
        };
        match known.get(&s.session_id) {
            Some((ks, km, kt)) if *ks == size && *km == mtime => {
                analysed.insert(s.session_id.clone());
                if *kt {
                    truncated += 1;
                }
            }
            _ => todo.push((s, size, mtime)),
        }
    }

    let remaining = todo.len().saturating_sub(cap);
    let now = chrono::Utc::now().to_rfc3339();
    for (s, size, mtime) in todo.into_iter().take(cap) {
        let path = s.transcript_path.as_deref().unwrap_or("");
        let (body, cut) = match read_bounded(Path::new(path), BUDGET_BYTES) {
            Ok(v) => v,
            // Deleted between the stat and the open: gone, not unreadable.
            Err(_) if resolves_to_nothing(Path::new(path)) => {
                missing.push((s.session_id.clone(), format!("{path}: no longer exists")));
                continue;
            }
            Err(e) => {
                unreadable.push((s.session_id.clone(), e));
                continue;
            }
        };
        let (rows, task) = extract(&s.session_id, &s.dir, &body, &mut worktrees);
        let stored = Ledger {
            size,
            mtime,
            truncated: cut,
            task: task.as_deref(),
        };
        if let Err(e) = store_rows(conn, &s.session_id, &rows, &stored, &now) {
            unreadable.push((
                s.session_id.clone(),
                format!("{path}: could not record its signals: {e}"),
            ));
            continue;
        }
        analysed.insert(s.session_id.clone());
        if cut {
            truncated += 1;
        }
    }

    let stored = load_rows(conn, &analysed)?;
    let tasks = load_tasks(conn, &analysed)?;
    let denials = hook_denials(conn, &sessions)?;

    // Skipped sessions make the pass short too: every "at least" stays.
    let short = analysed.len() < sessions.len() || !unreadable.is_empty() || !missing.is_empty();
    let mut out = emit(&stored, &tasks, &denials, &sessions, &analysed, cx, short);
    out.extend(checkout_unknown);

    if short || truncated > 0 {
        let mut sentence = format!(
            "analysed {} of {} sessions under `{}`; {} truncated at {} MB",
            analysed.len(),
            sessions.len(),
            repo.display(),
            truncated,
            BUDGET_BYTES / (1024 * 1024)
        );
        if !missing.is_empty() {
            sentence.push_str(&format!(
                "; {} had no transcript on disk and {} skipped",
                missing.len(),
                if missing.len() == 1 { "was" } else { "were" }
            ));
        }
        if remaining > 0 {
            sentence.push_str(&format!(
                "; {remaining} not yet read (at most {cap} are read per open)"
            ));
        }
        let mut evidence = vec![Evidence {
            at: Locator::File {
                path: repo.to_string_lossy().into_owned(),
                line: None,
            },
            measured: format!(
                "{} sessions analysed, {} unreadable, {} without a transcript on disk, {} not \
                 yet read, {} read only to {} MB",
                analysed.len(),
                unreadable.len(),
                missing.len(),
                remaining,
                truncated,
                BUDGET_BYTES / (1024 * 1024)
            ),
        }];
        // A sample behind the skipped count; the sentence carries it all.
        evidence.extend(session_rows(&missing));
        out.push(Finding::new(
            Check::Transcripts,
            Severity::Note,
            root_subject.clone(),
            evidence,
            sentence,
        ));
    }

    // Transcripts that exist and could not be read: ONE Unknown listing
    // them (#1367), never one row per session.
    if !unreadable.is_empty() {
        let k = unreadable.len();
        let sentence = if k == 1 {
            format!(
                "1 transcript under `{}` could not be read; the counts here are floors without it",
                repo.display()
            )
        } else {
            format!(
                "{k} transcripts under `{}` could not be read; the counts here are floors \
                 without them",
                repo.display()
            )
        };
        out.push(Finding::new(
            Check::Transcripts,
            Severity::Unknown,
            root_subject.clone(),
            session_rows(&unreadable),
            sentence,
        ));
    }
    Ok(out)
}

/// One evidence row per `(session, why)`: the first [`MAX_EVIDENCE`],
/// since the sentence beside them carries the full count.
fn session_rows(list: &[(String, String)]) -> Vec<Evidence> {
    list.iter()
        .take(MAX_EVIDENCE)
        .map(|(session_id, why)| Evidence {
            at: Locator::Session {
                session_id: session_id.clone(),
                record: None,
            },
            measured: why.clone(),
        })
        .collect()
}

/// Whether an io error establishes that the path names nothing. Any
/// other error is "could not look", which is not "gone".
fn is_gone(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// Directories under `repo` with at least [`MIN_EDITS_PER_DIR`] Edit or
/// Write calls across the sessions the pass has read, for the gaps
/// producer: a directory sessions keep editing is one whose missing
/// CLAUDE.md matters more than an untouched one's.
///
/// Reads only what a pass already stored, and only rows stored under the
/// current [`RULE_VERSION`]: a row from an older rule may carry a
/// worktree directory that re-roots now. A repository no pass has run
/// over yields an empty list, which is "no edits recorded", not "no
/// edits".
pub fn edited_dirs(conn: &Connection, repo: &Path) -> Result<Vec<PathBuf>, String> {
    let mut q = conn
        .prepare(
            "SELECT s.dir, COUNT(*) FROM claude_advice_signal s
               JOIN claude_advice_ledger l ON l.session_id = s.session_id
              WHERE s.signal = ?1 AND l.rule_version = ?3
              GROUP BY s.dir HAVING COUNT(*) >= ?2
              ORDER BY s.dir",
        )
        .map_err(|e| format!("claude_advice_signal: {e}"))?;
    let rows = q
        .query_map(
            rusqlite::params![SIG_EDIT, MIN_EDITS_PER_DIR as i64, RULE_VERSION],
            |r| r.get::<_, String>(0),
        )
        .map_err(|e| format!("claude_advice_signal: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let dir = PathBuf::from(row.map_err(|e| format!("claude_advice_signal: {e}"))?);
        if dir.starts_with(repo) {
            out.push(dir);
        }
    }
    Ok(out)
}

/// The sessions whose recorded cwd is under `repo`, linked worktrees
/// re-rooted first so a session run in `<repo>/.worktrees/t1` is
/// attributed to the repository itself.
///
/// Every cwd is read before any is re-rooted, so the resolver learns the
/// deleted checkouts (#1335) from the whole set in one git call.
fn sessions_under(
    conn: &Connection,
    repo: &Path,
    worktrees: &mut Worktrees,
) -> Result<Vec<SessionRow>, String> {
    let mut q = conn
        .prepare(
            "SELECT session_id, cwd, transcript_path, opening_prompt
               FROM claude_session
              WHERE cwd IS NOT NULL
              ORDER BY session_id",
        )
        .map_err(|e| format!("claude_session: {e}"))?;
    let rows = q
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| format!("claude_session: {e}"))?;
    let rows = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("claude_session: {e}"))?;
    worktrees.learn_deleted_checkouts(rows.iter().map(|(_, cwd, _, _)| Path::new(cwd)));
    let mut out = Vec::new();
    for (session_id, cwd, transcript_path, prompt) in rows {
        let dir = reroot_cwd(&cwd, worktrees);
        // `Path::starts_with` is by component, so `<repo>2` is not under
        // `<repo>`, and it is the same test on Windows separators.
        if !dir.starts_with(repo) {
            continue;
        }
        out.push(SessionRow {
            session_id,
            dir,
            transcript_path,
            has_prompt: prompt.is_some_and(|p| !p.trim().is_empty()),
        });
    }
    Ok(out)
}

/// A session's cwd, re-rooted when it lies in a linked worktree of the
/// repository (see [`Worktrees`]).
///
/// `pub(super)` so `cache::session_keys` selects the same session set
/// this producer will read. Two copies of this rule would be two answers
/// to "is this session under the repository", and the fingerprint would
/// cover a set the producer does not read. Only paths already under the
/// repository are re-rooted, and they re-root to paths still under it,
/// so the set is decided by the recorded cwd alone -- a worktree deleted
/// since does not move a session in or out of it. That is also why
/// `session_keys` does not call [`Worktrees::learn_deleted_checkouts`]:
/// a deleted checkout (#1335) moves a session's DIRECTORY, never its
/// membership, so the fingerprint would pay a git process to select the
/// same set.
pub(super) fn reroot_cwd(cwd: &str, worktrees: &mut Worktrees) -> PathBuf {
    worktrees.reroot(Path::new(cwd))
}

/// What probing one directory established about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// Its `.git` is a file naming `<repo>/.git/worktrees/<name>`.
    Worktree,
    /// It exists and is not a linked worktree of this repository: no
    /// `.git`, a `.git` directory, a submodule's `.git` file, or a
    /// worktree of a different repository.
    NotWorktree,
    /// It could not be probed: it no longer exists (a worktree removed
    /// after its branch merged), or its `.git` would not read.
    Unprobeable,
}

/// The linked git worktrees of one repository, identified structurally
/// and cached per directory for the life of one pass.
///
/// A linked worktree's root holds a `.git` FILE reading `gitdir:
/// <repo>/.git/worktrees/<name>`; that is how git itself finds its way
/// back, and it is what is tested here, so no directory name is guessed.
/// A path `<wt>/<rel>` inside one re-roots to `<repo>/<rel>`.
///
/// Only directories strictly under the repository are probed. A
/// worktree kept outside it (`../repo-t1`) is not a path this producer
/// selects sessions by, and resolving it would make the session set
/// depend on whether that directory still exists.
///
/// A worktree that has since been deleted cannot be probed, and the
/// advice usually runs after the merge that deleted it (#1335). Two rules
/// then apply, in order:
///
/// 1. **A deleted checkout the sessions themselves identify.** A
///    session's recorded cwd that lies strictly under the repository, no
///    longer exists, and is ignored by the repository's git was a
///    checkout root: Claude Code ran there, and the repository ignores
///    it. [`Worktrees::learn_deleted_checkouts`] asks git about every
///    such cwd in ONE `git check-ignore --stdin`, with a trailing `/`
///    because a cwd was a directory and a pattern like `/trees/*/` only
///    matches one git knows is a directory. No directory name is
///    guessed. A repository with no `.git` has no ignore rules, so
///    nothing is ignored; a git that cannot answer leaves every such cwd
///    un-rooted and is recorded in [`Worktrees::unknown`], never taken as
///    "not ignored".
/// 2. **Claude Code's own layout.** `<repo>/.claude/worktrees/<name>/`
///    re-roots by shape, which is what this rule did for `agent-<id>`
///    before it was structural.
///
/// Anything else deleted is left as its absolute path and keyed as such:
/// visibly not re-rooted. A finding naming such a path is not advice,
/// and `emit` leaves it out and counts it.
pub(super) struct Worktrees<'a> {
    repo: &'a Path,
    /// `repo` canonicalized, once, for a `gitdir:` that spells the
    /// repository differently (a symlinked home, `/private/var`).
    canonical: Option<Option<PathBuf>>,
    probed: HashMap<PathBuf, Probe>,
    /// Deleted checkout roots, learned from the sessions' cwds (rule 1).
    deleted_checkouts: Vec<PathBuf>,
    /// How many deleted cwds git could not be asked about, and why.
    pub(super) unknown: Option<(usize, String)>,
}

impl<'a> Worktrees<'a> {
    pub(super) fn new(repo: &'a Path) -> Worktrees<'a> {
        Worktrees {
            repo,
            canonical: None,
            probed: HashMap::new(),
            deleted_checkouts: Vec::new(),
            unknown: None,
        }
    }

    /// Learn which of `cwds` were checkout roots since deleted: strictly
    /// under the repository, not re-rooted by any other rule, gone from
    /// disk, and ignored by the repository's git. One git process for
    /// the lot.
    ///
    /// A cwd whose absence could not be established (a stat refused for
    /// another reason) is not asked about: "could not look" is not "gone".
    pub(super) fn learn_deleted_checkouts<'p>(&mut self, cwds: impl IntoIterator<Item = &'p Path>) {
        let mut rels: BTreeMap<String, PathBuf> = BTreeMap::new();
        for cwd in cwds {
            let Ok(rel) = cwd.strip_prefix(self.repo) else {
                continue;
            };
            if rel.components().next().is_none() || self.reroot(cwd) != cwd {
                continue;
            }
            let gone = matches!(
                std::fs::metadata(cwd),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory)
            );
            if gone {
                rels.insert(path_key(cwd, self.repo) + "/", cwd.to_path_buf());
            }
        }
        if rels.is_empty() {
            return;
        }
        let answer = match std::fs::symlink_metadata(self.repo.join(".git")) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => Err(format!("`.git` could not be checked: {e}")),
            Ok(_) => {
                let query: Vec<String> = rels.keys().cloned().collect();
                super::rot::check_ignored(crate::auth::git_program(), self.repo, &query)
            }
        };
        match answer {
            Ok(ignored) => {
                for (rel, cwd) in rels {
                    if ignored.contains(&rel) && !self.deleted_checkouts.contains(&cwd) {
                        self.deleted_checkouts.push(cwd);
                    }
                }
                // Deepest first, so a nested root wins, as it does for a
                // live worktree.
                self.deleted_checkouts
                    .sort_by_key(|p| std::cmp::Reverse(p.components().count()));
            }
            Err(why) => self.unknown = Some((rels.len(), why)),
        }
    }

    /// `path` re-rooted out of the deepest linked worktree holding it,
    /// else unchanged.
    fn reroot(&mut self, path: &Path) -> PathBuf {
        let Ok(rel) = path.strip_prefix(self.repo) else {
            return path.to_path_buf();
        };
        let comps: Vec<std::path::Component> = rel.components().collect();
        // Deepest first, so a worktree nested inside another resolves to
        // the path relative to the inner one.
        let mut unprobeable = vec![false; comps.len() + 1];
        for k in (1..=comps.len()).rev() {
            let root = comps[..k]
                .iter()
                .fold(self.repo.to_path_buf(), |p, c| p.join(c));
            match self.probe(&root) {
                Probe::Worktree => {
                    return comps[k..]
                        .iter()
                        .fold(self.repo.to_path_buf(), |p, c| p.join(c));
                }
                Probe::NotWorktree => {}
                Probe::Unprobeable => unprobeable[k] = true,
            }
        }
        // Rule 1: a deleted checkout the sessions identified.
        if let Some(root) = self.deleted_checkouts.iter().find(|r| path.starts_with(r)) {
            let rest = path.strip_prefix(root).unwrap_or(Path::new(""));
            return rest
                .components()
                .fold(self.repo.to_path_buf(), |p, c| p.join(c));
        }
        // Rule 2: a deleted worktree in Claude Code's layout.
        for i in (0..comps.len().saturating_sub(2)).rev() {
            let name = |j: usize| comps[j].as_os_str().to_string_lossy();
            if name(i) == ".claude"
                && name(i + 1) == "worktrees"
                && !name(i + 2).is_empty()
                && unprobeable[i + 3]
            {
                return comps[..i]
                    .iter()
                    .chain(&comps[i + 3..])
                    .fold(self.repo.to_path_buf(), |p, c| p.join(c));
            }
        }
        path.to_path_buf()
    }

    fn probe(&mut self, dir: &Path) -> Probe {
        if let Some(p) = self.probed.get(dir) {
            return *p;
        }
        let p = self.probe_uncached(dir);
        self.probed.insert(dir.to_path_buf(), p);
        p
    }

    fn probe_uncached(&mut self, dir: &Path) -> Probe {
        match std::fs::metadata(dir) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Probe::NotWorktree,
            Err(_) => return Probe::Unprobeable,
        }
        let dot_git = dir.join(".git");
        match std::fs::symlink_metadata(&dot_git) {
            Ok(m) if m.is_file() => {}
            Ok(_) => return Probe::NotWorktree,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Probe::NotWorktree,
            Err(_) => return Probe::Unprobeable,
        }
        let Ok(text) = std::fs::read_to_string(&dot_git) else {
            return Probe::Unprobeable;
        };
        let Some(target) = text.trim().strip_prefix("gitdir:").map(str::trim) else {
            return Probe::NotWorktree;
        };
        // Relative since git 2.48's `worktree.useRelativePaths`.
        let target = lexical(&dir.join(target));
        let is_worktree_dir = |p: &Path| {
            let name = |p: Option<&Path>| p.and_then(Path::file_name).map(|n| n.to_os_string());
            let parent = p.parent();
            let grandparent = parent.and_then(Path::parent);
            p.file_name().is_some()
                && name(parent).as_deref() == Some(std::ffi::OsStr::new("worktrees"))
                && name(grandparent).as_deref() == Some(std::ffi::OsStr::new(".git"))
        };
        if !is_worktree_dir(&target) {
            return Probe::NotWorktree;
        }
        let Some(owner) = target
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
        else {
            return Probe::NotWorktree;
        };
        if owner == self.repo {
            return Probe::Worktree;
        }
        let repo = self.repo;
        let canonical = self
            .canonical
            .get_or_insert_with(|| std::fs::canonicalize(repo).ok());
        match (canonical.as_deref(), std::fs::canonicalize(owner).ok()) {
            (Some(r), Some(o)) if r == o => Probe::Worktree,
            _ => Probe::NotWorktree,
        }
    }
}

/// `..` and `.` resolved by component, without touching the filesystem.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// A file path's dedup key: relative to the repository with `/`
/// separators when under it, so the same file read in two worktrees, or
/// on two platforms, is one key; absolute otherwise.
fn path_key(path: &Path, repo: &Path) -> String {
    match path.strip_prefix(repo) {
        Ok(rel) if rel.components().next().is_some() => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        _ => path.to_string_lossy().into_owned(),
    }
}

/// Read up to `budget` bytes of a transcript, reading one more so the
/// truncation verdict comes from the read itself (#1213). The partial
/// last line of a cut read fails to parse and is skipped, which is what
/// a cut read should do with it.
fn read_bounded(path: &Path, budget: u64) -> Result<(String, bool), String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("{}: could not open it: {e}", path.display()))?;
    let mut buf = Vec::new();
    file.take(budget + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("{}: could not read it: {e}", path.display()))?;
    let truncated = buf.len() as u64 > budget;
    if truncated {
        buf.truncate(budget as usize);
    }
    // Lossy: one undecodable byte must not cost the session's other
    // records, and no signal here depends on the exact bytes of a
    // record that failed to decode.
    Ok((String::from_utf8_lossy(&buf).into_owned(), truncated))
}

/// One tool call as the extractor sees it.
struct Call {
    record: u64,
    name: String,
    id: Option<String>,
    /// The dedup key: Bash head, file path, pattern, or the tool name.
    key: String,
    dir: PathBuf,
    /// Bash: the command's non-flag tokens after the `cd`/`export`
    /// prefix, for S1's shared-token test.
    tokens: Vec<String>,
}

/// One tool result as the extractor sees it.
struct Outcome {
    record: u64,
    is_error: Option<bool>,
    text: String,
    /// `toolUseResult.stderr` non-empty with `stdout` empty: a failure
    /// the result block did not flag.
    stderr_only: bool,
}

impl Outcome {
    fn failed(&self) -> bool {
        self.is_error == Some(true) || self.stderr_only
    }
}

/// Extract every signal occurrence from one transcript body, and the
/// fingerprint of the session's task: its first user text, as
/// [`task_fingerprint`] hashes it, or `None` when it recorded none.
fn extract(
    session_id: &str,
    session_dir: &Path,
    body: &str,
    worktrees: &mut Worktrees,
) -> (Vec<Row>, Option<String>) {
    let mut calls: Vec<Call> = Vec::new();
    let mut outcomes: HashMap<String, Outcome> = HashMap::new();
    let mut user_texts: Vec<(u64, String)> = Vec::new();

    for (i, line) in body.lines().enumerate() {
        let record = i as u64 + 1;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = rec.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let dir = rec
            .get("cwd")
            .and_then(|c| c.as_str())
            .filter(|c| !c.is_empty())
            .map(|c| reroot_cwd(c, worktrees))
            .unwrap_or_else(|| session_dir.to_path_buf());
        let Some(message) = rec.get("message") else {
            continue;
        };
        match kind {
            "assistant" => {
                for b in blocks_of(message.get("content")) {
                    if let Block::ToolUse { name, id, args } = b {
                        calls.push(call_of(record, name, id, args, &dir, worktrees));
                    }
                }
            }
            "user" => {
                let blocks = blocks_of(message.get("content"));
                let mut any_result = false;
                for b in &blocks {
                    if let Block::ToolResult {
                        text,
                        tool_use_id: Some(id),
                        is_error,
                        ..
                    } = b
                    {
                        any_result = true;
                        let tur = rec.get("toolUseResult");
                        let field = |k: &str| {
                            tur.and_then(|t| t.get(k))
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .unwrap_or("")
                        };
                        let stderr_only = !field("stderr").is_empty() && field("stdout").is_empty();
                        outcomes.insert(
                            id.clone(),
                            Outcome {
                                record,
                                is_error: *is_error,
                                text: text.clone(),
                                stderr_only,
                            },
                        );
                    }
                }
                if any_result {
                    continue;
                }
                // Injected records -- skill bodies, command output,
                // `isMeta` -- are not the user speaking.
                if rec.get("isMeta").and_then(|m| m.as_bool()) == Some(true) {
                    continue;
                }
                let text: String = blocks
                    .iter()
                    .filter_map(|b| match b {
                        Block::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.trim().is_empty() && !text.trim_start().starts_with('<') {
                    user_texts.push((record, text));
                }
            }
            _ => {}
        }
    }

    let mut rows = Vec::new();
    let row = |signal: &'static str, c: &Call, key: String| Row {
        session_id: session_id.to_string(),
        signal,
        dir: c.dir.clone(),
        key,
        aux: Some(c.name.clone()),
        record: Some(c.record),
        record_2: None,
        detail: None,
        tool_use_id: None,
        call_key: None,
    };

    // S1 -- corrected command.
    let bash: Vec<usize> = (0..calls.len())
        .filter(|&i| calls[i].name == "Bash")
        .collect();
    for (bi, &ai) in bash.iter().enumerate() {
        let a = &calls[ai];
        let Some(outcome) = a.id.as_ref().and_then(|id| outcomes.get(id)) else {
            continue;
        };
        // A denied call is not a failed command: nothing ran, so nothing
        // was corrected. The real corpus produced exactly this false
        // pair before the exclusion.
        if !outcome.failed() || is_denial(&outcome.text) {
            continue;
        }
        for &bj in bash.iter().skip(bi + 1).take(CORRECTION_LOOKAHEAD) {
            let b = &calls[bj];
            if b.key == a.key || !shares_token(&a.tokens, &b.tokens) {
                continue;
            }
            // A recorded success, not an absent verdict: `None` is
            // "it did not say", and a correction that did not
            // demonstrably work is not a correction.
            let succeeded =
                b.id.as_ref()
                    .and_then(|id| outcomes.get(id))
                    .is_some_and(|o| o.is_error == Some(false) && !o.stderr_only);
            if !succeeded {
                continue;
            }
            rows.push(Row {
                session_id: session_id.to_string(),
                signal: SIG_CORRECTED,
                dir: a.dir.clone(),
                key: b.key.clone(),
                aux: Some(a.key.clone()),
                record: Some(a.record),
                record_2: Some(b.record),
                detail: Some(clamp(&outcome.text)),
                tool_use_id: None,
                call_key: None,
            });
            break;
        }
    }

    // S2 -- user correction.
    for (record, text) in &user_texts {
        if !is_negation(text) {
            continue;
        }
        let Some(c) = calls
            .iter()
            .rev()
            .find(|c| c.record < *record && record - c.record <= CORRECTION_WINDOW)
        else {
            continue;
        };
        let mut r = row(SIG_USER_CORRECTION, c, c.key.clone());
        r.record = Some(*record);
        r.record_2 = Some(c.record);
        r.detail = Some(clamp(text));
        rows.push(r);
    }

    // S3 and S5 -- denied, and repeated error. A denial is not also an
    // error, or every denial would be reported twice.
    for c in &calls {
        let Some(o) = c.id.as_ref().and_then(|id| outcomes.get(id)) else {
            continue;
        };
        if o.is_error != Some(true) {
            continue;
        }
        if is_denial(&o.text) {
            let mut r = row(SIG_DENIED, c, c.key.clone());
            r.detail = Some(clamp(&o.text));
            r.tool_use_id = c.id.clone();
            rows.push(r);
        } else {
            let mut r = row(SIG_ERROR, c, error_key(&o.text));
            r.record = Some(o.record);
            r.detail = Some(clamp(&o.text));
            r.call_key = Some(c.key.clone());
            rows.push(r);
        }
    }

    // S4 -- repeated search, and the edit census for `edited_dirs`. A
    // file the session also changes, before or after the read, is one it
    // was working on, not one it had to go looking for (#1324). And an
    // early read is evidence of a missing pointer only when the session
    // had to LOOK first: a Grep, a Glob, or a failed Read before it
    // (#1336). Reading a CI workflow during a CI task is the work, not a
    // detour. The nearest such call rides on the row as its `detail`.
    let changed: HashSet<&str> = calls
        .iter()
        .filter(|c| is_change(&c.name))
        .map(|c| c.key.as_str())
        .collect();
    let mut looked: Option<String> = None;
    for (i, c) in calls.iter().enumerate() {
        match c.name.as_str() {
            "Grep" | "Glob" => {
                rows.push(row(SIG_SEARCH, c, c.key.clone()));
                looked = Some(format!("`{}` `{}`", c.name, c.key));
            }
            "Read" => {
                if i < EARLY_CALLS && !changed.contains(c.key.as_str()) {
                    if let Some(after) = &looked {
                        let mut r = row(SIG_SEARCH, c, c.key.clone());
                        r.detail = Some(after.clone());
                        rows.push(r);
                    }
                }
                // A recorded failure, not an absent verdict.
                let failed =
                    c.id.as_ref()
                        .and_then(|id| outcomes.get(id))
                        .is_some_and(|o| o.is_error == Some(true));
                if failed {
                    looked = Some(format!("a failed `Read` of `{}`", c.key));
                }
            }
            "Edit" | "Write" | "MultiEdit" => rows.push(row(SIG_EDIT, c, c.key.clone())),
            _ => {}
        }
    }
    let task = user_texts.first().map(|(_, text)| task_fingerprint(text));
    (rows, task)
}

/// A session's task, for counting replays of one task once (#1337):
/// SHA256 of its opening prompt with whitespace collapsed, as hex. The
/// prompt is the first user text record the extractor keeps -- not an
/// injected `<command>` record or an `isMeta` one, which many unrelated
/// sessions share.
fn task_fingerprint(prompt: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(collapse(prompt).as_bytes())
        .iter()
        .fold(String::new(), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// A `tool_use` block as a [`Call`], with its key and attributed directory.
fn call_of(
    record: u64,
    name: String,
    id: Option<String>,
    args: ToolArgs,
    cwd: &Path,
    worktrees: &mut Worktrees,
) -> Call {
    let repo = worktrees.repo;
    let (key, dir, tokens) = match &args {
        ToolArgs::Bash { command, .. } => {
            let (head, tokens) = command_head(command);
            (head, cwd.to_path_buf(), tokens)
        }
        ToolArgs::Edit { file_path, .. }
        | ToolArgs::MultiEdit { file_path, .. }
        | ToolArgs::Write { file_path, .. }
        | ToolArgs::Read { file_path, .. }
            if !file_path.is_empty() =>
        {
            let p = worktrees.reroot(Path::new(file_path));
            let dir = p
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| cwd.to_path_buf());
            // A path outside the repository attributes to the record's
            // cwd: there is no CLAUDE.md under the repository for it.
            let dir = if dir.starts_with(repo) {
                dir
            } else {
                cwd.to_path_buf()
            };
            (path_key(&p, repo), dir, Vec::new())
        }
        ToolArgs::Grep { pattern, path, .. } | ToolArgs::Glob { pattern, path } => {
            let dir = path
                .as_deref()
                .map(|p| worktrees.reroot(Path::new(p)))
                .filter(|p| p.starts_with(repo))
                .unwrap_or_else(|| cwd.to_path_buf());
            (pattern.clone(), dir, Vec::new())
        }
        _ => (name.clone(), cwd.to_path_buf(), Vec::new()),
    };
    Call {
        record,
        name,
        id,
        key,
        dir,
        tokens,
    }
}

/// A Bash command's head -- its first two tokens after any leading
/// `cd <dir> &&`, `export X=Y &&` and `NAME=value` prefixes -- and its
/// non-flag tokens for the shared-token test.
fn command_head(command: &str) -> (String, Vec<String>) {
    // Segments: `&&`, `;` and newlines all end one. A leading `(` or `{`
    // opens a subshell or group and is not part of the command.
    let segments: Vec<&str> = command
        .split(['\n', ';'])
        .flat_map(|s| s.split("&&"))
        .map(|s| s.trim().trim_start_matches(['(', '{']).trim())
        .filter(|s| !s.is_empty())
        .collect();
    // The command proper, after the `cd`, `export` and assignment
    // prefixes this repository's sessions open with.
    let words = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .skip_while(|t| t.contains('=') && !t.starts_with('-'))
            // A pipe or a redirection ends the command: what follows is
            // the reader of its output, not the command.
            .take_while(|t| !t.starts_with('|') && !t.contains('>'))
            .map(|t| t.trim_end_matches([')', '}']).to_string())
            .filter(|t| !t.is_empty())
            .collect()
    };
    let tokens: Vec<String> = segments
        .iter()
        .map(|s| words(s))
        .find(|w| {
            let first = w.first().map(String::as_str).unwrap_or("");
            !first.is_empty() && !matches!(first, "cd" | "export" | "set" | "source" | ".")
        })
        .unwrap_or_default();
    let head = tokens.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
    let shared = tokens
        .into_iter()
        .filter(|t| !t.starts_with('-') && t.len() >= 2 && !matches!(t.as_str(), "&&" | "||"))
        .collect();
    (head, shared)
}

/// Whether a tool call changes the file it names.
fn is_change(tool: &str) -> bool {
    matches!(tool, "Edit" | "Write" | "MultiEdit")
}

/// Whether a result's text is one of Claude Code's denial phrasings.
fn is_denial(text: &str) -> bool {
    DENIAL_PHRASES.iter().any(|p| text.contains(p))
}

fn shares_token(a: &[String], b: &[String]) -> bool {
    a.iter().any(|t| b.contains(t))
}

/// Whether a user text's first six words hold a negation.
fn is_negation(text: &str) -> bool {
    text.split_whitespace().take(6).any(|w| {
        let w = w
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
            .to_ascii_lowercase();
        NEGATIONS.contains(&w.as_str())
    })
}

/// The S5 dedup key: the first [`ERROR_KEY_CHARS`] characters of the
/// normalised text.
fn error_key(text: &str) -> String {
    let n = normalise_error(text);
    match n.char_indices().nth(ERROR_KEY_CHARS) {
        Some((i, _)) => n[..i].to_string(),
        None => n,
    }
}

/// Error text with what varies between runs removed: `\r\n`, absolute
/// paths, hex ids of eight or more digits, and every digit run, then
/// whitespace collapsed.
fn normalise_error(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let mut out: Vec<String> = Vec::new();
    for tok in text.split_whitespace() {
        let bytes = tok.as_bytes();
        let is_path = tok.starts_with('/')
            || (bytes.len() > 2
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && bytes[2] == b'\\');
        let is_hex = tok.len() >= 8 && tok.chars().all(|c| c.is_ascii_hexdigit());
        if is_path {
            out.push("<path>".into());
        } else if is_hex {
            out.push("<hex>".into());
        } else {
            let mut t = String::new();
            let mut in_digits = false;
            for c in tok.chars() {
                if c.is_ascii_digit() {
                    if !in_digits {
                        t.push('#');
                    }
                    in_digits = true;
                } else {
                    in_digits = false;
                    t.push(c);
                }
            }
            out.push(t);
        }
    }
    out.join(" ")
}

/// Clamp on a character boundary to [`DETAIL_CHARS`], as
/// `transcript::clamp_prompt` does.
fn clamp(text: &str) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth(DETAIL_CHARS) {
        Some((i, _)) => format!("{}…", &trimmed[..i]),
        None => trimmed.to_string(),
    }
}

/// Whitespace-collapsed, for the verbatim "already written" test.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What one session's `claude_advice_ledger` row records beside its id.
struct Ledger<'a> {
    size: i64,
    mtime: i64,
    truncated: bool,
    /// [`task_fingerprint`] of its opening prompt; `None` when it had none.
    task: Option<&'a str>,
}

/// Replace a session's stored rows and its ledger entry, in one
/// transaction so a reader never sees a session with a ledger entry and
/// half its rows.
fn store_rows(
    conn: &Connection,
    session_id: &str,
    rows: &[Row],
    ledger: &Ledger,
    now: &str,
) -> Result<(), rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM claude_advice_signal WHERE session_id = ?1",
        rusqlite::params![session_id],
    )?;
    for r in rows {
        tx.execute(
            "INSERT INTO claude_advice_signal
                (session_id, signal, dir, key, aux, record_index, record_index_2, detail,
                 tool_use_id, call_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                r.session_id,
                r.signal,
                r.dir.to_string_lossy().into_owned(),
                r.key,
                r.aux,
                r.record.map(|v| v as i64),
                r.record_2.map(|v| v as i64),
                r.detail,
                r.tool_use_id,
                r.call_key,
            ],
        )?;
    }
    tx.execute(
        "INSERT INTO claude_advice_ledger
            (session_id, size_bytes, mtime_ms, truncated, analysed_at, rule_version,
             task_fingerprint)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(session_id) DO UPDATE SET
            size_bytes = ?2, mtime_ms = ?3, truncated = ?4, analysed_at = ?5, rule_version = ?6,
            task_fingerprint = ?7",
        rusqlite::params![
            session_id,
            ledger.size,
            ledger.mtime,
            ledger.truncated as i64,
            now,
            RULE_VERSION,
            ledger.task
        ],
    )?;
    tx.commit()
}

/// The task fingerprint of every analysed session that recorded an
/// opening prompt. A session absent from the map had none, and is its
/// own task.
fn load_tasks(
    conn: &Connection,
    analysed: &HashSet<String>,
) -> Result<HashMap<String, String>, String> {
    let mut q = conn
        .prepare(
            "SELECT session_id, task_fingerprint FROM claude_advice_ledger
              WHERE rule_version = ?1 AND task_fingerprint IS NOT NULL",
        )
        .map_err(|e| format!("claude_advice_ledger: {e}"))?;
    let rows = q
        .query_map([RULE_VERSION], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| format!("claude_advice_ledger: {e}"))?;
    let mut out = HashMap::new();
    for row in rows {
        let (id, task) = row.map_err(|e| format!("claude_advice_ledger: {e}"))?;
        if analysed.contains(&id) {
            out.insert(id, task);
        }
    }
    Ok(out)
}

/// Every stored row for the analysed sessions.
fn load_rows(conn: &Connection, analysed: &HashSet<String>) -> Result<Vec<Row>, String> {
    let mut q = conn
        .prepare(
            "SELECT session_id, signal, dir, key, aux, record_index, record_index_2, detail,
                    tool_use_id, call_key
               FROM claude_advice_signal
              ORDER BY session_id, record_index",
        )
        .map_err(|e| format!("claude_advice_signal: {e}"))?;
    let rows = q
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
            ))
        })
        .map_err(|e| format!("claude_advice_signal: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let (session_id, signal, dir, key, aux, record, record_2, detail, tool_use_id, call_key) =
            row.map_err(|e| format!("claude_advice_signal: {e}"))?;
        if !analysed.contains(&session_id) {
            continue;
        }
        let signal = match signal.as_str() {
            SIG_CORRECTED => SIG_CORRECTED,
            SIG_USER_CORRECTION => SIG_USER_CORRECTION,
            SIG_DENIED => SIG_DENIED,
            SIG_SEARCH => SIG_SEARCH,
            SIG_ERROR => SIG_ERROR,
            SIG_EDIT => SIG_EDIT,
            // A signal this build does not know, from a newer one. Not
            // an error; it is simply not one of these findings.
            _ => continue,
        };
        out.push(Row {
            session_id,
            signal,
            dir: PathBuf::from(dir),
            key,
            aux,
            record: record.map(|v| v as u64),
            record_2: record_2.map(|v| v as u64),
            detail,
            tool_use_id,
            call_key,
        });
    }
    Ok(out)
}

/// `PermissionDenied` hook rows for the sessions under the repository:
/// `(session_id, tool_name, tool_use_id)`.
fn hook_denials(
    conn: &Connection,
    sessions: &[SessionRow],
) -> Result<Vec<(String, String, Option<String>)>, String> {
    let ids: HashSet<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
    let mut q = conn
        .prepare(
            "SELECT session_id, tool_name, tool_use_id FROM claude_hook_event
              WHERE event = 'PermissionDenied'",
        )
        .map_err(|e| format!("claude_hook_event: {e}"))?;
    let rows = q
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|e| format!("claude_hook_event: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let (sid, tool, id) = row.map_err(|e| format!("claude_hook_event: {e}"))?;
        if ids.contains(sid.as_str()) {
            out.push((sid, tool.unwrap_or_else(|| "a tool".into()), id));
        }
    }
    Ok(out)
}

/// The subject for a directory: its CLAUDE.md when the scan has one,
/// else the directory itself.
fn subject_for(dir: &Path, scan: &EffectiveScan) -> Subject {
    let file = dir.join("CLAUDE.md");
    match scan.repo.files.iter().find(|f| Path::new(&f.path) == file) {
        Some(f) => Subject::ClaudeMd {
            path: f.path.clone(),
            scope: Scope::Repo,
            section: None,
        },
        None => Subject::Directory {
            path: dir.to_string_lossy().into_owned(),
        },
    }
}

/// The CLAUDE.md-bearing directories under the repository, deepest first,
/// with the root last whether or not it has one.
fn claude_dirs(cx: &Context) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = cx
        .scan
        .repo
        .files
        .iter()
        .filter_map(|f| Path::new(&f.path).parent().map(Path::to_path_buf))
        .filter(|d| d.starts_with(cx.repo) && d != cx.repo)
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    dirs.push(cx.repo.to_path_buf());
    dirs
}

/// The deepest CLAUDE.md-bearing directory that is an ancestor of every
/// attributed directory in a group, else the repository root.
fn placement(dirs: &[&Path], candidates: &[PathBuf], repo: &Path) -> PathBuf {
    candidates
        .iter()
        .find(|c| dirs.iter().all(|d| d.starts_with(c)))
        .cloned()
        .unwrap_or_else(|| repo.to_path_buf())
}

/// What the "already written" test found for one key.
#[derive(Debug, PartialEq, Eq)]
enum Written {
    /// This file holds the key verbatim, as the sentence names it:
    /// `` `<path>` ``, or `` skill `<name>` (`<path>`) `` for a skill.
    In(String),
    /// No file read holds it. Settled: every file in the corpus was read.
    No,
    /// No file read holds it, and these `(path, io error)` could not be
    /// read, so any of them might (#1351).
    Unchecked(Vec<(String, String)>),
}

/// Which CLAUDE.md on the path from the root to `dir`, its imports, or
/// the global and local scopes, or a rule that applies there already
/// holds `key` verbatim.
///
/// A file that does not exist holds nothing, so not-found is a miss. Any
/// other read error is not a miss (#1351): if no readable file holds the
/// key, the answer is [`Written::Unchecked`] naming each such file. A
/// rule `claudemd::rules` could not read is one of those whatever its
/// `paths:`, which were not read either. So is a skill, and a skills
/// directory that could not be listed (#1370).
fn written_in(
    key: &str,
    dir: &Path,
    cx: &Context,
    skills: &Skills,
    cache: &mut HashMap<String, Result<String, String>>,
) -> Written {
    let needle = collapse(key);
    if needle.is_empty() {
        return Written::No;
    }
    let mut files: Vec<(String, &[ImportNode])> = Vec::new();
    for f in &cx.scan.repo.files {
        let Some(parent) = Path::new(&f.path).parent() else {
            continue;
        };
        if dir.starts_with(parent) {
            files.push((f.path.clone(), &f.imports));
        }
    }
    for s in &cx.scan.extra {
        files.push((s.file.path.clone(), &s.file.imports));
    }
    let mut queue: Vec<String> = Vec::new();
    for (path, imports) in files {
        queue.push(path);
        collect_imports(imports, &mut queue);
    }
    // #1370: a skill is where a procedure is written down. Its name, for
    // the sentence, keyed by path.
    let mut skill_names: HashMap<&str, &str> = HashMap::new();
    for (path, name) in &skills.files {
        queue.push(path.clone());
        skill_names.insert(path.as_str(), name.as_str());
    }
    // #1340: the repository's rules a session in `dir` would load.
    let rel = dir.strip_prefix(cx.repo).unwrap_or(dir);
    let rel = rel.to_string_lossy().replace('\\', "/");
    let rules = crate::claudemd::rules::read(cx.repo);
    for rule in &rules.files {
        if rule.applies_to(&rel) {
            queue.push(rule.path.to_string_lossy().into_owned());
        }
    }
    // `Rules::unreadable` is `path (io error)`; the path runs to the first
    // ` (`, and the error is what is inside the outer parentheses.
    let mut unread: Vec<(String, String)> = rules
        .unreadable
        .iter()
        .map(|u| match u.split_once(" (") {
            Some((path, err)) => (
                path.to_string(),
                err.strip_suffix(')').unwrap_or(err).to_string(),
            ),
            None => (u.clone(), "could not be read".to_string()),
        })
        .collect();
    unread.extend(skills.unreadable.iter().cloned());
    for path in queue {
        let content =
            cache
                .entry(path.clone())
                .or_insert_with(|| match std::fs::read_to_string(&path) {
                    Ok(c) => Ok(collapse(&c)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
                    Err(e) => Err(e.to_string()),
                });
        match content {
            Ok(c) if c.contains(&needle) => {
                return Written::In(match skill_names.get(path.as_str()) {
                    Some(name) => format!("skill `{name}` (`{path}`)"),
                    None => format!("`{path}`"),
                })
            }
            Ok(_) => {}
            Err(e) => {
                if !unread.iter().any(|(p, _)| *p == path) {
                    unread.push((path, e.clone()));
                }
            }
        }
    }
    if unread.is_empty() {
        Written::No
    } else {
        Written::Unchecked(unread)
    }
}

/// The skills a session under the repository can load, for the "already
/// written" corpus (#1370): every `SKILL.md` under the repository's
/// `.claude/skills` and the user's `~/.claude/skills`, at any depth.
/// Plugin skills are out of scope: they are not the repository owner's
/// to change. Walked once per pass.
struct Skills {
    /// `(path, name)`, the name being the directory holding the file.
    files: Vec<(String, String)>,
    /// `(path, io error)` for a skills directory that exists and could
    /// not be listed: it might hold the key (#1351).
    unreadable: Vec<(String, String)>,
}

/// How deep a skills walk goes, so a symlink cycle ends.
const SKILL_DEPTH: usize = 8;

fn skills_of(repo: &Path, home: Option<&Path>) -> Skills {
    let mut out = Skills {
        files: Vec::new(),
        unreadable: Vec::new(),
    };
    let roots = std::iter::once(repo).chain(home);
    for root in roots {
        walk_skills(&root.join(".claude").join("skills"), 0, &mut out);
    }
    out
}

fn walk_skills(dir: &Path, depth: usize, out: &mut Skills) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // Absent holds nothing.
        Err(e) if is_gone(&e) => return,
        Err(e) => {
            out.unreadable
                .push((dir.to_string_lossy().into_owned(), e.to_string()));
            return;
        }
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => paths.push(e.path()),
            Err(e) => {
                out.unreadable
                    .push((dir.to_string_lossy().into_owned(), e.to_string()));
            }
        }
    }
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if depth < SKILL_DEPTH {
                walk_skills(&path, depth + 1, out);
            }
        } else if path.file_name() == Some(std::ffi::OsStr::new("SKILL.md")) {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.files.push((path.to_string_lossy().into_owned(), name));
        }
    }
}

fn collect_imports(nodes: &[ImportNode], out: &mut Vec<String>) {
    for n in nodes {
        if let Some(p) = &n.path {
            if !out.contains(p) {
                out.push(p.clone());
                collect_imports(&n.children, out);
            }
        }
    }
}

/// The path a group's key names, when it names one: a file tool's key
/// under S2, S3 or S4. A key relative to the repository is joined onto
/// it component by component; an absolute one stands. `None` for a
/// pattern, a command head, an error text, or a call with no path (its
/// key is its tool name).
fn named_path(signal: &str, key: &str, tool: &str, repo: &Path) -> Option<PathBuf> {
    let path_signal = matches!(signal, SIG_SEARCH | SIG_USER_CORRECTION | SIG_DENIED);
    if !path_signal || !(tool == "Read" || is_change(tool)) || key == tool || key.is_empty() {
        return None;
    }
    let p = Path::new(key);
    Some(if p.is_absolute() {
        p.to_path_buf()
    } else {
        key.split('/')
            .filter(|c| !c.is_empty())
            .fold(repo.to_path_buf(), |acc, c| acc.join(c))
    })
}

/// Whether `path` is established to name nothing. A stat refused for any
/// other reason is not "nothing there": it could not be looked at.
fn resolves_to_nothing(path: &Path) -> bool {
    matches!(std::fs::metadata(path), Err(e) if is_gone(&e))
}

/// Whether a signal is an observation -- what recurred, with no stated
/// fix -- rather than advice. Only a correction that worked (S1) or a
/// user's stated fix (S2), and a denial (S3), carry a rule to write.
/// S5 shows only that something failed repeatedly (#1368); S4 only where
/// sessions spent their first calls, which a session's own task explains
/// as often as a missing pointer does (#1369).
fn is_observation(signal: &str) -> bool {
    matches!(signal, SIG_ERROR | SIG_SEARCH)
}

/// Group the stored rows, apply the thresholds, place and dedup each
/// group, and render the findings.
fn emit(
    stored: &[Row],
    tasks: &HashMap<String, String>,
    denials: &[(String, String, Option<String>)],
    sessions: &[SessionRow],
    analysed: &HashSet<String>,
    cx: &Context,
    short: bool,
) -> Vec<Finding> {
    let candidates = claude_dirs(cx);
    let mut cache: HashMap<String, Result<String, String>> = HashMap::new();
    let skills = skills_of(cx.repo, cx.home);
    let mut out = Vec::new();
    let at_least = if short { "at least " } else { "" };
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    // The denominator every count is stated against (#1369): the analysed
    // sessions, with replays of one task counted once, as the numerator
    // counts them (#1337).
    let denominator = analysed
        .iter()
        .map(|sid| match tasks.get(sid) {
            Some(t) => (true, t.as_str()),
            None => (false, sid.as_str()),
        })
        .collect::<HashSet<_>>()
        .len();

    // (signal, key, aux) -> session -> first row. BTreeMaps so the order
    // a reader sees is the order the keys sort in, run after run.
    let mut groups: BTreeMap<(&str, String, String), BTreeMap<String, Row>> = BTreeMap::new();
    for r in stored {
        if r.signal == SIG_EDIT {
            continue;
        }
        groups
            .entry((r.signal, r.key.clone(), r.aux.clone().unwrap_or_default()))
            .or_default()
            .entry(r.session_id.clone())
            .or_insert_with(|| r.clone());
    }
    // A hook denial whose call the transcript already showed is the same
    // denial; one the transcript did not show (an unread or truncated
    // session) counts its session under the tool alone, since the hook
    // row carries no command.
    let seen_calls: HashSet<(&str, &str)> = stored
        .iter()
        .filter(|r| r.signal == SIG_DENIED)
        .filter_map(|r| {
            r.tool_use_id
                .as_deref()
                .map(|id| (r.session_id.as_str(), id))
        })
        .collect();
    for (sid, tool, id) in denials {
        if id
            .as_deref()
            .is_some_and(|id| seen_calls.contains(&(sid.as_str(), id)))
        {
            continue;
        }
        let dir = sessions
            .iter()
            .find(|s| &s.session_id == sid)
            .map(|s| s.dir.clone())
            .unwrap_or_else(|| cx.repo.to_path_buf());
        groups
            .entry((SIG_DENIED, tool.clone(), tool.clone()))
            .or_default()
            .entry(sid.clone())
            .or_insert_with(|| Row {
                session_id: sid.clone(),
                signal: SIG_DENIED,
                dir,
                key: tool.clone(),
                aux: Some(tool.clone()),
                record: None,
                record_2: None,
                detail: None,
                tool_use_id: id.clone(),
                call_key: None,
            });
    }

    let mut gone: Vec<(PathBuf, String, usize)> = Vec::new();
    let order = [
        SIG_CORRECTED,
        SIG_USER_CORRECTION,
        SIG_DENIED,
        SIG_SEARCH,
        SIG_ERROR,
    ];
    for signal in order {
        for ((sig, key, aux), by_session) in &groups {
            if *sig != signal {
                continue;
            }
            // Replays of one task count once (#1337): sessions sharing a
            // task fingerprint are one run group, in the order of their
            // first session; a session with no opening prompt is its own.
            let mut runs: Vec<Vec<&Row>> = Vec::new();
            let mut index: HashMap<(bool, &str), usize> = HashMap::new();
            for (sid, r) in by_session {
                let task = match tasks.get(sid) {
                    Some(t) => (true, t.as_str()),
                    None => (false, sid.as_str()),
                };
                let i = *index.entry(task).or_insert_with(|| {
                    runs.push(Vec::new());
                    runs.len() - 1
                });
                runs[i].push(r);
            }
            let n = runs.len();
            let total = by_session.len();
            let min = match signal {
                SIG_CORRECTED => MIN_SESSIONS_CORRECTED,
                SIG_USER_CORRECTION => MIN_SESSIONS_USER_CORRECTION,
                SIG_DENIED => MIN_SESSIONS_DENIED,
                SIG_SEARCH => MIN_SESSIONS_SEARCH,
                _ => MIN_SESSIONS_ERROR,
            };
            // Below threshold is not shown at lower confidence; it is
            // not shown.
            if n < min {
                continue;
            }
            // A path that resolves to nothing cannot become a CLAUDE.md
            // pointer (#1335). It is left out, and counted below.
            if let Some(path) = named_path(signal, key, aux, cx.repo) {
                if resolves_to_nothing(&path) {
                    gone.push((path, aux.clone(), n));
                    continue;
                }
            }
            let dirs: Vec<&Path> = by_session.values().map(|r| r.dir.as_path()).collect();
            let dir = placement(&dirs, &candidates, cx.repo);
            let subject = subject_for(&dir, cx.scan);
            // Every count carries its denominator (#1369), in the same
            // unit: distinct tasks among the analysed sessions.
            let mut sessions_phrase = format!(
                "{at_least}{n} of {denominator} analysed session{} under `{}`",
                plural(denominator),
                cx.repo.display()
            );
            if total > n {
                sessions_phrase.push_str(&format!(
                    " ({total} runs; replays of one task counted once)"
                ));
            }
            if dir != cx.repo {
                sessions_phrase.push_str(&format!(", attributed to `{}`", dir.display()));
            }
            let (sentence, dedup_key) = match signal {
                SIG_CORRECTED => (
                    format!("`{aux}` failed and `{key}` followed it in {sessions_phrase}"),
                    key.clone(),
                ),
                SIG_USER_CORRECTION => (
                    format!("a `{aux}` call (`{key}`) was followed by a user correction in {sessions_phrase}"),
                    key.clone(),
                ),
                SIG_DENIED => {
                    if key == aux {
                        (format!("a `{aux}` call was denied in {sessions_phrase}"), String::new())
                    } else {
                        (format!("a `{aux}` call (`{key}`) was denied in {sessions_phrase}"), key.clone())
                    }
                }
                SIG_SEARCH => {
                    if aux == "Read" {
                        (
                            format!("`{key}` was read within the first {EARLY_CALLS} tool calls in {sessions_phrase}"),
                            key.clone(),
                        )
                    } else {
                        (
                            format!("`{key}` was searched with {aux} in {sessions_phrase}"),
                            key.clone(),
                        )
                    }
                }
                _ => {
                    // The failing call, when every session failed on the
                    // same one (#1338). The grouping stays the error text.
                    let mut calls = by_session.values().map(|r| r.call_key.as_deref());
                    let first = calls.next().flatten();
                    let on = match first {
                        Some(c) if calls.all(|k| k == Some(c)) => format!(" on `{c}`"),
                        _ => String::new(),
                    };
                    (
                        format!("recurring error: `{aux}`{on}: `{key}`, in {sessions_phrase}"),
                        key.clone(),
                    )
                }
            };
            // Already written is an observation, not advice (#1339): the
            // rule is doing its job and there is nothing to change. Not
            // found while a corpus file could not be read is not "not
            // written": the finding is Unknown and names each file (#1351).
            //
            // An observation signal recommends nothing whatever the corpus
            // holds, so it is a Note either way; an unread file cannot
            // make a recommendation it never made possibly wrong.
            let observation = is_observation(signal);
            let mut unchecked = Vec::new();
            let written = written_in(&dedup_key, &dir, cx, &skills, &mut cache);
            let (sentence, severity) = match written {
                Written::In(file) => (
                    format!("`{dedup_key}` is already written in {file} ({sentence})"),
                    Severity::Note,
                ),
                Written::No | Written::Unchecked(_) if observation => (sentence, Severity::Note),
                Written::No => (sentence, Severity::Advice),
                Written::Unchecked(files) => {
                    unchecked = files;
                    (
                        format!(
                            "{sentence}; whether `{dedup_key}` is already written could not be \
                             checked"
                        ),
                        Severity::Unknown,
                    )
                }
            };

            let mut evidence = Vec::new();
            for (i, group) in runs.iter().take(MAX_EVIDENCE).enumerate() {
                let r = group[0];
                let first = i == 0;
                let mut measured = match signal {
                    SIG_CORRECTED => {
                        let mut m = format!("first `{aux}` failed");
                        if let (true, Some(d)) = (first, &r.detail) {
                            m.push_str(&format!(" ({d})"));
                        }
                        m.push_str(&format!(", then `{key}` succeeded"));
                        if let Some(b) = r.record_2 {
                            m.push_str(&format!(" at record {b}"));
                        }
                        m
                    }
                    SIG_USER_CORRECTION => match (first, &r.detail, r.record_2) {
                        (true, Some(d), Some(call)) => {
                            format!("after the `{aux}` call at record {call}: \"{d}\"")
                        }
                        (_, _, Some(call)) => {
                            format!("a correction after the `{aux}` call at record {call}")
                        }
                        _ => format!("a correction after a `{aux}` call"),
                    },
                    SIG_DENIED => match (first, &r.detail) {
                        (true, Some(d)) => format!("denied: {d}"),
                        _ => "denied".to_string(),
                    },
                    SIG_SEARCH => match &r.detail {
                        Some(after) => format!("`{aux}` `{key}`, after {after}"),
                        None => format!("`{aux}` `{key}`"),
                    },
                    _ => match (&r.call_key, first, &r.detail) {
                        (Some(c), true, Some(d)) => format!("`{aux}` `{c}`: {d}"),
                        (Some(c), _, _) => {
                            format!("`{aux}` `{c}`: the same error, after normalisation")
                        }
                        (None, true, Some(d)) => format!("error text: {d}"),
                        (None, _, _) => "the same error, after normalisation".to_string(),
                    },
                };
                if group.len() > 1 {
                    measured.push_str(&format!("; {} runs of one task", group.len()));
                }
                evidence.push(Evidence {
                    at: Locator::Session {
                        session_id: r.session_id.clone(),
                        record: r.record,
                    },
                    measured,
                });
            }
            for (path, err) in unchecked {
                evidence.push(Evidence {
                    measured: format!(
                        "could not check whether it is already written: `{path}` ({err})"
                    ),
                    at: Locator::File { path, line: None },
                });
            }
            out.push(Finding::new(
                Check::Transcripts,
                severity,
                subject,
                evidence,
                sentence,
            ));
        }
    }

    if !gone.is_empty() {
        let k = gone.len();
        let sentence = if k == 1 {
            format!(
                "1 finding named a path that no longer exists under `{}` and was left out",
                cx.repo.display()
            )
        } else {
            format!(
                "{k} findings named paths that no longer exist under `{}` and were left out",
                cx.repo.display()
            )
        };
        let evidence = gone
            .iter()
            .take(MAX_EVIDENCE)
            .map(|(path, tool, n)| Evidence {
                at: Locator::File {
                    path: path.to_string_lossy().into_owned(),
                    line: None,
                },
                measured: format!(
                    "`{tool}` of a path that no longer exists, in {at_least}{n} session{}",
                    plural(*n)
                ),
            })
            .collect();
        out.push(Finding::new(
            Check::Transcripts,
            Severity::Note,
            subject_for(cx.repo, cx.scan),
            evidence,
            sentence,
        ));
    }

    // S6 -- the census: sessions per attributed directory, always.
    let mut census: BTreeMap<PathBuf, Vec<&SessionRow>> = BTreeMap::new();
    for s in sessions {
        census.entry(s.dir.clone()).or_default().push(s);
    }
    for (dir, rows) in census {
        let n = rows.len();
        let with_prompt = rows.iter().filter(|s| s.has_prompt).count();
        let read = rows
            .iter()
            .filter(|s| analysed.contains(&s.session_id))
            .count();
        let subject = subject_for(&placement(&[dir.as_path()], &candidates, cx.repo), cx.scan);
        let evidence = rows
            .iter()
            .take(MAX_EVIDENCE)
            .map(|s| Evidence {
                at: Locator::Session {
                    session_id: s.session_id.clone(),
                    record: None,
                },
                measured: if s.has_prompt {
                    "opening prompt recorded".to_string()
                } else {
                    "no opening prompt recorded".to_string()
                },
            })
            .collect();
        out.push(Finding::new(
            Check::Transcripts,
            Severity::Note,
            subject,
            evidence,
            format!(
                "{n} session{} recorded under `{}`; {with_prompt} with an opening prompt, {read} analysed",
                plural(n),
                dir.display()
            ),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claudemd::advice::{run, CheckRun};
    use crate::claudemd::scan_effective_opt;
    use std::fs;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    fn insert_session(
        conn: &Connection,
        id: &str,
        cwd: &str,
        path: Option<&Path>,
        prompt: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO claude_session (session_id, cwd, transcript_path, first_seen_at, opening_prompt)
             VALUES (?1, ?2, ?3, '2026-01-01T00:00:00Z', ?4)",
            rusqlite::params![id, cwd, path.map(|p| p.to_string_lossy().into_owned()), prompt],
        )
        .unwrap();
    }

    /// JSONL record builders. `cwd` on every conversation record, as the
    /// corpus has it (155 of 202 measured, every user/assistant one).
    fn user_text(cwd: &str, text: &str) -> String {
        serde_json::json!({"type":"user","cwd":cwd,"message":{"role":"user","content":text}})
            .to_string()
    }
    fn tool_use(cwd: &str, id: &str, name: &str, input: serde_json::Value) -> String {
        serde_json::json!({"type":"assistant","cwd":cwd,"message":{"role":"assistant","content":[
            {"type":"tool_use","id":id,"name":name,"input":input}]}})
        .to_string()
    }
    fn tool_result(cwd: &str, id: &str, is_error: Option<bool>, text: &str) -> String {
        let mut block = serde_json::json!({"type":"tool_result","tool_use_id":id,"content":text});
        if let Some(e) = is_error {
            block["is_error"] = serde_json::Value::Bool(e);
        }
        serde_json::json!({"type":"user","cwd":cwd,"message":{"role":"user","content":[block]}})
            .to_string()
    }
    fn bash(cwd: &str, id: &str, command: &str) -> String {
        tool_use(cwd, id, "Bash", serde_json::json!({"command":command}))
    }

    /// The founding pair: `yarn lint` fails, `make lint` succeeds. Each
    /// `n` opens with its own prompt, so each is its own task (#1337).
    fn corrected_pair(cwd: &str, n: usize) -> String {
        [
            user_text(cwd, &format!("run the linter, take {n}")),
            bash(cwd, &format!("a{n}"), "yarn lint"),
            tool_result(
                cwd,
                &format!("a{n}"),
                Some(true),
                "eslint: command not found",
            ),
            bash(cwd, &format!("b{n}"), "make lint"),
            tool_result(cwd, &format!("b{n}"), Some(false), "ok"),
        ]
        .join("\n")
            + "\n"
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        p
    }

    /// Replace a session file the ledger has already seen, and make the
    /// change visible to it. The ledger's change key is `(size_bytes,
    /// mtime_ms)`; a rewrite of the same length inside the same
    /// millisecond is invisible to it, which the module docs accept for
    /// real transcripts (append-mostly, written by another process) but
    /// which made a test flaky: `{"command":"git status"}` and
    /// `{"pattern":"**/ci*.yml"}` are both 22 bytes, so a fast rewrite
    /// served the stale rows. Moving mtime a full second past the old one
    /// makes the rewrite a change on every filesystem's granularity.
    fn rewrite(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        let before = fs::metadata(&p).unwrap().modified().unwrap();
        fs::write(&p, body).unwrap();
        fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(before + std::time::Duration::from_secs(1))
            .unwrap();
        p
    }

    fn context<'a>(repo: &'a Path, scan: &'a EffectiveScan, conn: &'a Connection) -> Context<'a> {
        Context {
            repo,
            home: None,
            scan,
            definitions: None,
            conn: Some(conn),
        }
    }

    fn corrected(findings: &[Finding]) -> Vec<&Finding> {
        findings
            .iter()
            .filter(|f| f.finding.contains("failed and `make lint` followed"))
            .collect()
    }

    /// S1 across two sessions is one finding on the root CLAUDE.md, with
    /// both sessions as evidence and the correcting head as its key;
    /// one session alone is below threshold and is not shown at all.
    #[test]
    fn a_corrected_command_in_two_sessions_is_one_finding() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let claude_md = write(repo, "CLAUDE.md", "# rules\n\nUse yarn.\n");
        let cwd = repo.to_string_lossy().into_owned();
        let s1 = write(repo, "s1.jsonl", &corrected_pair(&cwd, 1));
        let conn = db();
        insert_session(&conn, "s1", &cwd, Some(&s1), Some("run the linter"));
        let scan = scan_effective_opt(repo, None);

        let one = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert!(
            corrected(&one).is_empty(),
            "one session is below threshold: {one:?}"
        );

        let s2 = write(repo, "s2.jsonl", &corrected_pair(&cwd, 2));
        insert_session(&conn, "s2", &cwd, Some(&s2), None);
        let two = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let hits = corrected(&two);
        assert_eq!(hits.len(), 1, "{two:#?}");
        let f = hits[0];
        assert_eq!(f.severity, Severity::Advice);
        assert_eq!(f.subject.path(), claude_md.to_string_lossy());
        assert_eq!(
            f.finding,
            format!(
                "`yarn lint` failed and `make lint` followed it in 2 of 2 analysed sessions under `{}`",
                repo.display()
            )
        );
        let ids: Vec<&str> = f
            .evidence
            .iter()
            .map(|e| match &e.at {
                Locator::Session { session_id, .. } => session_id.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(ids, ["s1", "s2"]);
        // The error text rides on the first evidence row only, and the
        // record index is the failed call's line.
        assert_eq!(
            f.evidence[0].measured,
            "first `yarn lint` failed (eslint: command not found), then `make lint` succeeded at record 4"
        );
        assert_eq!(
            f.evidence[1].measured,
            "first `yarn lint` failed, then `make lint` succeeded at record 4"
        );
        assert_eq!(
            f.evidence[0].at,
            Locator::Session {
                session_id: "s1".into(),
                record: Some(2)
            }
        );
        assert!(
            !f.finding.contains("at least"),
            "the pass was complete: {}",
            f.finding
        );
        assert!(!f.finding.contains("already written"));
        // The census is always there, as a count -- an observation, not
        // advice (#1339).
        let census = two
            .iter()
            .find(|f| f.finding.starts_with("2 sessions recorded under"))
            .unwrap_or_else(|| panic!("the census: {two:#?}"));
        assert_eq!(census.severity, Severity::Note);
    }

    /// A session whose cwd is an agent worktree of the repository counts
    /// as the repository's, and the finding sits on the root, not the
    /// worktree.
    #[test]
    fn a_worktree_cwd_re_roots_to_the_repository() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        write(repo, "CLAUDE.md", "# rules\n");
        let cwd = repo.to_string_lossy().into_owned();
        let wt = repo.join(".claude").join("worktrees").join("agent-abc123");
        let wt_s = wt.to_string_lossy().into_owned();
        let conn = db();
        for (n, dir) in [(1, &cwd), (2, &cwd), (3, &wt_s)] {
            let p = write(repo, &format!("s{n}.jsonl"), &corrected_pair(dir, n));
            insert_session(&conn, &format!("s{n}"), dir, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let hits = corrected(&out);
        assert_eq!(hits.len(), 1, "{out:#?}");
        assert!(
            hits[0]
                .finding
                .contains("in 3 of 3 analysed sessions under"),
            "{}",
            hits[0].finding
        );
        assert_eq!(
            hits[0].subject.path(),
            repo.join("CLAUDE.md").to_string_lossy()
        );
        assert!(
            !hits[0].finding.contains("worktrees"),
            "{}",
            hits[0].finding
        );
        // The census re-roots too: one directory, three sessions.
        assert!(
            out.iter()
                .any(|f| f.finding.starts_with("3 sessions recorded under")),
            "{out:#?}"
        );

        // The worktree directory does not exist here, which is the
        // deleted-worktree fallback for Claude Code's own layout.
        let mut w = Worktrees::new(repo);
        assert_eq!(reroot_cwd(&wt_s, &mut w), repo);
        assert_eq!(reroot_cwd(&cwd, &mut w), repo);
        // `.claude/worktrees` itself names no worktree.
        let bare = repo.join(".claude").join("worktrees");
        assert_eq!(reroot_cwd(&bare.to_string_lossy(), &mut w), bare);
        assert_eq!(
            w.reroot(&wt.join("src").join("a.rs")),
            repo.join("src").join("a.rs")
        );
    }

    /// A key already in a CLAUDE.md on the path is reported as written,
    /// naming the file, and stays visible.
    #[test]
    fn a_rule_already_written_is_reported_as_written_not_as_a_gap() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let claude_md = write(
            repo,
            "CLAUDE.md",
            "# rules\n\nRun `make   lint`, not yarn lint.\n",
        );
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in [1, 2] {
            let p = write(repo, &format!("s{n}.jsonl"), &corrected_pair(&cwd, n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let hits = corrected(&out);
        assert_eq!(hits.len(), 1, "{out:#?}");
        assert!(
            hits[0].finding.starts_with(&format!(
                "`make lint` is already written in `{}`",
                claude_md.display()
            )),
            "{}",
            hits[0].finding
        );
        // Kept visible, as an observation: the rule is doing its job,
        // and there is nothing to change (#1339).
        assert_eq!(hits[0].severity, Severity::Note);
        assert!(
            !hits[0].brief.contains("Suggested change"),
            "{}",
            hits[0].brief
        );
    }

    /// The dedup looks through a resolved import and the global scope,
    /// not only the CLAUDE.md files themselves.
    #[test]
    fn dedup_looks_through_imports_and_the_global_scope() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        let home = t.path().join("home");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(home.join(".claude")).unwrap();
        write(&repo, "CLAUDE.md", "@./shared.md\n");
        let shared = write(&repo, "shared.md", "always run make lint\n");
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in [1, 2] {
            let p = write(&repo, &format!("s{n}.jsonl"), &corrected_pair(&cwd, n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(&repo, Some(&home));
        let out = analyse(&conn, &context(&repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        // The import's path as the resolver spells it (`<repo>/./shared.md`),
        // so the assertion is on the file name and the phrase.
        let f = &corrected(&out)[0].finding;
        assert!(
            f.starts_with("`make lint` is already written in `") && f.contains("shared.md` ("),
            "{f}"
        );
        let _ = shared;

        // Now only the global file carries it.
        write(&repo, "shared.md", "nothing here\n");
        let global = write(&home.join(".claude"), "CLAUDE.md", "make lint everywhere\n");
        let scan = scan_effective_opt(&repo, Some(&home));
        let out = analyse(&conn, &context(&repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert!(
            corrected(&out)[0]
                .finding
                .contains(&format!("already written in `{}`", global.display())),
            "{}",
            corrected(&out)[0].finding
        );
    }

    /// #1340: the dedup looks through the repository's `.claude/rules`,
    /// but only a rule a session in the attributed directory would load.
    #[test]
    fn dedup_looks_through_the_rules_that_apply() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        write(repo, "CLAUDE.md", "# rules\n");
        let rules = repo.join(".claude").join("rules");
        fs::create_dir_all(&rules).unwrap();
        let rule = write(&rules, "lint.md", "---\npaths: web/**\n---\nmake lint\n");
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in [1, 2] {
            let p = write(repo, &format!("s{n}.jsonl"), &corrected_pair(&cwd, n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let f = &corrected(&out)[0].finding;
        assert!(!f.contains("already written"), "scoped to web/: {f}");

        write(&rules, "lint.md", "Always make lint.\n");
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let f = &corrected(&out)[0].finding;
        assert!(
            f.starts_with(&format!(
                "`make lint` is already written in `{}`",
                rule.display()
            )),
            "{f}"
        );
    }

    /// #1351: a corpus file that could not be read is not "not written".
    /// With the key only in an unreadable import, the finding is Unknown,
    /// never Advice, and names the file and its io error. A hit in a
    /// readable file still settles it.
    ///
    /// Unix-only: the wall is a permission bit.
    #[test]
    #[cfg(unix)]
    fn an_unreadable_import_makes_already_written_unknown_not_advice() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        write(repo, "CLAUDE.md", "@./shared.md\n");
        let shared = write(repo, "shared.md", "always run make lint\n");
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in [1, 2] {
            let p = write(repo, &format!("s{n}.jsonl"), &corrected_pair(&cwd, n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o000)).unwrap();
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o644)).unwrap();

        let hits = corrected(&out);
        assert_eq!(hits.len(), 1, "{out:#?}");
        let f = hits[0];
        assert_eq!(f.severity, Severity::Unknown, "{}", f.finding);
        assert!(
            f.finding
                .contains("whether `make lint` is already written could not be checked"),
            "{}",
            f.finding
        );
        let named = f
            .evidence
            .iter()
            .find(|e| {
                e.measured
                    .starts_with("could not check whether it is already written: `")
            })
            .unwrap_or_else(|| panic!("{:?}", f.evidence));
        assert!(
            named.measured.contains("shared.md` (") && named.measured.ends_with(')'),
            "{}",
            named.measured
        );
        assert!(!f.brief.contains("transcript named"), "{}", f.brief);

        // A readable file holding the key settles it, unreadable import
        // or not.
        write(repo, "CLAUDE.md", "@./shared.md\n\nmake lint\n");
        let scan = scan_effective_opt(repo, None);
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o000)).unwrap();
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(corrected(&out)[0].severity, Severity::Note, "{out:#?}");
    }

    /// A context carrying a home directory, for the user scope.
    fn context_home<'a>(
        repo: &'a Path,
        home: &'a Path,
        scan: &'a EffectiveScan,
        conn: &'a Connection,
    ) -> Context<'a> {
        Context {
            home: Some(home),
            ..context(repo, scan, conn)
        }
    }

    /// #1370: a skill is where a procedure is written down, so a key a
    /// skill holds is already written. The repository's
    /// `.claude/skills/**/SKILL.md` and the user's
    /// `~/.claude/skills/**/SKILL.md` count, nested ones too, and the hit
    /// names the skill. Plugin skills do not: they are out of scope.
    #[test]
    fn dedup_looks_through_repository_and_user_skills() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        let home = t.path().join("home");
        fs::create_dir_all(&repo).unwrap();
        write(&repo, "CLAUDE.md", "# rules\n");
        let skill_dir = repo.join(".claude").join("skills").join("lint");
        fs::create_dir_all(&skill_dir).unwrap();
        let skill = write(
            &skill_dir,
            "SKILL.md",
            "---\nname: lint\n---\nRun make lint.\n",
        );
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in [1, 2] {
            let p = write(&repo, &format!("s{n}.jsonl"), &corrected_pair(&cwd, n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(&repo, Some(&home));
        let cx = context_home(&repo, &home, &scan, &conn);
        let out = analyse(&conn, &cx, SESSIONS_PER_PASS).unwrap();
        let f = corrected(&out)[0];
        assert_eq!(
            f.finding,
            format!(
                "`make lint` is already written in skill `lint` (`{}`) (`yarn lint` failed and \
                 `make lint` followed it in 2 of 2 analysed sessions under `{}`)",
                skill.display(),
                repo.display()
            )
        );
        assert_eq!(f.severity, Severity::Note);

        // Only the user's skill holds it now, one directory deeper.
        fs::remove_dir_all(repo.join(".claude")).unwrap();
        let nested = home
            .join(".claude")
            .join("skills")
            .join("tools")
            .join("tidy");
        fs::create_dir_all(&nested).unwrap();
        let user_skill = write(&nested, "SKILL.md", "Always make lint first.\n");
        // A plugin's skill holding it too is not consulted.
        let plugin = home
            .join(".claude")
            .join("plugins")
            .join("p")
            .join("skills")
            .join("x");
        fs::create_dir_all(&plugin).unwrap();
        write(&plugin, "SKILL.md", "make lint\n");
        let out = analyse(&conn, &cx, SESSIONS_PER_PASS).unwrap();
        let f = &corrected(&out)[0].finding;
        assert!(
            f.starts_with(&format!(
                "`make lint` is already written in skill `tidy` (`{}`)",
                user_skill.display()
            )),
            "{f}"
        );

        fs::remove_dir_all(home.join(".claude").join("skills")).unwrap();
        let out = analyse(&conn, &cx, SESSIONS_PER_PASS).unwrap();
        let f = corrected(&out)[0];
        assert!(!f.finding.contains("already written"), "{}", f.finding);
        assert_eq!(f.severity, Severity::Advice, "plugins are out of scope");
    }

    /// #1370 with #1351: a skill that could not be read might hold the
    /// rule, so the finding is Unknown, never Advice, and names it.
    #[test]
    #[cfg(unix)]
    fn an_unreadable_skill_makes_already_written_unknown_not_advice() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        write(repo, "CLAUDE.md", "# rules\n");
        let skill_dir = repo.join(".claude").join("skills").join("lint");
        fs::create_dir_all(&skill_dir).unwrap();
        let skill = write(&skill_dir, "SKILL.md", "make lint\n");
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in [1, 2] {
            let p = write(repo, &format!("s{n}.jsonl"), &corrected_pair(&cwd, n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        fs::set_permissions(&skill, fs::Permissions::from_mode(0o000)).unwrap();
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS);
        fs::set_permissions(&skill, fs::Permissions::from_mode(0o644)).unwrap();
        let out = out.unwrap();

        let f = corrected(&out)[0];
        if f.severity == Severity::Note {
            eprintln!("skipped: mode 0o000 did not block the read (running as root?)");
            return;
        }
        assert_eq!(f.severity, Severity::Unknown, "{}", f.finding);
        assert!(
            f.evidence.iter().any(|e| e.measured
                == format!(
                    "could not check whether it is already written: `{}` (Permission denied (os error 13))",
                    skill.display()
                )),
            "{:?}",
            f.evidence
        );
    }

    /// #1351: a `.claude/rules` rule that could not be read might be the
    /// one that holds the key, so it makes the finding Unknown too, and
    /// is named with its error (the `Rules::unreadable` gap).
    #[test]
    #[cfg(unix)]
    fn an_unreadable_rule_makes_already_written_unknown_not_advice() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        write(repo, "CLAUDE.md", "# rules\n");
        let rules = repo.join(".claude").join("rules");
        fs::create_dir_all(&rules).unwrap();
        let rule = write(&rules, "lint.md", "make lint\n");
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in [1, 2] {
            let p = write(repo, &format!("s{n}.jsonl"), &corrected_pair(&cwd, n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        fs::set_permissions(&rule, fs::Permissions::from_mode(0o000)).unwrap();
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        fs::set_permissions(&rule, fs::Permissions::from_mode(0o644)).unwrap();

        let f = corrected(&out)[0];
        assert_eq!(f.severity, Severity::Unknown, "{}", f.finding);
        assert!(
            f.evidence.iter().any(|e| e.measured
                == format!(
                    "could not check whether it is already written: `{}` (Permission denied (os error 13))",
                    rule.display()
                )),
            "{:?}",
            f.evidence
        );
    }

    /// An unreadable transcript is one Unknown finding with the path and
    /// the reason, and the findings from the readable sessions stand
    /// beside it, qualified.
    #[test]
    #[cfg(unix)]
    fn an_unreadable_transcript_is_an_unknown_finding_and_the_others_stand() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        write(repo, "CLAUDE.md", "# rules\n");
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in [1, 2] {
            let p = write(repo, &format!("s{n}.jsonl"), &corrected_pair(&cwd, n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let walled = write(repo, "s4.jsonl", &corrected_pair(&cwd, 4));
        insert_session(&conn, "s4", &cwd, Some(&walled), None);
        fs::set_permissions(&walled, fs::Permissions::from_mode(0o000)).unwrap();
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS);
        fs::set_permissions(&walled, fs::Permissions::from_mode(0o644)).unwrap();
        let out = out.unwrap();

        let unknown: Vec<&Finding> = out
            .iter()
            .filter(|f| f.severity == Severity::Unknown)
            .collect();
        if unknown.is_empty() {
            eprintln!("skipped: mode 0o000 did not block the read (running as root?)");
            return;
        }
        assert_eq!(unknown.len(), 1, "{out:#?}");
        assert!(
            unknown[0].finding.starts_with("1 transcript under `"),
            "{}",
            unknown[0].finding
        );
        assert!(
            unknown[0].evidence[0].measured.contains("s4.jsonl"),
            "{}",
            unknown[0].evidence[0].measured
        );
        assert!(
            unknown[0].evidence[0]
                .measured
                .contains("Permission denied"),
            "{}",
            unknown[0].evidence[0].measured
        );
        assert_eq!(
            unknown[0].evidence[0].at,
            Locator::Session {
                session_id: "s4".into(),
                record: None
            }
        );
        // The others stand, as a floor.
        let hits = corrected(&out);
        assert_eq!(hits.len(), 1, "{out:#?}");
        assert!(
            hits[0]
                .finding
                .contains("in at least 2 of 2 analysed sessions"),
            "{}",
            hits[0].finding
        );
        let coverage = out
            .iter()
            .find(|f| f.finding.starts_with("analysed 2 of 3 sessions under"))
            .expect("the coverage finding");
        assert_eq!(coverage.severity, Severity::Note);
        assert!(
            coverage.finding.ends_with("; 0 truncated at 8 MB"),
            "{}",
            coverage.finding
        );
    }

    /// #1367: a transcript that is gone -- deleted with its worktree's
    /// project directory, or never recorded -- is a session that no
    /// longer exists, not a question about a CLAUDE.md. A hundred of them
    /// are counted in ONE coverage Note, and two that exist but cannot be
    /// read are ONE Unknown listing both: two rows, not a hundred and two.
    #[test]
    #[cfg(unix)]
    fn missing_transcripts_are_counted_and_unreadable_ones_grouped() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        write(repo, "CLAUDE.md", "# rules\n");
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in 0..100 {
            // Half recorded a path that has since been deleted, half
            // recorded none.
            let gone = repo.join("deleted").join(format!("g{n}.jsonl"));
            let path = (n % 2 == 0).then_some(gone.as_path());
            insert_session(&conn, &format!("g{n:03}"), &cwd, path, None);
        }
        let mut walled = Vec::new();
        for n in [1, 2] {
            let p = write(repo, &format!("w{n}.jsonl"), &corrected_pair(&cwd, n));
            insert_session(&conn, &format!("w{n}"), &cwd, Some(&p), None);
            fs::set_permissions(&p, fs::Permissions::from_mode(0o000)).unwrap();
            walled.push(p);
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS);
        for p in &walled {
            fs::set_permissions(p, fs::Permissions::from_mode(0o644)).unwrap();
        }
        let out = out.unwrap();

        let unknown: Vec<&Finding> = out
            .iter()
            .filter(|f| f.severity == Severity::Unknown)
            .collect();
        if unknown.is_empty() {
            eprintln!("skipped: mode 0o000 did not block the read (running as root?)");
            return;
        }
        assert_eq!(unknown.len(), 1, "{out:#?}");
        assert_eq!(
            unknown[0].finding,
            format!(
                "2 transcripts under `{}` could not be read; the counts here are floors without them",
                repo.display()
            )
        );
        let listed: Vec<&Locator> = unknown[0].evidence.iter().map(|e| &e.at).collect();
        assert_eq!(
            listed,
            [
                &Locator::Session {
                    session_id: "w1".into(),
                    record: None
                },
                &Locator::Session {
                    session_id: "w2".into(),
                    record: None
                }
            ]
        );
        assert!(
            unknown[0].evidence[0]
                .measured
                .contains("Permission denied"),
            "{}",
            unknown[0].evidence[0].measured
        );
        // Its brief says what to make readable, not what to edit.
        assert!(
            unknown[0]
                .brief
                .contains("Make the transcripts named in the evidence readable"),
            "{}",
            unknown[0].brief
        );

        let coverage: Vec<&Finding> = out
            .iter()
            .filter(|f| f.finding.starts_with("analysed "))
            .collect();
        assert_eq!(coverage.len(), 1, "{out:#?}");
        assert_eq!(coverage[0].severity, Severity::Note);
        assert_eq!(
            coverage[0].finding,
            format!(
                "analysed 0 of 102 sessions under `{}`; 0 truncated at 8 MB; 100 had no \
                 transcript on disk and were skipped",
                repo.display()
            )
        );
        // A sample behind the count, after the counts row.
        assert_eq!(coverage[0].evidence.len(), 1 + MAX_EVIDENCE);
        assert!(coverage[0].evidence[1..].iter().all(|e| {
            e.measured == "no transcript path is recorded for this session"
                || e.measured.ends_with(": no longer exists")
        }));
        // Nothing mentions a session one row at a time.
        assert!(
            !out.iter().any(|f| f.finding.starts_with("session `")),
            "{out:#?}"
        );
    }

    /// No sessions under the repository is its own finding, never an
    /// empty list.
    #[test]
    fn no_sessions_under_the_repository_is_its_own_finding() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let conn = db();
        // A session under a DIFFERENT directory, so the query has rows to
        // reject.
        insert_session(&conn, "other", "/home/octocat/other-repo", None, None);
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert_eq!(out.len(), 1, "{out:#?}");
        assert_eq!(
            out[0].finding,
            format!(
                "no Claude Code sessions were recorded under `{}`",
                repo.display()
            )
        );
        assert_eq!(out[0].severity, Severity::Note);
        assert_eq!(
            out[0].subject,
            Subject::Directory {
                path: repo.to_string_lossy().into_owned()
            }
        );
    }

    /// `is_error` absent is neither a failure nor a success. A reader
    /// that folded absent into `false` would count the second session's
    /// `make lint` as a success and emit the finding; one that folded it
    /// into `true` would count its `yarn lint` as a failure.
    #[test]
    fn absent_is_error_is_neither_failure_nor_success() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        let s1 = write(repo, "s1.jsonl", &corrected_pair(&cwd, 1));
        insert_session(&conn, "s1", &cwd, Some(&s1), None);
        // B's result says nothing: not a demonstrated correction.
        let s2 = write(
            repo,
            "s2.jsonl",
            &[
                bash(&cwd, "a2", "yarn lint"),
                tool_result(&cwd, "a2", Some(true), "eslint: command not found"),
                bash(&cwd, "b2", "make lint"),
                tool_result(&cwd, "b2", None, "ok"),
            ]
            .join("\n"),
        );
        insert_session(&conn, "s2", &cwd, Some(&s2), None);
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert!(
            corrected(&out).is_empty(),
            "an absent verdict on B is not a success: {out:#?}"
        );

        // A's result says nothing: not a failure either.
        write(
            repo,
            "s2.jsonl",
            &[
                bash(&cwd, "a2", "yarn lint"),
                tool_result(&cwd, "a2", None, "eslint: command not found"),
                bash(&cwd, "b2", "make lint"),
                tool_result(&cwd, "b2", Some(false), "ok"),
            ]
            .join("\n"),
        );
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert!(
            corrected(&out).is_empty(),
            "an absent verdict on A is not a failure: {out:#?}"
        );

        // And the positive, through stderr-with-no-stdout, which IS a
        // recorded failure.
        let mut a =
            serde_json::from_str::<serde_json::Value>(&tool_result(&cwd, "a2", None, "")).unwrap();
        a["toolUseResult"] = serde_json::json!({"stdout":"","stderr":"eslint: command not found","interrupted":false});
        write(
            repo,
            "s2.jsonl",
            &[
                bash(&cwd, "a2", "yarn lint"),
                a.to_string(),
                bash(&cwd, "b2", "make lint"),
                tool_result(&cwd, "b2", Some(false), "ok"),
            ]
            .join("\n"),
        );
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert_eq!(corrected(&out).len(), 1, "{out:#?}");
    }

    /// A pair that sits past the 8 MB budget is not seen, the session is
    /// counted as truncated, and the coverage finding says so.
    #[test]
    fn a_pair_past_the_budget_is_not_seen_and_the_pass_says_so() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        let s1 = write(repo, "s1.jsonl", &corrected_pair(&cwd, 1));
        insert_session(&conn, "s1", &cwd, Some(&s1), None);
        // 9 MB of filler records, then the pair.
        let filler = user_text(&cwd, &"x".repeat(4_000));
        let mut big = String::new();
        while big.len() < 9 * 1024 * 1024 {
            big.push_str(&filler);
            big.push('\n');
        }
        big.push_str(&corrected_pair(&cwd, 2));
        let s2 = write(repo, "s2.jsonl", &big);
        insert_session(&conn, "s2", &cwd, Some(&s2), None);
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert!(
            corrected(&out).is_empty(),
            "the pair past the cap must not be seen: {out:#?}"
        );
        let coverage = out
            .iter()
            .find(|f| f.finding.starts_with("analysed 2 of 2 sessions under"))
            .expect("the coverage finding");
        assert_eq!(coverage.severity, Severity::Note);
        assert!(
            coverage.finding.ends_with("; 1 truncated at 8 MB"),
            "{}",
            coverage.finding
        );
        let cut: i64 = conn
            .query_row(
                "SELECT truncated FROM claude_advice_ledger WHERE session_id = 's2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            cut, 1,
            "truncation is remembered so the next open says so too"
        );
    }

    /// The ledger: an unchanged transcript is not re-read; a changed one
    /// is, and its old rows are replaced.
    #[test]
    fn the_ledger_skips_an_unchanged_transcript_and_rereads_a_changed_one() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        let s1 = write(repo, "s1.jsonl", &corrected_pair(&cwd, 1));
        insert_session(&conn, "s1", &cwd, Some(&s1), None);
        let scan = scan_effective_opt(repo, None);
        let cx = context(repo, &scan, &conn);
        analyse(&conn, &cx, SESSIONS_PER_PASS).unwrap();
        let first: String = conn
            .query_row(
                "SELECT analysed_at FROM claude_advice_ledger WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let rows = || -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM claude_advice_signal WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        // The corrected pair is one S1 row and one S5 row: the failed
        // call's error is a repeated-error candidate too.
        assert_eq!(rows(), 2);

        // Unchanged: the ledger entry is untouched.
        analyse(&conn, &cx, SESSIONS_PER_PASS).unwrap();
        let second: String = conn
            .query_row(
                "SELECT analysed_at FROM claude_advice_ledger WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(first, second, "an unchanged transcript was re-read");

        // Changed (a different size): re-read, rows replaced, not added.
        let mut body = corrected_pair(&cwd, 1);
        body.push_str(&corrected_pair(&cwd, 9));
        fs::write(&s1, body).unwrap();
        analyse(&conn, &cx, SESSIONS_PER_PASS).unwrap();
        assert_eq!(
            rows(),
            4,
            "the changed transcript's rows were replaced, not added to"
        );
        let size: i64 = conn
            .query_row(
                "SELECT size_bytes FROM claude_advice_ledger WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(size, fs::metadata(&s1).unwrap().len() as i64);
    }

    /// The per-pass cap: what was read is answered, what was not is
    /// counted, and the next pass picks it up.
    #[test]
    fn the_pass_cap_leaves_the_rest_for_the_next_open() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in [1, 2] {
            let p = write(repo, &format!("s{n}.jsonl"), &corrected_pair(&cwd, n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        let cx = context(repo, &scan, &conn);
        let out = analyse(&conn, &cx, 1).unwrap();
        assert!(
            corrected(&out).is_empty(),
            "one read session is below threshold: {out:#?}"
        );
        let coverage = out
            .iter()
            .find(|f| f.finding.starts_with("analysed 1 of 2 sessions under"))
            .expect("the coverage finding");
        assert_eq!(coverage.severity, Severity::Note);
        assert!(
            coverage
                .finding
                .ends_with("; 0 truncated at 8 MB; 1 not yet read (at most 1 are read per open)"),
            "{}",
            coverage.finding
        );
        // The census qualifies too.
        assert!(
            out.iter()
                .any(|f| f.finding.contains("2 sessions recorded under")
                    && f.finding.ends_with("1 analysed")),
            "{out:#?}"
        );

        let out = analyse(&conn, &cx, 1).unwrap();
        assert_eq!(corrected(&out).len(), 1, "{out:#?}");
        assert!(
            !out.iter().any(|f| f.finding.starts_with("analysed")),
            "{out:#?}"
        );
    }

    /// No store is Unknown for this check, and the report says so
    /// without dropping the other producers.
    #[test]
    fn no_store_is_unknown_not_clean() {
        let t = tempfile::tempdir().unwrap();
        let scan = scan_effective_opt(t.path(), None);
        let cx = Context {
            repo: t.path(),
            home: None,
            scan: &scan,
            definitions: None,
            conn: None,
        };
        let report = run(&cx);
        let c = report
            .checks
            .iter()
            .find(|c| c.check == Check::Transcripts)
            .unwrap();
        assert_eq!(
            c.run,
            CheckRun::Unknown {
                reason: "no session store was available to this run".into()
            }
        );
        assert!(report.is_partial());
    }

    fn rows_of(body: &str, cwd: &str, repo: &Path) -> Vec<Row> {
        extract("s", Path::new(cwd), body, &mut Worktrees::new(repo)).0
    }

    /// S2: a negation in the first six words, within five records of a
    /// tool call, keys on that call's head; a later or unrelated text
    /// does not.
    #[test]
    fn a_user_correction_is_keyed_on_the_preceding_call() {
        let cwd = "/home/octocat/hello-world";
        let body = [
            bash(cwd, "a", "git push origin main"),
            tool_result(cwd, "a", Some(false), "pushed"),
            user_text(cwd, "No, don't push to main. Open a PR."),
            user_text(cwd, "Thanks, that's fine."),
        ]
        .join("\n");
        let rows = rows_of(&body, cwd, Path::new(cwd));
        let s2: Vec<&Row> = rows
            .iter()
            .filter(|r| r.signal == SIG_USER_CORRECTION)
            .collect();
        assert_eq!(s2.len(), 1, "{rows:?}");
        assert_eq!(s2[0].key, "git push");
        assert_eq!(s2[0].aux.as_deref(), Some("Bash"));
        assert_eq!(s2[0].record, Some(3));
        assert_eq!(s2[0].record_2, Some(1));
        assert_eq!(
            s2[0].detail.as_deref(),
            Some("No, don't push to main. Open a PR.")
        );

        // Six records later, it is not about the call.
        let far = [
            bash(cwd, "a", "git push origin main"),
            tool_result(cwd, "a", Some(false), "pushed"),
            user_text(cwd, "ok"),
            user_text(cwd, "ok"),
            user_text(cwd, "ok"),
            user_text(cwd, "ok"),
            user_text(cwd, "ok"),
            user_text(cwd, "No, stop."),
        ]
        .join("\n");
        assert!(rows_of(&far, cwd, Path::new(cwd))
            .iter()
            .all(|r| r.signal != SIG_USER_CORRECTION));
        // Injected text is not the user.
        let injected = [
            bash(cwd, "a", "git push"),
            user_text(
                cwd,
                "<local-command-stdout>no such command</local-command-stdout>",
            ),
        ]
        .join("\n");
        assert!(rows_of(&injected, cwd, Path::new(cwd))
            .iter()
            .all(|r| r.signal != SIG_USER_CORRECTION));
        assert!(is_negation("do not push"));
        assert!(!is_negation("push it now please and thanks, no wait"));
    }

    /// S3: a denial phrasing is a denial and not also a repeated error;
    /// a hook row for the same call is counted once, and a hook row for
    /// a call the transcript did not show counts its session under the
    /// tool alone.
    #[test]
    fn a_denial_is_s3_and_not_s5_and_hook_rows_join_by_call() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let cwd = repo.to_string_lossy().into_owned();
        let denied = "Permission for this action was denied by the Claude Code auto mode classifier. Reason: [Permission Grant].";
        let body = |n: usize| {
            [
                bash(&cwd, &format!("toolu_{n}"), "git push origin main"),
                tool_result(&cwd, &format!("toolu_{n}"), Some(true), denied),
            ]
            .join("\n")
        };
        let rows = rows_of(&body(1), &cwd, repo);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].signal, SIG_DENIED);
        // A denied call followed by a succeeding one that shares a token
        // is not a corrected command: nothing ran to be corrected.
        let then_ok = [
            body(1),
            bash(&cwd, "ok", "git push origin feature"),
            tool_result(&cwd, "ok", Some(false), "pushed"),
        ]
        .join("\n");
        assert!(
            rows_of(&then_ok, &cwd, repo)
                .iter()
                .all(|r| r.signal != SIG_CORRECTED),
            "{:?}",
            rows_of(&then_ok, &cwd, repo)
        );
        assert_eq!(rows[0].key, "git push");
        assert_eq!(rows[0].tool_use_id.as_deref(), Some("toolu_1"));

        let conn = db();
        for n in [1, 2, 3] {
            let p = write(repo, &format!("s{n}.jsonl"), &body(n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        // Hook rows: two for calls the transcripts show (no double
        // count), one for session s3's call the transcript did not
        // show, and one for s4, which has no transcript at all.
        insert_session(&conn, "s4", &cwd, None, None);
        for (sid, id) in [
            ("s1", "toolu_1"),
            ("s2", "toolu_2"),
            ("s3", "toolu_other"),
            ("s4", "toolu_9"),
        ] {
            conn.execute(
                "INSERT INTO claude_hook_event (session_id, event, at, tool_name, tool_use_id)
                 VALUES (?1, 'PermissionDenied', ?2, 'Bash', ?3)",
                rusqlite::params![sid, format!("2026-01-01T00:00:0{}Z", id.len() % 10), id],
            )
            .unwrap();
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let head_keyed = out
            .iter()
            .find(|f| {
                f.finding
                    .contains("a `Bash` call (`git push`) was denied in")
            })
            .expect("the head-keyed denial");
        assert!(
            head_keyed
                .finding
                .contains("at least 3 of 3 analysed sessions"),
            "{}",
            head_keyed.finding
        );
        assert_eq!(head_keyed.evidence.len(), 3);
        assert!(head_keyed.evidence[0]
            .measured
            .starts_with("denied: Permission for this action"));
        assert_eq!(head_keyed.evidence[1].measured, "denied");
        // Two hook-only rows are below the threshold of three, so no
        // tool-only finding.
        assert!(
            !out.iter()
                .any(|f| f.finding.contains("a `Bash` call was denied in")),
            "{out:#?}"
        );
        // s4 has no transcript: a session that is gone, counted in the
        // coverage Note with its reason, never an Unknown (#1367).
        assert!(
            !out.iter().any(|f| f.severity == Severity::Unknown),
            "{out:#?}"
        );
        let coverage = out
            .iter()
            .find(|f| f.finding.starts_with("analysed 3 of 4 sessions"))
            .unwrap_or_else(|| panic!("the coverage Note: {out:#?}"));
        assert!(
            coverage
                .finding
                .contains("; 1 had no transcript on disk and was skipped"),
            "{}",
            coverage.finding
        );
        assert!(coverage.evidence.iter().any(|e| e.at
            == Locator::Session {
                session_id: "s4".into(),
                record: None
            }
            && e.measured == "no transcript path is recorded for this session"));
    }

    /// S4 and S5: identical patterns and early reads are searches; an
    /// error text is keyed on its normalised first 80 characters.
    #[test]
    fn repeated_searches_and_errors_are_keyed_and_normalised() {
        let cwd = "/home/octocat/hello-world";
        let repo = Path::new(cwd);
        let mut records = vec![tool_use(
            cwd,
            "g",
            "Grep",
            serde_json::json!({"pattern":"fn main","path":cwd}),
        )];
        for i in 0..EARLY_CALLS {
            records.push(tool_use(
                cwd,
                &format!("r{i}"),
                "Read",
                serde_json::json!({"file_path":format!("{cwd}/src/f{i}.rs")}),
            ));
        }
        // The eleventh call: past the early window.
        records.push(tool_use(
            cwd,
            "late",
            "Read",
            serde_json::json!({"file_path":format!("{cwd}/README.md")}),
        ));
        records.push(bash(cwd, "e", "cargo test"));
        records.push(tool_result(cwd, "e", Some(true), "error[E0425]: cannot find value `x` in /home/octocat/hello-world/src/main.rs:12:5\r\nrun 3 tests, id deadbeefcafe"));
        let rows = rows_of(&records.join("\n"), cwd, repo);
        let search: Vec<&Row> = rows.iter().filter(|r| r.signal == SIG_SEARCH).collect();
        assert_eq!(search.len(), 1 + EARLY_CALLS - 1, "{search:?}");
        assert_eq!(search[0].key, "fn main");
        assert_eq!(search[0].aux.as_deref(), Some("Grep"));
        assert!(
            search.iter().all(|r| r.key != "README.md"),
            "the late read is not orientation"
        );
        assert_eq!(search[1].dir, repo.join("src"));
        assert_eq!(
            search[1].key, "src/f0.rs",
            "keyed relative to the repository"
        );
        let err: Vec<&Row> = rows.iter().filter(|r| r.signal == SIG_ERROR).collect();
        assert_eq!(err.len(), 1);
        assert_eq!(
            err[0].key,
            "error[E#]: cannot find value `x` in <path> run # tests, id <hex>"
        );
        assert_eq!(err[0].record, Some((records.len()) as u64));
        assert!(err[0]
            .detail
            .as_deref()
            .unwrap()
            .starts_with("error[E0425]"));
        assert_eq!(error_key(&"z".repeat(200)).chars().count(), ERROR_KEY_CHARS);
    }

    /// `command_head` strips the `cd … &&` and env prefixes this
    /// repository's sessions open with, and the shared-token test sees
    /// through flags.
    #[test]
    fn command_head_strips_prefixes_and_keeps_shared_tokens() {
        let (head, tokens) = command_head("cd /home/octocat/hello-world && export FOO=1 && CARGO_TARGET_DIR=/x cargo test --lib -- --nocapture");
        assert_eq!(head, "cargo test");
        assert_eq!(
            tokens,
            ["cargo", "test"],
            "flags and `--` are not shared tokens"
        );
        assert_eq!(command_head("yarn lint").0, "yarn lint");
        assert_eq!(command_head("ls").0, "ls");
        // The shapes the real corpus produced: `;` between segments, a
        // subshell, a pipe.
        let (head, tokens) =
            command_head("export FOO=1; (cd src-tauri && make lint) 2>&1 | tail -5");
        assert_eq!(head, "make lint");
        assert_eq!(tokens, ["make", "lint"]);
        assert_eq!(command_head("cd a\ncargo test").0, "cargo test");
        assert_eq!(
            command_head("cd a && export B=1").0,
            "",
            "nothing but prefixes"
        );
        assert!(shares_token(
            &command_head("yarn lint").1,
            &command_head("make lint").1
        ));
        assert!(!shares_token(
            &command_head("yarn lint").1,
            &command_head("make test").1
        ));
        assert_eq!(command_head("").0, "");
    }

    /// `edited_dirs` lists a directory with three or more Edit/Write
    /// calls across the read sessions, and not one with two.
    #[test]
    fn edited_dirs_lists_directories_with_three_or_more_edits() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let cwd = repo.to_string_lossy().into_owned();
        let src = repo.join("src");
        let docs = repo.join("docs");
        let conn = db();
        let edit = |id: &str, path: &Path| {
            tool_use(
                &cwd,
                id,
                "Edit",
                serde_json::json!({"file_path":path.to_string_lossy(),"old_string":"a","new_string":"b"}),
            )
        };
        let s1 = write(
            repo,
            "s1.jsonl",
            &[
                edit("e1", &src.join("a.rs")),
                edit("e2", &src.join("b.rs")),
                edit("d1", &docs.join("x.md")),
            ]
            .join("\n"),
        );
        let s2 = write(
            repo,
            "s2.jsonl",
            &[
                tool_use(&cwd, "w1", "Write", serde_json::json!({"file_path":src.join("c.rs").to_string_lossy(),"content":"x"})),
                edit("d2", &docs.join("y.md")),
            ]
            .join("\n"),
        );
        insert_session(&conn, "s1", &cwd, Some(&s1), None);
        insert_session(&conn, "s2", &cwd, Some(&s2), None);
        assert!(
            edited_dirs(&conn, repo).unwrap().is_empty(),
            "nothing read yet is nothing recorded"
        );
        let scan = scan_effective_opt(repo, None);
        analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert_eq!(edited_dirs(&conn, repo).unwrap(), vec![src.clone()]);
        // Scoped to the repository asked about.
        assert!(edited_dirs(&conn, Path::new("/home/octocat/elsewhere"))
            .unwrap()
            .is_empty());
    }

    /// A file-path call attributes to the deepest CLAUDE.md-bearing
    /// ancestor, and a group spanning two subdirectories to their common
    /// one.
    #[test]
    fn a_file_path_attributes_to_the_deepest_claude_md_ancestor() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        write(repo, "CLAUDE.md", "# root\n");
        fs::create_dir_all(repo.join("src-tauri").join("src")).unwrap();
        let inner = write(&repo.join("src-tauri"), "CLAUDE.md", "# inner\n");
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        let file = repo.join("src-tauri").join("src").join("lib.rs");
        // It exists: a path that resolves to nothing is not advice (#1335).
        fs::write(&file, "").unwrap();
        for n in [1, 2, 3] {
            // A search first: an early read counts only after one (#1336).
            let body = [
                tool_use(
                    &cwd,
                    &format!("g{n}"),
                    "Glob",
                    serde_json::json!({"pattern":"**/lib.rs"}),
                ),
                tool_use(
                    &cwd,
                    &format!("r{n}"),
                    "Read",
                    serde_json::json!({"file_path":file.to_string_lossy()}),
                ),
            ]
            .join("\n");
            let p = write(repo, &format!("s{n}.jsonl"), &body);
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let read = out
            .iter()
            .find(|f| f.finding.contains("was read within the first"))
            .expect("the early-read finding");
        assert_eq!(
            read.subject.path(),
            inner.to_string_lossy(),
            "{}",
            read.finding
        );
        // The denominator is the repository's analysed sessions, and the
        // attribution is said separately, so "3 of 3" is not read as
        // three sessions run inside `src-tauri` (#1369).
        assert_eq!(
            read.finding,
            format!(
                "`src-tauri/src/lib.rs` was read within the first {EARLY_CALLS} tool calls in 3 \
                 of 3 analysed sessions under `{}`, attributed to `{}`",
                repo.display(),
                repo.join("src-tauri").display()
            )
        );
        // An observation, not advice (#1369): an early read carries no
        // stated fix.
        assert_eq!(read.severity, Severity::Note);
        assert!(!read.brief.contains("Suggested change"), "{}", read.brief);

        // A second group of the same key from the root's cwd sits above
        // both: the placement is the common ancestor.
        let dirs = [repo.join("src-tauri").join("src"), repo.join("src")];
        let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
        assert_eq!(
            placement(&refs, &claude_dirs(&context(repo, &scan, &conn)), repo),
            repo
        );
    }

    /// A linked worktree of `repo` at `repo/<rel>`, as `git worktree
    /// add` leaves it: a `.git` FILE naming `<repo>/.git/worktrees/<name>`.
    fn worktree(repo: &Path, rel: &[&str], name: &str) -> PathBuf {
        let wt = rel.iter().fold(repo.to_path_buf(), |p, c| p.join(c));
        fs::create_dir_all(wt.join("src")).unwrap();
        let admin = repo.join(".git").join("worktrees").join(name);
        fs::create_dir_all(&admin).unwrap();
        fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", admin.to_string_lossy()),
        )
        .unwrap();
        wt
    }

    fn early_reads(findings: &[Finding]) -> Vec<&Finding> {
        findings
            .iter()
            .filter(|f| f.finding.contains("was read within the first"))
            .collect()
    }

    /// #1324: three sessions that each read a file early and then edit
    /// it were working on it, not searching for it. No S4 finding, and
    /// the edit census still sees the edits, re-rooted out of the
    /// worktree.
    #[test]
    fn a_read_of_a_file_the_session_edits_is_not_a_search() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src").join("a.ts"), "").unwrap();
        let wt = worktree(repo, &[".wt", "t1"], "t1");
        let file = wt.join("src").join("a.ts");
        let wt_s = wt.to_string_lossy().into_owned();
        let conn = db();
        let glob = |n: usize| {
            tool_use(
                &wt_s,
                &format!("g{n}"),
                "Glob",
                serde_json::json!({"pattern":"**/a.ts"}),
            )
        };
        for n in [1, 2, 3] {
            let body = [
                glob(n),
                tool_use(
                    &wt_s,
                    &format!("r{n}"),
                    "Read",
                    serde_json::json!({"file_path":file.to_string_lossy()}),
                ),
                tool_use(
                    &wt_s,
                    &format!("e{n}"),
                    "Edit",
                    serde_json::json!({"file_path":file.to_string_lossy(),"old_string":"a","new_string":"b"}),
                ),
            ]
            .join("\n");
            let p = write(repo, &format!("s{n}.jsonl"), &body);
            insert_session(&conn, &format!("s{n}"), &wt_s, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert!(early_reads(&out).is_empty(), "{out:#?}");
        assert_eq!(edited_dirs(&conn, repo).unwrap(), vec![repo.join("src")]);
        // The census re-roots the worktree cwd to the repository.
        assert!(
            out.iter().any(|f| f.finding
                == format!(
                    "3 sessions recorded under `{}`; 0 with an opening prompt, 3 analysed",
                    repo.display()
                )),
            "{out:#?}"
        );

        // The control: the same reads without the edits are a finding.
        let only = |n: usize| {
            [
                glob(n),
                tool_use(
                    &wt_s,
                    &format!("r{n}"),
                    "Read",
                    serde_json::json!({"file_path":file.to_string_lossy()}),
                ),
            ]
            .join("\n")
        };
        for n in [1, 2, 3] {
            rewrite(repo, &format!("s{n}.jsonl"), &only(n));
        }
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert_eq!(early_reads(&out).len(), 1, "{out:#?}");
    }

    /// #1324: the same file read early in two worktrees and in the
    /// repository itself is ONE key, `src/a.ts`, counted over three
    /// sessions and placed on the repository, never on a worktree.
    #[test]
    fn early_reads_in_two_worktrees_and_the_repository_are_one_key() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src").join("a.ts"), "").unwrap();
        let w1 = worktree(repo, &[".wt", "t1"], "t1");
        let w2 = worktree(repo, &[".wt", "t2"], "t2");
        let conn = db();
        for (n, root) in [(1, &w1), (2, &w2), (3, &repo.to_path_buf())] {
            let cwd = root.to_string_lossy().into_owned();
            let body = searched_then_read(root, n);
            let p = write(repo, &format!("s{n}.jsonl"), &body);
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        // A fourth session that read nothing: it is in the denominator
        // and not the count (#1369).
        let quiet = write(repo, "s4.jsonl", &user_text(&repo.to_string_lossy(), "hi"));
        insert_session(&conn, "s4", &repo.to_string_lossy(), Some(&quiet), None);
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let hits = early_reads(&out);
        assert_eq!(hits.len(), 1, "{out:#?}");
        assert_eq!(
            hits[0].finding,
            format!(
                "`src/a.ts` was read within the first {EARLY_CALLS} tool calls in 3 of 4 analysed sessions under `{}`",
                repo.display()
            )
        );
        assert_eq!(hits[0].severity, Severity::Note);
        assert_eq!(hits[0].evidence.len(), 3);
        assert!(
            out.iter().all(|f| !f.finding.contains(".wt")),
            "no finding names a worktree: {out:#?}"
        );
    }

    /// A worktree is what its `.git` file says, not what its directory is
    /// called: a relative `gitdir:` counts, a submodule, a worktree of
    /// another repository and a plain directory do not, and the deepest
    /// worktree wins.
    #[test]
    fn a_worktree_is_identified_by_its_git_file_not_its_name() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let rel = |root: &Path| root.join("src").join("a.rs");
        let mut w = Worktrees::new(&repo);

        let named = worktree(&repo, &[".worktrees", "t1"], "t1");
        assert_eq!(w.reroot(&rel(&named)), rel(&repo));
        assert_eq!(w.reroot(&named), repo, "the root itself re-roots");

        let relative = repo.join("trees").join("t2");
        fs::create_dir_all(&relative).unwrap();
        fs::write(relative.join(".git"), "gitdir: ../../.git/worktrees/t2\n").unwrap();
        assert_eq!(w.reroot(&rel(&relative)), rel(&repo));

        let submodule = repo.join("vendor").join("lib");
        fs::create_dir_all(&submodule).unwrap();
        fs::write(submodule.join(".git"), "gitdir: ../../.git/modules/lib\n").unwrap();
        assert_eq!(w.reroot(&rel(&submodule)), rel(&submodule));

        let foreign = repo.join(".worktrees").join("t3");
        fs::create_dir_all(&foreign).unwrap();
        let other = t
            .path()
            .join("other")
            .join(".git")
            .join("worktrees")
            .join("t3");
        fs::write(
            foreign.join(".git"),
            format!("gitdir: {}\n", other.to_string_lossy()),
        )
        .unwrap();
        assert_eq!(w.reroot(&rel(&foreign)), rel(&foreign));

        // Claude Code's layout, present but not a worktree: the structure
        // says no, and the shape fallback does not overrule it.
        let plain = repo.join(".claude").join("worktrees").join("t4");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(w.reroot(&rel(&plain)), rel(&plain));

        let inner = worktree(
            &repo,
            &[".worktrees", "t1", ".claude", "worktrees", "t5"],
            "t5",
        );
        assert_eq!(w.reroot(&rel(&inner)), rel(&repo));

        // Outside the repository nothing is probed or re-rooted.
        let outside = t.path().join("elsewhere").join("a.rs");
        assert_eq!(w.reroot(&outside), outside);
    }

    /// A worktree deleted since the session ran cannot be probed. Claude
    /// Code's own layout re-roots by shape; any other layout is left as
    /// its absolute path rather than guessed. And a root, once probed,
    /// keeps its answer for the rest of the pass.
    #[test]
    fn a_deleted_worktree_falls_back_to_the_known_layout_only() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let mut w = Worktrees::new(repo);
        let gone = repo
            .join(".claude")
            .join("worktrees")
            .join("t6")
            .join("src")
            .join("a.rs");
        assert_eq!(w.reroot(&gone), repo.join("src").join("a.rs"));
        let unknown = repo.join(".worktrees").join("t7").join("src").join("a.rs");
        assert_eq!(w.reroot(&unknown), unknown);
        assert_eq!(path_key(&unknown, repo), ".worktrees/t7/src/a.rs");

        // Probed once per pass: deleting the worktree mid-pass does not
        // change this resolver's answer, and a new pass sees it gone.
        let wt = worktree(repo, &[".worktrees", "t8"], "t8");
        let file = wt.join("src").join("a.rs");
        assert_eq!(w.reroot(&file), repo.join("src").join("a.rs"));
        fs::remove_dir_all(&wt).unwrap();
        assert_eq!(w.reroot(&file), repo.join("src").join("a.rs"));
        assert_eq!(Worktrees::new(repo).reroot(&file), file);
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

    /// A session opening with a Glob and then reading `<root>/src/a.ts`,
    /// so the read is an early read whatever S4 asks of it.
    fn searched_then_read(root: &Path, n: usize) -> String {
        let cwd = root.to_string_lossy().into_owned();
        [
            tool_use(
                &cwd,
                &format!("g{n}"),
                "Glob",
                serde_json::json!({"pattern":"**/a.ts"}),
            ),
            tool_use(
                &cwd,
                &format!("r{n}"),
                "Read",
                serde_json::json!({"file_path":root.join("src").join("a.ts").to_string_lossy()}),
            ),
        ]
        .join("\n")
    }

    fn gone_paths(findings: &[Finding]) -> Vec<&Finding> {
        findings
            .iter()
            .filter(|f| f.finding.contains("no longer exist"))
            .collect()
    }

    /// #1335: a deleted checkout outside `.claude/worktrees` is re-rooted
    /// by the session's own cwd -- it no longer exists and the
    /// repository's git ignores it -- so three sessions in t1, t2 and t3
    /// fold onto `src/a.ts`, and no finding names `.wt`.
    #[test]
    fn a_deleted_ignored_checkout_re_roots_by_the_sessions_own_cwd() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        git_init(repo);
        write(repo, ".gitignore", ".wt/\n");
        fs::create_dir_all(repo.join("src")).unwrap();
        write(&repo.join("src"), "a.ts", "export {}\n");
        let conn = db();
        for n in [1, 2, 3] {
            let root = repo.join(".wt").join(format!("t{n}"));
            let p = write(repo, &format!("s{n}.jsonl"), &searched_then_read(&root, n));
            insert_session(
                &conn,
                &format!("s{n}"),
                &root.to_string_lossy(),
                Some(&p),
                None,
            );
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let hits = early_reads(&out);
        assert_eq!(hits.len(), 1, "{out:#?}");
        assert!(
            hits[0].finding.starts_with(&format!(
                "`src/a.ts` was read within the first {EARLY_CALLS} tool calls in 3 of 3 analysed sessions under `{}`",
                repo.display()
            )),
            "{}",
            hits[0].finding
        );
        assert!(
            out.iter().all(|f| !f.finding.contains(".wt")),
            "no finding names a deleted checkout: {out:#?}"
        );
        assert!(gone_paths(&out).is_empty(), "{out:#?}");
        // The census folds the three cwds onto the repository too.
        assert!(
            out.iter()
                .any(|f| f.finding.starts_with("3 sessions recorded under")),
            "{out:#?}"
        );
    }

    /// #1335: the same deleted directory, NOT ignored, is not known to
    /// have been a checkout. Its paths stay absolute, and a path-keyed
    /// finding naming a path that no longer exists is left out and
    /// counted, never advised.
    #[test]
    fn a_deleted_directory_git_does_not_ignore_stays_absolute_and_is_dropped() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        git_init(repo);
        fs::create_dir_all(repo.join("src")).unwrap();
        write(&repo.join("src"), "a.ts", "export {}\n");
        let root = repo.join(".wt").join("t1");
        let file = root.join("src").join("a.ts");
        let mut w = Worktrees::new(repo);
        w.learn_deleted_checkouts([root.as_path()]);
        assert_eq!(w.reroot(&file), file, "not ignored: not re-rooted");
        assert_eq!(w.unknown, None);

        let conn = db();
        for n in [1, 2, 3] {
            let p = write(repo, &format!("s{n}.jsonl"), &searched_then_read(&root, n));
            insert_session(
                &conn,
                &format!("s{n}"),
                &root.to_string_lossy(),
                Some(&p),
                None,
            );
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert!(early_reads(&out).is_empty(), "{out:#?}");
        let gone = gone_paths(&out);
        assert_eq!(gone.len(), 1, "{out:#?}");
        // A count, not advice (#1354).
        assert_eq!(gone[0].severity, Severity::Note, "{}", gone[0].finding);
        assert_eq!(
            gone[0].finding,
            format!(
                "1 finding named a path that no longer exists under `{}` and was left out",
                repo.display()
            )
        );
        assert_eq!(
            gone[0].evidence[0].at,
            Locator::File {
                path: file.to_string_lossy().into_owned(),
                line: None
            }
        );
        assert_eq!(
            gone[0].evidence[0].measured,
            "`Read` of a path that no longer exists, in 3 sessions"
        );
        // The Glob is not a path, and still stands.
        assert!(
            out.iter()
                .any(|f| f.finding.starts_with("`**/a.ts` was searched with Glob")),
            "{out:#?}"
        );
    }

    /// #1335: a repository whose git cannot answer leaves a deleted
    /// directory un-rooted and says why, as Unknown. It never guesses
    /// "not ignored".
    #[test]
    fn a_git_that_cannot_answer_is_unknown_not_a_guess() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        // A `.git` directory git does not accept as a repository.
        fs::create_dir_all(repo.join(".git")).unwrap();
        let root = repo.join(".wt").join("t1");
        let mut w = Worktrees::new(repo);
        w.learn_deleted_checkouts([root.as_path()]);
        let file = root.join("src").join("a.ts");
        assert_eq!(w.reroot(&file), file);
        let (n, why) = w.unknown.clone().expect("git's failure is recorded");
        assert_eq!(n, 1);
        assert!(why.contains("git check-ignore"), "{why}");

        let conn = db();
        let p = write(repo, "s1.jsonl", &searched_then_read(&root, 1));
        insert_session(&conn, "s1", &root.to_string_lossy(), Some(&p), None);
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let unknown: Vec<&Finding> = out
            .iter()
            .filter(|f| f.severity == Severity::Unknown)
            .collect();
        assert_eq!(unknown.len(), 1, "{out:#?}");
        assert!(
            unknown[0].finding.starts_with(
                "could not tell whether 1 deleted session directory was a checkout of this repository: "
            ),
            "{}",
            unknown[0].finding
        );

        // No `.git` at all is no ignore rules: nothing is ignored, which
        // is established, not Unknown.
        let bare = tempfile::tempdir().unwrap();
        let root = bare.path().join(".wt").join("t1");
        let mut w = Worktrees::new(bare.path());
        w.learn_deleted_checkouts([root.as_path()]);
        assert_eq!(w.unknown, None);
        assert_eq!(w.reroot(&root.join("a.ts")), root.join("a.ts"));
    }

    /// #1336: reading a file early is not evidence a pointer is missing
    /// unless the session had to look for it first. Three sessions that
    /// read `ci.yml` as their second call with no search before it give
    /// no finding; three that Glob and then read it give one, with the
    /// Glob in every evidence row. A failed Read counts as looking too.
    #[test]
    fn an_early_read_counts_only_after_a_search() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let workflows = repo.join(".github").join("workflows");
        fs::create_dir_all(&workflows).unwrap();
        let ci = write(&workflows, "ci.yml", "on: push\n");
        let cwd = repo.to_string_lossy().into_owned();
        let read = |n: usize| {
            tool_use(
                &cwd,
                &format!("r{n}"),
                "Read",
                serde_json::json!({"file_path":ci.to_string_lossy()}),
            )
        };
        let conn = db();
        for n in [1, 2, 3] {
            let body = [bash(&cwd, &format!("b{n}"), "git status"), read(n)].join("\n");
            let p = write(repo, &format!("s{n}.jsonl"), &body);
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert!(
            early_reads(&out).is_empty(),
            "no search came first: {out:#?}"
        );

        for n in [1, 2, 3] {
            let body = [
                tool_use(
                    &cwd,
                    &format!("g{n}"),
                    "Glob",
                    serde_json::json!({"pattern":"**/ci*.yml"}),
                ),
                read(n),
            ]
            .join("\n");
            rewrite(repo, &format!("s{n}.jsonl"), &body);
        }
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let hits = early_reads(&out);
        assert_eq!(hits.len(), 1, "{out:#?}");
        assert_eq!(hits[0].evidence.len(), 3);
        for e in &hits[0].evidence {
            assert_eq!(
                e.measured,
                "`Read` `.github/workflows/ci.yml`, after `Glob` `**/ci*.yml`"
            );
        }

        // A failed Read before it is looking, too.
        let failed = [
            tool_use(
                &cwd,
                "x",
                "Read",
                serde_json::json!({"file_path":repo.join("ci.yml").to_string_lossy()}),
            ),
            tool_result(&cwd, "x", Some(true), "File does not exist."),
            read(1),
        ]
        .join("\n");
        let rows = rows_of(&failed, &cwd, repo);
        let early: Vec<&Row> = rows
            .iter()
            .filter(|r| r.signal == SIG_SEARCH && r.key == ".github/workflows/ci.yml")
            .collect();
        assert_eq!(early.len(), 1, "{rows:?}");
        assert_eq!(
            early[0].detail.as_deref(),
            Some("a failed `Read` of `ci.yml`")
        );
        // An absent verdict is not a failure: no search came first.
        let silent = failed.replace(",\"is_error\":true", "");
        assert!(
            rows_of(&silent, &cwd, repo)
                .iter()
                .all(|r| r.key != ".github/workflows/ci.yml"),
            "{silent}"
        );
    }

    /// A session that opens with `prompt` and whose one Bash call fails
    /// with the same error every time.
    fn failing_task(cwd: &str, prompt: &str, n: usize) -> String {
        [
            user_text(cwd, prompt),
            bash(cwd, &format!("b{n}"), "curl localhost"),
            tool_result(
                cwd,
                &format!("b{n}"),
                Some(true),
                "curl: (7) Failed to connect",
            ),
        ]
        .join("\n")
    }

    fn errors(findings: &[Finding]) -> Vec<&Finding> {
        findings
            .iter()
            .filter(|f| f.finding.starts_with("recurring error: `Bash`"))
            .collect()
    }

    /// #1337: seven automated replays of one task are one task. Sessions
    /// sharing an opening prompt count once toward a threshold, so seven
    /// with the same prompt and the same error give no S5 finding (1 <
    /// 3), and three with different prompts give one.
    #[test]
    fn replays_of_one_task_count_once() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in 1..=7 {
            // Whitespace differs; the task does not.
            let prompt = if n % 2 == 0 {
                "check  the\nservice"
            } else {
                "check the service"
            };
            let p = write(repo, &format!("s{n}.jsonl"), &failing_task(&cwd, prompt, n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), Some(prompt));
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        assert!(errors(&out).is_empty(), "one task, seven runs: {out:#?}");
        let stored: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT task_fingerprint) FROM claude_advice_ledger",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, 1, "the fingerprint is on the ledger");

        // Two more tasks with the same error: three independent tasks.
        for (n, prompt) in [(8, "deploy the app"), (9, "why is the build red")] {
            let p = write(repo, &format!("s{n}.jsonl"), &failing_task(&cwd, prompt, n));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), Some(prompt));
        }
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let hits = errors(&out);
        assert_eq!(hits.len(), 1, "{out:#?}");
        assert!(
            hits[0].finding.contains(&format!(
                "in 3 of 3 analysed sessions under `{}` (9 runs; replays of one task counted once)",
                repo.display()
            )),
            "{}",
            hits[0].finding
        );
        // One evidence row per task, the replayed one saying so.
        assert_eq!(hits[0].evidence.len(), 3, "{:#?}", hits[0].evidence);
        assert!(
            hits[0].evidence[0]
                .measured
                .ends_with("; 7 runs of one task"),
            "{}",
            hits[0].evidence[0].measured
        );
        assert!(!hits[0].evidence[1].measured.contains("runs of one task"));
    }

    /// #1337: a session with no opening prompt is its own task, never
    /// folded with another that has none. Absent is not a shared value.
    #[test]
    fn sessions_without_an_opening_prompt_are_each_their_own_task() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        for n in 1..=3 {
            let body = [
                bash(&cwd, &format!("b{n}"), "curl localhost"),
                tool_result(
                    &cwd,
                    &format!("b{n}"),
                    Some(true),
                    "curl: (7) Failed to connect",
                ),
            ]
            .join("\n");
            let p = write(repo, &format!("s{n}.jsonl"), &body);
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let hits = errors(&out);
        assert_eq!(hits.len(), 1, "{out:#?}");
        assert!(!hits[0].finding.contains("runs"), "{}", hits[0].finding);
        let nulls: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM claude_advice_ledger WHERE task_fingerprint IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 3);
        assert_eq!(task_fingerprint("a  b\n c"), task_fingerprint("a b c"));
        assert_ne!(task_fingerprint("a b"), task_fingerprint("a c"));
    }

    /// #1338: an error cluster keeps the failing call's key. A Read's path
    /// is in the call's input, not the error text, so without it the
    /// finding could not say which file. Every evidence row names the
    /// call, and when every session failed on the same one, so does the
    /// sentence.
    ///
    /// #1368: and it is an observation. Three sessions failing the same
    /// way with no correction is a Note, with no suggestion; the same
    /// failure followed by a call that worked is S1, which stays Advice.
    #[test]
    fn an_error_cluster_names_the_failing_call() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let cwd = repo.to_string_lossy().into_owned();
        let failing_read = |n: usize, file: &str| {
            [
                tool_use(
                    &cwd,
                    &format!("r{n}"),
                    "Read",
                    serde_json::json!({"file_path":repo.join("src").join(file).to_string_lossy()}),
                ),
                tool_result(&cwd, &format!("r{n}"), Some(true), "File does not exist."),
            ]
            .join("\n")
        };
        let conn = db();
        for n in [1, 2, 3] {
            let p = write(repo, &format!("s{n}.jsonl"), &failing_read(n, "gone.ts"));
            insert_session(&conn, &format!("s{n}"), &cwd, Some(&p), None);
        }
        let scan = scan_effective_opt(repo, None);
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let hits: Vec<&Finding> = out
            .iter()
            .filter(|f| f.finding.starts_with("recurring error: `Read`"))
            .collect();
        assert_eq!(hits.len(), 1, "{out:#?}");
        assert_eq!(
            hits[0].finding,
            format!(
                "recurring error: `Read` on `src/gone.ts`: `File does not exist.`, in 3 of 3 analysed sessions under `{}`",
                repo.display()
            )
        );
        assert_eq!(hits[0].severity, Severity::Note, "{}", hits[0].finding);
        assert!(
            !hits[0].brief.contains("Suggested change"),
            "{}",
            hits[0].brief
        );
        assert_eq!(
            hits[0].evidence[0].measured,
            "`Read` `src/gone.ts`: File does not exist."
        );
        for e in &hits[0].evidence[1..] {
            assert_eq!(
                e.measured,
                "`Read` `src/gone.ts`: the same error, after normalisation"
            );
        }

        // Different files: the sentence cannot name one, the rows still do.
        rewrite(repo, "s3.jsonl", &failing_read(3, "other.ts"));
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let hit = out
            .iter()
            .find(|f| f.finding.starts_with("recurring error: `Read`"))
            .expect("the error cluster");
        assert!(
            hit.finding.starts_with(
                "recurring error: `Read`: `File does not exist.`, in 3 of 3 analysed sessions"
            ),
            "{}",
            hit.finding
        );
        assert!(
            hit.evidence[2]
                .measured
                .starts_with("`Read` `src/other.ts`: "),
            "{}",
            hit.evidence[2].measured
        );

        // The control: the same kind of failure corrected by a call that
        // worked is S1, and S1 is Advice. Its error is still observed.
        for n in [1, 2, 3] {
            rewrite(repo, &format!("s{n}.jsonl"), &corrected_pair(&cwd, n));
        }
        let out = analyse(&conn, &context(repo, &scan, &conn), SESSIONS_PER_PASS).unwrap();
        let s1 = corrected(&out);
        assert_eq!(s1.len(), 1, "{out:#?}");
        assert_eq!(s1[0].severity, Severity::Advice, "{}", s1[0].finding);
        let s5 = errors(&out);
        assert_eq!(s5.len(), 1, "{out:#?}");
        assert_eq!(s5[0].severity, Severity::Note, "{}", s5[0].finding);
    }

    /// Rows stored under an older extraction rule are not served: the
    /// session is re-read even though its transcript has not moved, and
    /// its rows are replaced.
    #[test]
    fn a_ledger_row_from_an_older_rule_is_re_read() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        let cwd = repo.to_string_lossy().into_owned();
        let conn = db();
        let s1 = write(repo, "s1.jsonl", &corrected_pair(&cwd, 1));
        insert_session(&conn, "s1", &cwd, Some(&s1), None);
        let scan = scan_effective_opt(repo, None);
        let cx = context(repo, &scan, &conn);
        analyse(&conn, &cx, SESSIONS_PER_PASS).unwrap();

        // What an older build left behind: the same (size, mtime), an
        // old key, and no rule version.
        conn.execute_batch(
            "UPDATE claude_advice_ledger SET rule_version = 0, analysed_at = 'old';
             UPDATE claude_advice_signal SET key = 'stale';",
        )
        .unwrap();
        analyse(&conn, &cx, SESSIONS_PER_PASS).unwrap();
        let (version, at): (i64, String) = conn
            .query_row(
                "SELECT rule_version, analysed_at FROM claude_advice_ledger WHERE session_id = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(version, RULE_VERSION);
        assert_ne!(at, "old", "the session was not re-read");
        let stale: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM claude_advice_signal WHERE key = 'stale'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stale, 0, "the old rule's rows were replaced");
    }

    /// Measured over this machine's real corpus: wall time cold and warm,
    /// truncation, and what each signal found. `#[ignore]`d because it
    /// depends on the developer's own `~/.claude/projects`; the PR body
    /// records its output.
    #[test]
    #[ignore = "needs the developer's own ~/.claude/projects"]
    fn real_corpus() {
        let repo = std::env::var("HEADSTATE_ADVICE_REPO")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .to_path_buf()
            });
        let scan = match crate::claude::scan_default() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}; nothing to measure");
                return;
            }
        };
        let mut conn = db();
        crate::claude::store::import(&mut conn, scan).unwrap();
        let home = crate::claudemd::home();
        let effective = scan_effective_opt(&repo, home.as_deref());
        let cx = context(&repo, &effective, &conn);

        let started = std::time::Instant::now();
        let cold = analyse(&conn, &cx, SESSIONS_PER_PASS).unwrap();
        let cold_ms = started.elapsed().as_millis();
        let started = std::time::Instant::now();
        let warm = analyse(&conn, &cx, SESSIONS_PER_PASS).unwrap();
        let warm_ms = started.elapsed().as_millis();
        let ledger: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_advice_ledger", [], |r| {
                r.get(0)
            })
            .unwrap();
        let truncated: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM claude_advice_ledger WHERE truncated = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut per_signal: BTreeMap<String, i64> = BTreeMap::new();
        let mut q = conn
            .prepare("SELECT signal, COUNT(*) FROM claude_advice_signal GROUP BY signal")
            .unwrap();
        for row in q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .unwrap()
        {
            let (s, n) = row.unwrap();
            per_signal.insert(s, n);
        }
        println!("repo                 {}", repo.display());
        println!("sessions in ledger   {ledger}");
        println!("truncated at 8 MB    {truncated}");
        println!("cold                 {cold_ms} ms");
        println!("warm                 {warm_ms} ms");
        println!("raw rows per signal  {per_signal:?}");
        // The keys behind the threshold-bearing signals, so a reader of
        // the PR body can judge whether the thresholds are noise.
        let mut q = conn
            .prepare(
                "SELECT signal, key, aux FROM claude_advice_signal
                  WHERE signal IN ('corrected', 'user_correction', 'denied')
                  ORDER BY signal, key",
            )
            .unwrap();
        for row in q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
        {
            let (signal, key, aux) = row.unwrap();
            println!("  {signal:<16} key `{key}` aux {aux:?}");
        }
        println!("findings (cold)      {}", cold.len());
        for f in &cold {
            println!("  [{:?}] {}", f.severity, f.finding);
        }
        assert_eq!(
            cold.len(),
            warm.len(),
            "a warm pass must answer the same as a cold one"
        );
    }
}
