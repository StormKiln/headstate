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
//! | S4 repeated search | identical Grep/Glob `pattern`, or a `Read` within the first [`EARLY_CALLS`] tool calls | ≥ [`MIN_SESSIONS_SEARCH`] sessions |
//! | S5 repeated error | `is_error: true` text normalised by [`normalise_error`] | identical first [`ERROR_KEY_CHARS`] chars in ≥ [`MIN_SESSIONS_ERROR`] sessions |
//! | S6 task census | sessions per attributed directory, from `claude_session` | always, as a count |
//!
//! A count is DISTINCT SESSIONS, never records. `is_error` absent is not
//! `false`: a result that did not say is neither a failure (for A) nor a
//! success (for B), and a test below pins both directions. The
//! thresholds are unvalidated on a real corpus -- this machine holds one
//! session -- and are constants so a corpus can move them without a
//! migration.
//!
//! # Attribution
//!
//! A tool call's absolute `file_path` places the finding on the deepest
//! CLAUDE.md-bearing ancestor directory from the repository scan, else
//! the repository root; a Bash command has no path and attributes to the
//! record's `cwd`. An agent-worktree path `<repo>/.claude/worktrees/
//! agent-<id>/…` re-roots to `<repo>/…`, the shape
//! `subagent::Kind::classify` matches. The stored row carries the
//! attributed DIRECTORY, and which CLAUDE.md it maps to is decided at
//! read time against the current scan, so a CLAUDE.md added since the
//! pass moves the finding without a re-read.
//!
//! # Already written is not a gap
//!
//! Before a finding is emitted its dedup key is tested verbatim, with
//! whitespace collapsed and case kept, against every CLAUDE.md from the
//! repository root to the attributed directory, their resolved imports,
//! and the global and local scopes the effective scan carries. A hit is
//! kept visible as a finding worded "already written in `<file>`", so the
//! reader sees the rule doing its job. A paraphrased rule is missed by
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
//! re-reads only what changed. This runs on demand behind the panel, on
//! the command's `spawn_blocking`, and never on the live pass (#1246).
//!
//! # Coverage travels as findings
//!
//! The report model has no per-check coverage struct beyond
//! [`super::CheckRun`], so what the spec calls `Coverage` travels here as
//! findings: one [`Severity::Unknown`] per transcript that could not be
//! read (`<path>: <why>`), one [`Severity::Advice`] stating "analysed N
//! of M sessions under `<repo>`; K truncated at 8 MB" whenever the pass
//! is short or a read was cut, and one stating "no Claude Code sessions
//! were recorded under `<repo>`" when there are none -- never an empty
//! list, because no sessions is not "nothing went wrong". While the pass
//! is short, every count says "at least".
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
use crate::claude::subagent::Kind;
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
    /// The recorded cwd, re-rooted out of an agent worktree.
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
    /// User or error text, clamped to [`DETAIL_CHARS`].
    detail: Option<String>,
    /// S3: the denied call's `tool_use.id`, so a hook row for the same
    /// call is not counted twice.
    tool_use_id: Option<String>,
}

/// Run the pass over the sessions under `cx.repo`, reading at most
/// `cap` transcripts, and assemble the findings.
fn analyse(conn: &Connection, cx: &Context, cap: usize) -> Result<Vec<Finding>, String> {
    let repo = cx.repo;
    let sessions = sessions_under(conn, repo)?;
    let root_subject = subject_for(repo, cx.scan);

    if sessions.is_empty() {
        return Ok(vec![Finding::new(
            Check::Transcripts,
            Severity::Advice,
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
    let mut known: HashMap<String, (i64, i64, bool)> = HashMap::new();
    {
        let mut q = conn
            .prepare("SELECT session_id, size_bytes, mtime_ms, truncated FROM claude_advice_ledger")
            .map_err(|e| format!("claude_advice_ledger: {e}"))?;
        let rows = q
            .query_map([], |r| {
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

    let mut unreadable: Vec<(String, String)> = Vec::new();
    let mut analysed: HashSet<String> = HashSet::new();
    let mut truncated = 0usize;
    let mut todo: Vec<(&SessionRow, i64, i64)> = Vec::new();
    for s in &sessions {
        let Some(path) = &s.transcript_path else {
            unreadable.push((
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
            Err(e) => {
                unreadable.push((s.session_id.clone(), e));
                continue;
            }
        };
        let rows = extract(&s.session_id, &s.dir, &body, repo);
        if let Err(e) = store_rows(conn, &s.session_id, &rows, size, mtime, cut, &now) {
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
    let denials = hook_denials(conn, &sessions)?;

    let short = analysed.len() < sessions.len() || !unreadable.is_empty();
    let mut out = emit(&stored, &denials, &sessions, &analysed, cx, short);

    if short || truncated > 0 {
        let mut sentence = format!(
            "analysed {} of {} sessions under `{}`; {} truncated at {} MB",
            analysed.len(),
            sessions.len(),
            repo.display(),
            truncated,
            BUDGET_BYTES / (1024 * 1024)
        );
        if remaining > 0 {
            sentence.push_str(&format!(
                "; {remaining} not yet read (at most {cap} are read per open)"
            ));
        }
        out.push(Finding::new(
            Check::Transcripts,
            Severity::Advice,
            root_subject.clone(),
            vec![Evidence {
                at: Locator::File {
                    path: repo.to_string_lossy().into_owned(),
                    line: None,
                },
                measured: format!(
                    "{} sessions analysed, {} unreadable, {} not yet read, {} read only to {} MB",
                    analysed.len(),
                    unreadable.len(),
                    remaining,
                    truncated,
                    BUDGET_BYTES / (1024 * 1024)
                ),
            }],
            sentence,
        ));
    }

    for (session_id, why) in unreadable {
        out.push(Finding::new(
            Check::Transcripts,
            Severity::Unknown,
            root_subject.clone(),
            vec![Evidence {
                at: Locator::Session {
                    session_id: session_id.clone(),
                    record: None,
                },
                measured: why.clone(),
            }],
            format!("session `{session_id}` could not be read: {why}"),
        ));
    }
    Ok(out)
}

/// Directories under `repo` with at least [`MIN_EDITS_PER_DIR`] Edit or
/// Write calls across the sessions the pass has read, for the gaps
/// producer: a directory sessions keep editing is one whose missing
/// CLAUDE.md matters more than an untouched one's.
///
/// Reads only what a pass already stored. A repository no pass has run
/// over yields an empty list, which is "no edits recorded", not "no
/// edits".
pub fn edited_dirs(conn: &Connection, repo: &Path) -> Result<Vec<PathBuf>, String> {
    let mut q = conn
        .prepare(
            "SELECT dir, COUNT(*) FROM claude_advice_signal
              WHERE signal = ?1
              GROUP BY dir HAVING COUNT(*) >= ?2
              ORDER BY dir",
        )
        .map_err(|e| format!("claude_advice_signal: {e}"))?;
    let rows = q
        .query_map(rusqlite::params![SIG_EDIT, MIN_EDITS_PER_DIR as i64], |r| {
            r.get::<_, String>(0)
        })
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

/// The sessions whose recorded cwd is under `repo`, agent worktrees
/// re-rooted first so a session run in `<repo>/.claude/worktrees/
/// agent-x` counts as the repository's own.
fn sessions_under(conn: &Connection, repo: &Path) -> Result<Vec<SessionRow>, String> {
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
    let mut out = Vec::new();
    for row in rows {
        let (session_id, cwd, transcript_path, prompt) =
            row.map_err(|e| format!("claude_session: {e}"))?;
        let dir = reroot_cwd(&cwd);
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

/// A session's cwd, re-rooted when it is an agent worktree root: the
/// last three components `.claude/worktrees/agent-<id>` are stripped, the
/// exact shape `Kind::classify` matches and nothing looser.
fn reroot_cwd(cwd: &str) -> PathBuf {
    let p = Path::new(cwd);
    if Kind::classify(Some(cwd)).is_subagent() {
        if let Some(root) = p.parent().and_then(Path::parent).and_then(Path::parent) {
            return root.to_path_buf();
        }
    }
    p.to_path_buf()
}

/// A tool call's path, re-rooted when it lies under an agent worktree
/// anywhere in its ancestry: `<repo>/.claude/worktrees/agent-x/src/a.rs`
/// is `<repo>/src/a.rs` for attribution.
fn reroot_path(path: &Path) -> PathBuf {
    let comps: Vec<std::path::Component> = path.components().collect();
    let names: Vec<String> = comps
        .iter()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    for i in 0..names.len().saturating_sub(2) {
        if names[i] == ".claude"
            && names[i + 1] == "worktrees"
            && names[i + 2]
                .strip_prefix("agent-")
                .is_some_and(|id| !id.is_empty())
        {
            let mut out = PathBuf::new();
            for (j, c) in comps.iter().enumerate() {
                if j < i || j > i + 2 {
                    out.push(c);
                }
            }
            return out;
        }
    }
    path.to_path_buf()
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

/// Extract every signal occurrence from one transcript body.
fn extract(session_id: &str, session_dir: &Path, body: &str, repo: &Path) -> Vec<Row> {
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
            .map(reroot_cwd)
            .unwrap_or_else(|| session_dir.to_path_buf());
        let Some(message) = rec.get("message") else {
            continue;
        };
        match kind {
            "assistant" => {
                for b in blocks_of(message.get("content")) {
                    if let Block::ToolUse { name, id, args } = b {
                        calls.push(call_of(record, name, id, args, &dir, repo));
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
            rows.push(r);
        }
    }

    // S4 -- repeated search, and the edit census for `edited_dirs`.
    for (i, c) in calls.iter().enumerate() {
        match c.name.as_str() {
            "Grep" | "Glob" => rows.push(row(SIG_SEARCH, c, c.key.clone())),
            "Read" if i < EARLY_CALLS => rows.push(row(SIG_SEARCH, c, c.key.clone())),
            "Edit" | "Write" | "MultiEdit" => rows.push(row(SIG_EDIT, c, c.key.clone())),
            _ => {}
        }
    }
    rows
}

/// A `tool_use` block as a [`Call`], with its key and attributed directory.
fn call_of(
    record: u64,
    name: String,
    id: Option<String>,
    args: ToolArgs,
    cwd: &Path,
    repo: &Path,
) -> Call {
    let file_key = |file_path: &str| -> (String, PathBuf) {
        let p = reroot_path(Path::new(file_path));
        let dir = p
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| cwd.to_path_buf());
        // A path outside the repository attributes to the record's cwd:
        // there is no CLAUDE.md under the repository for it.
        let dir = if dir.starts_with(repo) {
            dir
        } else {
            cwd.to_path_buf()
        };
        (p.to_string_lossy().into_owned(), dir)
    };
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
            let (k, d) = file_key(file_path);
            (k, d, Vec::new())
        }
        ToolArgs::Grep { pattern, path, .. } | ToolArgs::Glob { pattern, path } => {
            let dir = path
                .as_deref()
                .map(|p| reroot_path(Path::new(p)))
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

/// Replace a session's stored rows and its ledger entry, in one
/// transaction so a reader never sees a session with a ledger entry and
/// half its rows.
fn store_rows(
    conn: &Connection,
    session_id: &str,
    rows: &[Row],
    size: i64,
    mtime: i64,
    truncated: bool,
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
                (session_id, signal, dir, key, aux, record_index, record_index_2, detail, tool_use_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
            ],
        )?;
    }
    tx.execute(
        "INSERT INTO claude_advice_ledger (session_id, size_bytes, mtime_ms, truncated, analysed_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(session_id) DO UPDATE SET
            size_bytes = ?2, mtime_ms = ?3, truncated = ?4, analysed_at = ?5",
        rusqlite::params![session_id, size, mtime, truncated as i64, now],
    )?;
    tx.commit()
}

/// Every stored row for the analysed sessions.
fn load_rows(conn: &Connection, analysed: &HashSet<String>) -> Result<Vec<Row>, String> {
    let mut q = conn
        .prepare(
            "SELECT session_id, signal, dir, key, aux, record_index, record_index_2, detail, tool_use_id
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
            ))
        })
        .map_err(|e| format!("claude_advice_signal: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let (session_id, signal, dir, key, aux, record, record_2, detail, tool_use_id) =
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

/// Which CLAUDE.md on the path from the root to `dir`, its imports, or
/// the global and local scopes already holds `key` verbatim.
fn written_in(
    key: &str,
    dir: &Path,
    cx: &Context,
    cache: &mut HashMap<String, Option<String>>,
) -> Option<String> {
    let needle = collapse(key);
    if needle.is_empty() {
        return None;
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
    for path in queue {
        let content = cache
            .entry(path.clone())
            .or_insert_with(|| std::fs::read_to_string(&path).ok().map(|c| collapse(&c)));
        if content.as_deref().is_some_and(|c| c.contains(&needle)) {
            return Some(path);
        }
    }
    None
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

/// Group the stored rows, apply the thresholds, place and dedup each
/// group, and render the findings.
fn emit(
    stored: &[Row],
    denials: &[(String, String, Option<String>)],
    sessions: &[SessionRow],
    analysed: &HashSet<String>,
    cx: &Context,
    short: bool,
) -> Vec<Finding> {
    let candidates = claude_dirs(cx);
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut out = Vec::new();
    let at_least = if short { "at least " } else { "" };
    let plural = |n: usize| if n == 1 { "" } else { "s" };

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
            });
    }

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
            let n = by_session.len();
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
            let dirs: Vec<&Path> = by_session.values().map(|r| r.dir.as_path()).collect();
            let dir = placement(&dirs, &candidates, cx.repo);
            let subject = subject_for(&dir, cx.scan);
            let sessions_phrase = format!(
                "{at_least}{n} session{} under `{}`",
                plural(n),
                dir.display()
            );
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
                _ => (
                    format!("the same `{aux}` error was recorded in {sessions_phrase}: `{key}`"),
                    key.clone(),
                ),
            };
            let sentence = match written_in(&dedup_key, &dir, cx, &mut cache) {
                Some(file) => format!("`{dedup_key}` is already written in `{file}` ({sentence})"),
                None => sentence,
            };

            let mut evidence = Vec::new();
            for (i, r) in by_session.values().take(MAX_EVIDENCE).enumerate() {
                let first = i == 0;
                let measured = match signal {
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
                    SIG_SEARCH => format!("`{aux}` `{key}`"),
                    _ => match (first, &r.detail) {
                        (true, Some(d)) => format!("error text: {d}"),
                        _ => "the same error, after normalisation".to_string(),
                    },
                };
                evidence.push(Evidence {
                    at: Locator::Session {
                        session_id: r.session_id.clone(),
                        record: r.record,
                    },
                    measured,
                });
            }
            out.push(Finding::new(
                Check::Transcripts,
                Severity::Advice,
                subject,
                evidence,
                sentence,
            ));
        }
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
            Severity::Advice,
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

    /// The founding pair: `yarn lint` fails, `make lint` succeeds.
    fn corrected_pair(cwd: &str, n: usize) -> String {
        [
            user_text(cwd, "run the linter"),
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
                "`yarn lint` failed and `make lint` followed it in 2 sessions under `{}`",
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
        // The census is always there, as a count.
        assert!(
            two.iter()
                .any(|f| f.finding.starts_with("2 sessions recorded under")),
            "{two:#?}"
        );
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
            hits[0].finding.contains("in 3 sessions under"),
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

        assert_eq!(reroot_cwd(&wt_s), repo);
        assert_eq!(reroot_cwd(&cwd), repo);
        // A bare `agent-` is not a worktree, exactly as `Kind::classify`
        // rules, and a file path under one re-roots the same way.
        let bare = repo.join(".claude").join("worktrees").join("agent-");
        assert_eq!(reroot_cwd(&bare.to_string_lossy()), bare);
        assert_eq!(
            reroot_path(&wt.join("src").join("a.rs")),
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
        assert_eq!(hits[0].severity, Severity::Advice);
        assert!(
            hits[0].brief.contains("change nothing"),
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
            unknown[0]
                .finding
                .starts_with("session `s4` could not be read: "),
            "{}",
            unknown[0].finding
        );
        assert!(
            unknown[0].finding.contains("s4.jsonl"),
            "{}",
            unknown[0].finding
        );
        assert!(
            unknown[0].finding.contains("Permission denied"),
            "{}",
            unknown[0].finding
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
            hits[0].finding.contains("in at least 2 sessions"),
            "{}",
            hits[0].finding
        );
        let coverage = out
            .iter()
            .find(|f| f.finding.starts_with("analysed 2 of 3 sessions under"))
            .expect("the coverage finding");
        assert!(
            coverage.finding.ends_with("; 0 truncated at 8 MB"),
            "{}",
            coverage.finding
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
        assert_eq!(out[0].severity, Severity::Advice);
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
        extract("s", Path::new(cwd), body, repo)
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
            head_keyed.finding.contains("at least 3 sessions"),
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
        // s4 has no transcript: Unknown, with the reason.
        assert!(
            out.iter().any(|f| f.severity == Severity::Unknown
                && f.finding.contains("s4")
                && f.finding.contains("no transcript path")),
            "{out:#?}"
        );
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
            search.iter().all(|r| r.key != format!("{cwd}/README.md")),
            "the late read is not orientation"
        );
        assert_eq!(search[1].dir, repo.join("src"));
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
        for n in [1, 2, 3] {
            let body = [tool_use(
                &cwd,
                &format!("r{n}"),
                "Read",
                serde_json::json!({"file_path":file.to_string_lossy()}),
            )]
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
        assert!(
            read.finding
                .contains(&format!("under `{}`", repo.join("src-tauri").display())),
            "{}",
            read.finding
        );

        // A second group of the same key from the root's cwd sits above
        // both: the placement is the common ancestor.
        let dirs = [repo.join("src-tauri").join("src"), repo.join("src")];
        let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
        assert_eq!(
            placement(&refs, &claude_dirs(&context(repo, &scan, &conn)), repo),
            repo
        );
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
