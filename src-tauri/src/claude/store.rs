//! Writing imported transcripts into `claude_session` (#914).
//!
//! Migration 11 (`store/schema.rs`) created the tables; this is the first
//! thing to write to them.
//!
//! # Why an upsert, and why on `session_id`
//!
//! `session_id` is the primary key because it is the one stable identity
//! a session has: a `claude --resume` or `--continue` reuses the SAME id
//! with a new pid, and only a fresh start mints a new one (measured, epic
//! #910 §1.2). It is also sound as a cross-source key -- across the real
//! corpus the transcript filename equals the body's `sessionId` for all
//! 1,430 files, and no id appears in two project directories.
//!
//! So a full rescan is idempotent by construction. Running it twice
//! produces the same rows, which is what lets #914 skip every piece of
//! incremental machinery: there is no offset to keep, and re-importing
//! costs one UPSERT per session.
//!
//! # The merge rule: transcript wins on `cwd`
//!
//! The two sources know different things, and the ONE field they disagree
//! about has a known-correct answer:
//!
//! | Field | Authority | Why |
//! |---|---|---|
//! | `cwd`, `git_branch`, `claude_version`, `transcript_path` | **transcript** | ground truth on disk |
//! | `name` | first non-empty, user's rename last | see below |
//! | `first_seen_at` | earliest of the two | |
//! | `last_activity_at` | latest of the two | |
//! | pid, `source`, `end_reason`, `ended_at` | hook only (`claude_run`) | the transcript has none of them |
//!
//! The transcript winning on `cwd` is not a tie-break preference, it is a
//! bug fix: a `SessionStart` hook can receive a STALE `session_id` and
//! `transcript_path` from the previous session after `/exit` then
//! `--continue` (Claude Code upstream issue 9188). A hook-recorded `cwd` can
//! therefore be the wrong directory, and re-reading disk corrects it.
//! Preferring the stored value would preserve the lie indefinitely,
//! because nothing else ever revisits it.
//!
//! # What an import must NOT do
//!
//! It must not write `claude_run`. A transcript proves a session existed;
//! it cannot prove a pid or how a run ended, and migration 11 declares
//! `claude_run.pid` `NOT NULL` precisely so that an unobserved process
//! cannot be recorded as an observed one. A transcript-only session
//! therefore has zero runs, which the liveness layer reports as unknown
//! rather than dead -- honest, because we never watched that process.

mod graph;

use rusqlite::{Connection, OptionalExtension};

use super::subagent::{Kind, Parent};
use super::transcript::{PopulationProgress, PopulationStatus, Scan, Transcript};

/// What an import changed, and what it could not read.
///
/// The unreadable counts are carried through from the [`Scan`] rather
/// than dropped at the storage boundary, because the caller that renders
/// the list is the one that has to say "this may be incomplete". A count
/// that only reaches a log line is a count nobody sees.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Imported {
    #[serde(default)]
    pub population: super::transcript::PopulationProgress,
    /// Sessions written (inserted or updated).
    pub sessions: usize,
    /// Rows the database refused, with why. Counted, not swallowed: a
    /// scan that read 1,430 transcripts and stored 1,200 must say so.
    pub write_failures: Vec<String>,
    pub subagent_files_skipped: usize,
    /// Bytes across the session transcripts (#1135).
    pub session_bytes: u64,
    /// Bytes across the subagent transcripts, kept apart because the two
    /// mean different things.
    pub subagent_bytes: u64,
    /// Files whose size could not be read, making both totals floors.
    pub unsized_files: usize,
    pub unreadable_dirs: Vec<String>,
    pub unreadable_files: Vec<String>,
    pub metadata_beyond_first_record: usize,
    pub elapsed_ms: u64,
    /// Sessions that ran inside an agent worktree (#1002).
    ///
    /// Hidden from the list by default. Counted so the exclusion can STATE
    /// its size -- #975's rule, which `subagent_files_skipped` above
    /// already follows for the other subagent shape: a hidden exclusion
    /// that does not say how many it hid leaves a user counting rows in
    /// disagreement with the app and no way to find out why.
    pub subagents: usize,
    /// How many of those were traced to the session that spawned them.
    ///
    /// The DIFFERENCE from `subagents` is the point: it is the number of
    /// subagent sessions whose parent could not be told, and the UI says
    /// so rather than rendering them as belonging to nobody.
    pub subagents_attributed: usize,
    /// Transcripts that could not be read while building the parent map.
    ///
    /// Carried separately from `unreadable_files` because they are a
    /// different failure with a different consequence: an unreadable
    /// transcript here does not cost a SESSION, it costs an attribution --
    /// the file that could not be read may be the one that would have
    /// named a parent.
    pub subagent_map_unreadable: Vec<String>,
    /// The transcript root, when it does not exist at all (#970).
    ///
    /// Carried through from [`Scan::absent_root`] and kept OUT of
    /// [`Imported::is_partial`] for the reason that field's own doc gives:
    /// a machine that has never run Claude Code has a complete list of
    /// nothing, and the page's first sentence to its owner must not be
    /// that the list is incomplete by an unknown amount.
    pub absent_root: Option<String>,
}

impl Imported {
    /// Whether anything could not be read or written.
    ///
    /// `absent_root` is deliberately not consulted -- see its doc (#970).
    ///
    /// Neither is `subagent_map_unreadable` (#1002), and for the same
    /// kind of reason. This flag drives "this LIST may be incomplete". A
    /// transcript that could not be read while building the parent map
    /// costs an ATTRIBUTION, not a session: every row is still present and
    /// the list is still the whole list. The affected children say "could
    /// not tell" on their own rows, which is where that uncertainty
    /// belongs. Raising the list-level banner for it would tell the user
    /// their session list is short when it is complete.
    pub fn is_partial(&self) -> bool {
        self.population.status != PopulationStatus::Complete
            || !self.unreadable_dirs.is_empty()
            || !self.unreadable_files.is_empty()
            || !self.write_failures.is_empty()
    }
}

/// Upsert one transcript-sourced session.
///
/// `first_seen_at` is `NOT NULL` in migration 11, so a transcript with no
/// timestamp anywhere in its head needs one. The transcript path is not a
/// source of time -- a file's mtime says when it was last written, not
/// when the session began -- so the fallback is the last activity if
/// known, and the id's own row is left with the import time otherwise.
/// That is a recorded fact about OUR observation, not a claim about the
/// session, and `last_activity_at` stays NULL so nothing renders it as
/// activity.
fn upsert(conn: &Connection, t: &Transcript, now: &str) -> Result<(), rusqlite::Error> {
    let first_seen = t
        .first_seen_at
        .as_deref()
        .or(t.last_activity_at.as_deref())
        .unwrap_or(now);

    conn.execute(
        // COALESCE order encodes the merge rule above, and the direction
        // differs per field on purpose:
        //
        //   cwd / git_branch / claude_version / transcript_path
        //       -- the NEW value first: disk is ground truth and must
        //          overwrite a stale hook payload (upstream 9188).
        //   name
        //       -- the EXISTING value first: a user's rename, and any
        //          name we already have, survives a rescan. An import
        //          never overwrites a name.
        //   first_seen_at / last_activity_at
        //       -- MIN and MAX, so the row widens to cover both sources
        //          and a rescan can never narrow a known range.
        //
        // `excluded.cwd` rather than `?` in the update half so the rule
        // reads off the row being merged, not off argument order.
        "INSERT INTO claude_session
            (session_id, name, cwd, git_branch, claude_version,
             transcript_path, first_seen_at, last_activity_at, opening_prompt)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(session_id) DO UPDATE SET
            cwd              = COALESCE(excluded.cwd, claude_session.cwd),
            git_branch       = COALESCE(excluded.git_branch, claude_session.git_branch),
            claude_version   = COALESCE(excluded.claude_version, claude_session.claude_version),
            transcript_path  = COALESCE(excluded.transcript_path, claude_session.transcript_path),
            name             = COALESCE(claude_session.name, excluded.name),
            -- NEW value first, like `cwd` and unlike `name`: this is read
            -- off disk every scan and the transcript is ground truth. A
            -- name survives a rescan because a user may have renamed it;
            -- nobody renames the prompt they typed (#1133).
            opening_prompt   = COALESCE(excluded.opening_prompt, claude_session.opening_prompt),
            first_seen_at    = MIN(claude_session.first_seen_at, excluded.first_seen_at),
            last_activity_at = MAX(
                COALESCE(claude_session.last_activity_at, excluded.last_activity_at),
                COALESCE(excluded.last_activity_at, claude_session.last_activity_at)
            )",
        rusqlite::params![
            t.session_id,
            t.name,
            t.cwd,
            t.git_branch,
            t.claude_version,
            t.path,
            first_seen,
            t.last_activity_at,
            t.opening_prompt,
        ],
    )?;
    Ok(())
}

/// The pull requests one session produced (#1132).
pub fn prs_for_session(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<super::subagent::PrLink>, rusqlite::Error> {
    let mut q = conn.prepare(
        "SELECT session_id, repo, number, url, first_seen_at
           FROM claude_session_pr WHERE session_id = ?1
          ORDER BY first_seen_at, repo, number",
    )?;
    let rows = q.query_map([session_id], |r| {
        Ok(super::subagent::PrLink {
            session_id: r.get(0)?,
            repo: r.get(1)?,
            number: r.get::<_, i64>(2)? as u64,
            url: r.get(3)?,
            first_seen_at: r.get(4)?,
        })
    })?;
    rows.collect()
}

/// The sessions that produced a pull request with this NUMBER, in any
/// repository (#1545).
///
/// What the search box asks when the query is a bare `#1234` or `1234`.
/// It used to resolve the repository from the tracked OPEN pull requests
/// first, so a merged one -- the usual state of "the PR that session
/// made" by the time anyone searches for it -- named no repository, and
/// nothing was looked up. Measured on the owner's link table: 8 of 1,034
/// linked pull requests were in that open list.
///
/// The link table itself is the list of repositories that can answer, so
/// the number is looked up there directly. Every repository carrying it
/// comes back -- two repos can both hold a `#1234`, and choosing one
/// would be a guess -- and the caller names each.
///
/// Also what a QUALIFIED query uses, filtered in the caller, because
/// `owner/repo` is case-insensitive on GitHub and a transferred
/// repository keeps its old owner in the links written before the
/// transfer. An exact `repo = ?` match missed both.
///
/// No index on `number` alone: the table holds one row per (session,
/// PR) -- 1,034 on the owner's machine -- so the scan is sub-millisecond
/// and an index would be a migration for nothing measurable.
pub fn sessions_for_pr_number(
    conn: &Connection,
    number: u64,
) -> Result<Vec<super::subagent::PrLink>, rusqlite::Error> {
    let mut q = conn.prepare(
        "SELECT session_id, repo, number, url, first_seen_at
           FROM claude_session_pr WHERE number = ?1
          ORDER BY repo, first_seen_at, session_id",
    )?;
    let rows = q.query_map([number as i64], |r| {
        Ok(super::subagent::PrLink {
            session_id: r.get(0)?,
            repo: r.get(1)?,
            number: r.get::<_, i64>(2)? as u64,
            url: r.get(3)?,
            first_seen_at: r.get(4)?,
        })
    })?;
    rows.collect()
}

/// Record one session's token usage (#1134).
///
/// Called from the import pass, which is the only place that can afford
/// the read: `usage.rs` measures the whole corpus at 3.8 s.
pub fn record_usage(
    conn: &Connection,
    session_id: &str,
    u: &super::usage::Usage,
    now: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO claude_session_usage
            (session_id, messages, input_tokens, output_tokens,
             cache_read, cache_creation, truncated, measured_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            session_id,
            u.messages as i64,
            u.input_tokens as i64,
            u.output_tokens as i64,
            u.cache_read_tokens as i64,
            u.cache_creation_tokens as i64,
            u.truncated as i64,
            now,
        ],
    )?;
    // Rewritten whole for this session: a re-measure can legitimately
    // find fewer models than before, and an upsert would leave the old
    // one behind with nothing to remove it.
    conn.execute(
        "DELETE FROM claude_session_model WHERE session_id = ?1",
        [session_id],
    )?;
    for m in &u.models {
        conn.execute(
            "INSERT OR REPLACE INTO claude_session_model (session_id, model, messages)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![session_id, m.model, m.messages as i64],
        )?;
    }
    Ok(())
}

/// Sessions whose stored usage is a floor, not a total (#1213).
///
/// The bulk import reads at most `usage::BUDGET_BYTES` per transcript,
/// which keeps the whole-corpus pass at 3.8 s. Measured on a real
/// machine, that bound leaves 0.33 GB unread -- and all of it sits in
/// **14 files**, which between them hold 53.6% of all session bytes.
///
/// So "what did this cost" is answered with a floor precisely on the
/// longest sessions, which are the ones anybody asks it about.
///
/// Returned newest-first so a backfill that stops early has done the
/// most useful work.
pub fn truncated_sessions(conn: &Connection) -> Result<Vec<String>, rusqlite::Error> {
    let mut q = conn.prepare(
        "SELECT session_id FROM claude_session_usage
         WHERE truncated = 1
         ORDER BY measured_at DESC",
    )?;
    let rows = q.query_map([], |r| r.get::<_, String>(0))?;
    rows.collect()
}

/// Re-measure one truncated session without the budget (#1213).
///
/// A BACKFILL, not a scan. The caller walks only the rows
/// [`truncated_sessions`] returns -- 14 files on the machine this was
/// measured on, one at a time, off the interactive path. #1086 measured
/// `summarise_whole` at 160 ms for the 76.7 MB worst case, so the whole
/// backfill is a few seconds of background work.
///
/// It runs **once**. The cache is keyed on the transcript, so a session
/// re-measured whole has `truncated` flipped to 0 and is never returned
/// again unless the file grows past the budget afresh.
///
/// The 3.8 s whole-corpus figure `BUDGET_BYTES` defends against is real
/// and is not incurred here: nothing re-reads the untruncated majority.
pub fn backfill_one(
    conn: &Connection,
    session_id: &str,
    path: &std::path::Path,
    now: &str,
) -> Result<bool, rusqlite::Error> {
    // A transcript that has gone leaves its previous measurement in
    // place rather than writing zeros over it -- the same rule the
    // import loop states: absent is not zero, and a session whose file
    // is temporarily unreadable has not suddenly cost nothing.
    let Ok(u) = super::usage::summarise_whole(path) else {
        return Ok(false);
    };
    record_usage(conn, session_id, &u, now)?;
    Ok(true)
}

/// Token usage summed across every measured session (#1134).
pub fn usage_profile(conn: &Connection) -> Result<super::usage::Profile, rusqlite::Error> {
    let mut out: super::usage::Profile = conn.query_row(
        "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read),0), COALESCE(SUM(cache_creation),0),
                COALESCE(SUM(messages),0), COUNT(*),
                COALESCE(SUM(truncated),0)
           FROM claude_session_usage",
        [],
        |r| {
            Ok(super::usage::Profile {
                input_tokens: r.get::<_, i64>(0)? as u64,
                output_tokens: r.get::<_, i64>(1)? as u64,
                cache_read_tokens: r.get::<_, i64>(2)? as u64,
                cache_creation_tokens: r.get::<_, i64>(3)? as u64,
                messages: r.get::<_, i64>(4)? as u64,
                sessions_measured: r.get::<_, i64>(5)? as u64,
                sessions_truncated: r.get::<_, i64>(6)? as u64,
                ..Default::default()
            })
        },
    )?;

    let (fresh, stale): (i64, i64) = conn.query_row("SELECT COALESCE(SUM(l.fresh=1),0),COALESCE(SUM(l.fresh=0),0) FROM claude_session_usage u JOIN claude_usage_ledger l ON l.session_id=u.session_id", [], |r| Ok((r.get(0)?,r.get(1)?)))?;
    out.fresh_sessions = fresh as u64;
    out.stale_sessions = stale as u64;
    out.unknown_sessions = out
        .sessions_measured
        .saturating_sub(out.fresh_sessions + out.stale_sessions);
    out.metadata_population = super::transcript::stored_progress(conn, "metadata")?;
    out.population = super::transcript::stored_progress(conn, "usage")?;
    if let Some(discovery) = super::transcript::stored_progress(conn, "import")? {
        if discovery.status == PopulationStatus::Running {
            out.population = Some(discovery);
        }
    }

    let mut q = conn.prepare(
        "SELECT model, SUM(messages) FROM claude_session_model
          GROUP BY model ORDER BY SUM(messages) DESC",
    )?;
    out.models = q
        .query_map([], |r| {
            Ok(super::usage::ModelCount {
                model: r.get(0)?,
                messages: r.get::<_, i64>(1)? as u64,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Joined to `claude_session` for the cwd, because usage rows carry
    // only the id. A session whose cwd was never recorded is excluded
    // rather than bucketed as empty: an unknown directory is not a
    // directory.
    let mut q = conn.prepare(
        "SELECT s.cwd, SUM(u.output_tokens), COUNT(*)
           FROM claude_session_usage u
           JOIN claude_session s ON s.session_id = u.session_id
          WHERE s.cwd IS NOT NULL
          GROUP BY s.cwd
          ORDER BY SUM(u.output_tokens) DESC
          LIMIT ?1",
    )?;
    out.by_directory = q
        .query_map([super::usage::TOP_DIRECTORIES as i64], |r| {
            Ok(super::usage::DirectoryUsage {
                cwd: r.get(0)?,
                output_tokens: r.get::<_, i64>(1)? as u64,
                sessions: r.get::<_, i64>(2)? as u64,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(out)
}

/// Continue changed usage measurements from durable per-file checkpoints.
pub(crate) fn usage_pass(
    conn: &mut Connection,
    scan: &Scan,
    control: &super::transcript::PopulationControl,
) -> rusqlite::Result<Imported> {
    let generation = super::transcript::population_generation(conn, "usage")?;
    let _activity = super::transcript::PopulationActivity::new(conn, "usage", generation);
    let mut out = Imported {
        population: scan.population_progress(),
        unreadable_dirs: scan.unreadable_dirs.clone(),
        unreadable_files: scan.unreadable_files.clone(),
        ..Default::default()
    };
    populate_usage(
        conn,
        &scan.sessions,
        control,
        generation,
        &mut out,
        &chrono::Utc::now().to_rfc3339(),
    )?;
    Ok(out)
}

pub(crate) fn metadata_pass(
    conn: &mut Connection,
    scan: &Scan,
    control: &super::transcript::PopulationControl,
) -> rusqlite::Result<PopulationProgress> {
    use super::transcript::{
        generation_current, persist_progress, PopulationActivity, PopulationPhase,
    };
    let generation = super::transcript::population_generation(conn, "metadata")?;
    let _activity = PopulationActivity::new(conn, "metadata", generation);
    let mut progress = scan.population_progress();
    // Metadata coverage keeps the existing absent-root qualification; an
    // import on a machine with no history is still a complete empty list.
    progress.total_known &= scan.absent_root.is_none();
    progress.phase = PopulationPhase::Metadata;
    progress.failed = scan.unreadable_files.len() as u64;
    persist_progress(conn, "metadata", generation, &progress)?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut todo = Vec::new();
    for source in &scan.sessions {
        if control.stopped() || !generation_current(conn, "metadata", generation)? {
            progress.status = control
                .stop_reason()
                .unwrap_or(PopulationStatus::Invalidated);
            break;
        }
        let before = match super::transcript::fingerprint(std::path::Path::new(&source.path)) {
            Ok(value) => value,
            Err(_) => {
                progress.failed += 1;
                continue;
            }
        };
        let key = format!("claude_metadata_checkpoint:{}", source.session_id);
        let fingerprint = serde_json::to_string(&(&source.path, &before))
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let known: Option<String> = conn
            .query_row("SELECT value FROM settings WHERE key=?1", [&key], |r| {
                r.get(0)
            })
            .optional()?;
        if known.as_ref() == Some(&fingerprint) {
            progress.completed += 1;
            progress.unchanged += 1;
        } else {
            todo.push((source, before, key, fingerprint));
        }
    }
    super::retry_queue::rotate_pending(conn, "metadata", &mut todo, |entry| &entry.0.session_id)?;
    for (source, before, key, fingerprint) in
        todo.into_iter().take(super::search::SESSIONS_PER_PASS)
    {
        if control.stopped() || !generation_current(conn, "metadata", generation)? {
            progress.status = control
                .stop_reason()
                .unwrap_or(PopulationStatus::Invalidated);
            break;
        }
        if !super::retry_queue::checkpoint_attempt(
            conn,
            "metadata",
            generation,
            &source.session_id,
        )? {
            progress.status = PopulationStatus::Invalidated;
            break;
        }
        let path = std::path::Path::new(&source.path);
        let mut metadata = match super::transcript::extract_controlled(path, control) {
            Ok(value) => value,
            Err(_) => {
                if let Some(reason) = control.stop_reason() {
                    progress.status = reason;
                    break;
                }
                progress.failed += 1;
                continue;
            }
        };
        if super::transcript::fingerprint(path).ok().as_ref() != Some(&before) {
            progress.failed += 1;
            continue;
        }
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if control.stopped() || !generation_current(&tx, "metadata", generation)? {
            progress.status = control
                .stop_reason()
                .unwrap_or(PopulationStatus::Invalidated);
            break;
        }
        // A newer hook may have updated the working context after this disk
        // snapshot. Preserve those facts; upsert still widens activity bounds
        // and never replaces a user's name.
        let previous: Option<Option<String>> = tx
            .query_row(
                "SELECT last_activity_at FROM claude_session WHERE session_id=?1",
                [&source.session_id],
                |r| r.get(0),
            )
            .optional()?;
        if previous.flatten().as_ref().is_some_and(|latest| {
            metadata
                .last_activity_at
                .as_ref()
                .is_none_or(|disk| latest > disk)
        }) {
            metadata.cwd = None;
            metadata.git_branch = None;
            metadata.claude_version = None;
        }
        let result = upsert(&tx, &metadata, &now).and_then(|()| {
            tx.execute("INSERT INTO settings(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", rusqlite::params![key,fingerprint]).map(|_| ())
        });
        match result {
            Ok(()) => {
                progress.completed += 1;
                persist_progress(&tx, "metadata", generation, &progress)?;
                tx.commit()?;
            }
            Err(_) => progress.failed += 1,
        }
    }
    progress.finish();
    persist_progress(conn, "metadata", generation, &progress)?;
    Ok(progress)
}

fn populate_usage(
    conn: &mut Connection,
    sessions: &[Transcript],
    control: &super::transcript::PopulationControl,
    generation: i64,
    out: &mut Imported,
    now: &str,
) -> rusqlite::Result<()> {
    // The caller carries typed discovery status and denominator knowledge.
    // Rebuilding this from unreadable errors would turn a stopped empty scan
    // into a successful 0/0 receipt.
    out.population.failed = out.unreadable_files.len() as u64;
    super::transcript::persist_progress(conn, "usage", generation, &out.population)?;
    let observed: std::collections::HashSet<&str> =
        sessions.iter().map(|t| t.session_id.as_str()).collect();
    let fresh_ids = conn
        .prepare("SELECT session_id FROM claude_usage_ledger WHERE fresh=1")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for id in fresh_ids
        .iter()
        .filter(|id| !observed.contains(id.as_str()))
    {
        conn.execute("UPDATE claude_usage_ledger SET fresh=0 WHERE session_id=?1 AND fresh<>0 AND EXISTS(SELECT 1 FROM claude_population_generation WHERE lane='usage' AND generation=?2)", rusqlite::params![id,generation])?;
    }
    let mut todo = Vec::new();
    let mut checked = std::collections::HashSet::new();
    // Classify every observed source before the bounded reads, so changed
    // files waiting for a later pass cannot masquerade as fresh measurements.
    for t in sessions {
        if control.stopped() || !super::transcript::generation_current(conn, "usage", generation)? {
            out.population.status = control
                .stop_reason()
                .unwrap_or(PopulationStatus::Invalidated);
            break;
        }
        checked.insert(t.session_id.as_str());
        let path = std::path::Path::new(&t.path);
        let before = match super::transcript::fingerprint(path) {
            Ok(value) => value,
            Err(e) => {
                conn.execute(
                    "UPDATE claude_usage_ledger SET fresh=0 WHERE session_id=?1 AND fresh<>0 AND EXISTS(SELECT 1 FROM claude_population_generation WHERE lane='usage' AND generation=?2)",
                    rusqlite::params![&t.session_id,generation],
                )?;
                out.unreadable_files
                    .push(format!("{}: could not measure usage source: {e}", t.path));
                out.population.failed += 1;
                continue;
            }
        };
        let known: Option<(String, i64, String, bool)> = conn.query_row("SELECT path,size_bytes,mtime_ns,fresh FROM claude_usage_ledger WHERE session_id=?1", [&t.session_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        if known.as_ref() == Some(&(t.path.clone(), before.0, before.1.clone(), true)) {
            out.population.completed += 1;
            out.population.unchanged += 1;
            continue;
        }
        conn.execute("INSERT INTO claude_usage_ledger(session_id,path,size_bytes,mtime_ns,fresh) SELECT ?1,?2,?3,?4,0 WHERE EXISTS(SELECT 1 FROM claude_population_generation WHERE lane='usage' AND generation=?5) ON CONFLICT(session_id) DO UPDATE SET fresh=0 WHERE fresh<>0", rusqlite::params![t.session_id,t.path,before.0,before.1,generation])?;
        todo.push((t, before));
    }
    // Cancellation cannot leave unvisited retained measurements labelled current.
    for id in fresh_ids.iter().filter(|id| !checked.contains(id.as_str())) {
        conn.execute("UPDATE claude_usage_ledger SET fresh=0 WHERE session_id=?1 AND fresh<>0 AND EXISTS(SELECT 1 FROM claude_population_generation WHERE lane='usage' AND generation=?2)", rusqlite::params![id,generation])?;
    }
    super::retry_queue::rotate_pending(conn, "usage", &mut todo, |item| &item.0.session_id)?;
    for (t, before) in todo.into_iter().take(super::search::SESSIONS_PER_PASS) {
        if control.stopped() || !super::transcript::generation_current(conn, "usage", generation)? {
            out.population.status = control
                .stop_reason()
                .unwrap_or(PopulationStatus::Invalidated);
            break;
        }
        let path = std::path::Path::new(&t.path);
        if !super::retry_queue::checkpoint_attempt(conn, "usage", generation, &t.session_id)? {
            out.population.status = PopulationStatus::Invalidated;
            break;
        }
        let u = match super::usage::read_and_sum_cancellable(path, None, &|| control.stopped()) {
            Ok(u) => u,
            Err(e) => {
                if let Some(reason) = control.stop_reason() {
                    out.population.status = reason;
                    break;
                }
                out.unreadable_files.push(e);
                out.population.failed += 1;
                continue;
            }
        };
        if super::transcript::fingerprint(path).ok().as_ref() != Some(&before) {
            out.population.status = PopulationStatus::Invalidated;
            out.unreadable_files
                .push(format!("{}: changed while measuring usage", t.path));
            continue;
        }
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if control.stopped() || !super::transcript::generation_current(&tx, "usage", generation)? {
            out.population.status = control
                .stop_reason()
                .unwrap_or(PopulationStatus::Invalidated);
            break;
        }
        let result = record_usage(&tx, &t.session_id, &u, now).and_then(|()| {
            tx.execute("INSERT INTO claude_usage_ledger(session_id,path,size_bytes,mtime_ns,fresh) VALUES (?1,?2,?3,?4,1) ON CONFLICT(session_id) DO UPDATE SET path=?2,size_bytes=?3,mtime_ns=?4,fresh=1", rusqlite::params![t.session_id,t.path,before.0,before.1]).map(|_| ())
        });
        match result {
            Ok(()) => {
                out.population.completed += 1;
                super::transcript::persist_progress(&tx, "usage", generation, &out.population)?;
                tx.commit()?;
            }
            Err(e) => {
                out.write_failures
                    .push(format!("{}: could not store usage: {e}", t.session_id));
                out.population.failed += 1;
            }
        }
    }
    out.population.finish();
    super::transcript::persist_progress(conn, "usage", generation, &out.population)?;
    Ok(())
}

/// Accept stable metadata and usage per file; replace the corpus-derived
/// graph atomically only when discovery is still valid. Failed graph writes
/// retain the previous graph without discarding accepted local checkpoints.
pub fn import(conn: &mut Connection, scan: Scan) -> Result<Imported, rusqlite::Error> {
    import_controlled(
        conn,
        scan,
        &super::transcript::PopulationControl::discovery(),
    )
}

pub(crate) fn import_controlled(
    conn: &mut Connection,
    scan: Scan,
    control: &super::transcript::PopulationControl,
) -> Result<Imported, rusqlite::Error> {
    // Callers with a pre-discovery timestamp use import_controlled_since.
    // Zero safely replays sources when the scan's start time is unknown.
    import_controlled_since(conn, scan, control, 0)
}

pub(crate) fn import_controlled_since(
    conn: &mut Connection,
    scan: Scan,
    control: &super::transcript::PopulationControl,
    since_ms: i64,
) -> Result<Imported, rusqlite::Error> {
    let generation = super::transcript::population_generation(conn, "import")?;
    let _activity = super::transcript::PopulationActivity::new(conn, "import", generation);
    let out = import_generation(conn, scan, control, generation, since_ms)?;
    super::transcript::persist_progress(conn, "import", generation, &out.population)?;
    Ok(out)
}

fn import_generation(
    conn: &mut Connection,
    scan: Scan,
    control: &super::transcript::PopulationControl,
    generation: i64,
    since_ms: i64,
) -> Result<Imported, rusqlite::Error> {
    let complete =
        !scan.is_partial() && scan.absent_root.is_none() && scan.subagents.unreadable.is_empty();
    let now = chrono::Utc::now().to_rfc3339();
    let mut out = Imported {
        population: scan.population_progress(),
        subagent_files_skipped: scan.subagent_files_skipped,
        session_bytes: scan.session_bytes,
        subagent_bytes: scan.subagent_bytes,
        unsized_files: scan.unsized_files,
        unreadable_dirs: scan.unreadable_dirs,
        unreadable_files: scan.unreadable_files,
        metadata_beyond_first_record: scan.metadata_beyond_first_record,
        elapsed_ms: scan.elapsed_ms,
        absent_root: scan.absent_root,
        subagent_map_unreadable: scan.subagents.unreadable.clone(),
        ..Default::default()
    };

    // Which sessions belong to each agent worktree. Needed before the
    // attribution because a child must never be its own parent, and only
    // this grouping knows which sessions are the agent's own.
    let mut own: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for t in &scan.sessions {
        if let Kind::Subagent { agent_id } = Kind::classify(t.cwd.as_deref()) {
            own.entry(agent_id).or_default().push(t.session_id.clone());
        }
    }

    let mut metadata_progress = out.population.clone();
    metadata_progress.phase = super::transcript::PopulationPhase::Metadata;
    super::transcript::persist_progress(conn, "import", generation, &metadata_progress)?;
    // Session metadata is local to one file: stable neighbors remain useful
    // even when a changing source prevents whole-corpus graph publication.
    for t in &scan.sessions {
        let source = scan
            .source_fingerprints
            .iter()
            .find(|(path, _, _)| path == std::path::Path::new(&t.path));
        if let Some((path, size, modified)) = source {
            if super::transcript::fingerprint(path).ok().as_ref()
                != Some(&(*size, modified.clone()))
            {
                continue;
            }
        }
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if control.stopped() || !super::transcript::generation_current(&tx, "import", generation)? {
            out.population.status = control
                .stop_reason()
                .unwrap_or(PopulationStatus::Invalidated);
            out.population.finish();
            return Ok(out);
        }
        match upsert(&tx, t, &now) {
            Ok(()) => {
                metadata_progress.completed += 1;
                if metadata_progress.completed.is_multiple_of(25)
                    || metadata_progress.completed == metadata_progress.total
                {
                    super::transcript::persist_progress(
                        &tx,
                        "import",
                        generation,
                        &metadata_progress,
                    )?;
                }
                tx.commit()?;
                out.sessions += 1;
            }
            Err(e) => out
                .write_failures
                .push(format!("{}: could not store metadata: {e}", t.session_id)),
        }
    }
    // End metadata reporting before body reads so the usage lane owns live progress.
    metadata_progress.finish();
    super::transcript::persist_progress(conn, "import", generation, &metadata_progress)?;
    let usage_generation = super::transcript::population_generation(conn, "usage")?;
    let _usage_activity =
        super::transcript::PopulationActivity::new(conn, "usage", usage_generation);
    populate_usage(
        conn,
        &scan.sessions,
        &control.bounded(),
        usage_generation,
        &mut out,
        &now,
    )?;
    let mut source_changed = false;
    for (path, size, modified) in &scan.source_fingerprints {
        if super::transcript::discovery_fingerprint(path, control)
            .ok()
            .as_ref()
            != Some(&(*size, modified.clone()))
        {
            source_changed = true;
            out.unreadable_files.push(format!(
                "{}: changed during discovery; previous derived facts retained",
                path.display()
            ));
        }
    }
    if source_changed {
        out.population.status = control
            .stop_reason()
            .unwrap_or(PopulationStatus::Invalidated);
        return Ok(out);
    }

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if control.stopped() || !super::transcript::generation_current(&tx, "import", generation)? {
        out.population.status = control
            .stop_reason()
            .unwrap_or(PopulationStatus::Invalidated);
        out.population.finish();
        return Ok(out);
    }
    let failures_before_graph = out.write_failures.len();
    let mut parents = Vec::new();
    for t in scan.sessions.iter().filter(|_| complete) {
        let Kind::Subagent { agent_id } = Kind::classify(t.cwd.as_deref()) else {
            continue;
        };
        let siblings = own.get(&agent_id).cloned().unwrap_or_default();
        let (parent, why) = match scan.subagents.parent_of(&agent_id, &siblings) {
            Parent::Session { session_id } => (Some(session_id), None),
            Parent::Unattributed { why } => (None, Some(why)),
        };
        out.subagents += 1;
        out.subagents_attributed += usize::from(parent.is_some());
        parents.push(graph::Attribution {
            session_id: t.session_id.clone(),
            agent_id,
            parent,
            why,
        });
    }
    // Links and attribution still publish atomically under the source fence.
    // Only semantic differences mutate rows; partial scans retain missing edges.
    if let Err(error) = graph::publish(
        &tx,
        &scan.subagents.pr_links,
        complete.then_some(parents.as_slice()),
        &now,
    ) {
        out.write_failures
            .push(format!("could not publish derived session graph: {error}"));
    }
    if out.write_failures.len() > failures_before_graph {
        out.population.status = PopulationStatus::Failed;
        out.population.failed += (out.write_failures.len() - failures_before_graph) as u64;
        out.subagents = 0;
        out.subagents_attributed = 0;
        return Ok(out);
    }
    if complete {
        // Graph publication and its continuation are one accepted snapshot.
        // Usage can still be deferred without disabling future link discovery.
        super::linkscan::reset(&tx, since_ms)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    }
    tx.commit()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::transcript::Transcript;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn empty_stopped_discovery_receipts_keep_unknown_total_and_stop_status() {
        let mut conn = db();
        for status in [PopulationStatus::Cancelled, PopulationStatus::TimedOut] {
            let scan = Scan {
                stop_reason: Some(status),
                ..Default::default()
            };
            let control = super::super::transcript::PopulationControl::discovery();
            let metadata = metadata_pass(&mut conn, &scan, &control).unwrap();
            let usage = usage_pass(&mut conn, &scan, &control).unwrap().population;
            for progress in [metadata, usage] {
                assert_eq!(progress.status, status);
                assert!(!progress.total_known);
                assert_eq!(progress.completed, 0);
            }
        }
    }

    #[test]
    fn interrupted_or_incomplete_discovery_does_not_call_retained_usage_current() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let mut session = t("s");
        session.path = path.to_string_lossy().into_owned();
        let complete = Scan {
            sessions: vec![session],
            ..Default::default()
        };
        let mut conn = db();
        for cancelled in [false, true] {
            usage_pass(
                &mut conn,
                &complete,
                &super::super::transcript::PopulationControl::default(),
            )
            .unwrap();
            assert_eq!(usage_profile(&conn).unwrap().fresh_sessions, 1);
            let control = super::super::transcript::PopulationControl::default();
            let partial = Scan {
                unreadable_dirs: vec!["unavailable project".into()],
                ..Default::default()
            };
            if cancelled {
                control.cancel();
            }
            let result = usage_pass(
                &mut conn,
                if cancelled { &complete } else { &partial },
                &control,
            )
            .unwrap();
            assert!(result.is_partial());
            let profile = usage_profile(&conn).unwrap();
            assert_eq!(profile.sessions_measured, 1);
            assert_eq!(profile.fresh_sessions, 0);
            assert_eq!(profile.stale_sessions, 1);
        }
    }

    #[test]
    fn typed_population_distinguishes_cancellation_deadline_and_dirty_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let mut session = t("s");
        session.path = path.to_string_lossy().into_owned();
        let scan = Scan {
            sessions: vec![session],
            ..Default::default()
        };
        let mut conn = db();
        let control = super::super::transcript::PopulationControl::default();
        usage_pass(&mut conn, &scan, &control).unwrap();
        assert_eq!(usage_profile(&conn).unwrap().fresh_sessions, 1);
        let expired = control.with_budget(std::time::Duration::ZERO);
        let result = usage_pass(&mut conn, &scan, &expired).unwrap();
        assert_eq!(
            result.population.status,
            super::super::transcript::PopulationStatus::TimedOut
        );
        std::fs::write(&path, "{}\n{}\n").unwrap();
        let cancel = control.clone();
        super::super::usage::READ_OBSERVER
            .with(|hook| *hook.borrow_mut() = Some(Box::new(move |_| cancel.cancel())));
        let result = usage_pass(&mut conn, &scan, &control).unwrap();
        super::super::usage::READ_OBSERVER.with(|hook| *hook.borrow_mut() = None);
        assert_eq!(
            result.population.status,
            super::super::transcript::PopulationStatus::Cancelled
        );
        assert_eq!(result.population.total, 1);
        assert_eq!(result.population.completed, 0);
        assert_eq!(result.population.remaining, 1);
        let profile = usage_profile(&conn).unwrap();
        assert_eq!(profile.sessions_measured, 1);
        assert_eq!(profile.fresh_sessions, 0);
        assert_eq!(profile.stale_sessions, 1);
    }

    fn synthetic_graph() -> Scan {
        let mut child = t("child");
        child.cwd = Some(agent_cwd("cccccccc3333"));
        let mut map = crate::claude::subagent::Map::default();
        map_note(&mut map, "cccccccc3333", "parent", "2026-09-14T01:00:00Z");
        map.pr_links.push(crate::claude::subagent::PrLink {
            session_id: "parent".into(),
            repo: "octocat/hello-world".into(),
            number: 1,
            url: "https://github.com/octocat/hello-world/pull/1".into(),
            first_seen_at: None,
        });
        Scan {
            sessions: vec![t("parent"), child],
            subagents: map,
            ..Default::default()
        }
    }

    #[test]
    fn unchanged_graph_has_zero_mutations_and_keeps_resolution_time() {
        let mut conn = db();
        let mut scan = synthetic_graph();
        import(&mut conn, scan.clone()).unwrap();
        conn.execute("UPDATE claude_subagent SET resolved_at='accepted-time'", [])
            .unwrap();
        conn.execute_batch("CREATE TABLE graph_mutations(name TEXT, op TEXT);")
            .unwrap();
        for table in ["claude_session_pr", "claude_subagent"] {
            for op in ["INSERT", "UPDATE", "DELETE"] {
                conn.execute_batch(&format!("CREATE TRIGGER count_{table}_{op} AFTER {op} ON {table} BEGIN INSERT INTO graph_mutations VALUES('{table}', '{op}'); END;")).unwrap();
            }
        }
        // Intermediate duplicate entries must not cause transient graph writes:
        // current import semantics are final incoming value wins for each key.
        let mut earlier = scan.subagents.pr_links[0].clone();
        earlier.url = "https://github.com/octocat/hello-world/pull/old".into();
        scan.subagents.pr_links.insert(0, earlier);
        import(&mut conn, scan.clone()).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM graph_mutations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT resolved_at FROM claude_subagent", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "accepted-time"
        );
        scan.subagents.pr_links.last_mut().unwrap().first_seen_at = Some("new evidence".into());
        import(&mut conn, scan.clone()).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM graph_mutations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            prs_for_session(&conn, "parent").unwrap()[0]
                .first_seen_at
                .as_deref(),
            Some("new evidence")
        );
        conn.execute("DELETE FROM graph_mutations", []).unwrap();
        scan.subagents.pr_links.clear();
        import(&mut conn, scan).unwrap();
        assert!(prs_for_session(&conn, "parent").unwrap().is_empty());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM graph_mutations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn review_corpus_failed_graph_replacement_retains_prior_links_and_attribution() {
        let mut conn = db();
        let mut child = t("child");
        child.cwd = Some(agent_cwd("cccccccc3333"));
        let mut map = crate::claude::subagent::Map::default();
        map_note(&mut map, "cccccccc3333", "parent", "2026-09-14T01:00:00Z");
        map.pr_links.push(crate::claude::subagent::PrLink {
            session_id: "parent".into(),
            repo: "octocat/hello-world".into(),
            number: 1,
            url: "https://github.com/octocat/hello-world/pull/1".into(),
            first_seen_at: None,
        });
        let mut scan = Scan {
            sessions: vec![t("parent"), child],
            subagents: map,
            ..Default::default()
        };
        import(&mut conn, scan.clone()).unwrap();
        scan.subagents.pr_links[0].number = 2;
        scan.sessions[1].cwd = Some(agent_cwd("dddddddd4444"));
        conn.execute_batch("CREATE TRIGGER reject_link BEFORE INSERT ON claude_session_pr BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
        assert!(import(&mut conn, scan.clone()).unwrap().is_partial());
        assert_eq!(prs_for_session(&conn, "parent").unwrap().len(), 1);
        conn.execute_batch("DROP TRIGGER reject_link; CREATE TRIGGER reject_parent BEFORE INSERT ON claude_subagent BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
        assert!(import(&mut conn, scan).unwrap().is_partial());
        assert_eq!(prs_for_session(&conn, "parent").unwrap()[0].number, 1);
        assert_eq!(
            conn.query_row(
                "SELECT parent_session_id FROM claude_subagent WHERE session_id='child'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "parent"
        );
    }

    #[test]
    fn review_corpus_one_changing_file_does_not_starve_stable_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("stable.jsonl"), "{}\n").unwrap();
        let changing = project.join("changing.jsonl");
        std::fs::write(&changing, "{}\n").unwrap();
        let scan = super::super::transcript::scan(dir.path());
        std::fs::write(&changing, "{}\n{}\n").unwrap();
        let mut conn = db();
        let result = import(&mut conn, scan).unwrap();
        assert!(result.is_partial());
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM claude_session WHERE session_id='stable'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM claude_session WHERE session_id='changing'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn failed_prefix_does_not_starve_usage_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = (0..101)
            .map(|n| {
                let mut session = t(&format!("s{n:03}"));
                let path = dir.path().join(format!("s{n:03}.jsonl"));
                std::fs::write(&path, "{}\n").unwrap();
                session.path = path.to_string_lossy().into_owned();
                session
            })
            .collect();
        let scan = Scan {
            sessions,
            ..Default::default()
        };
        let path = dir.path().join("usage-fair.db");
        let mut conn = crate::store::open_db(&path).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_usage_prefix BEFORE INSERT ON claude_session_usage WHEN NEW.session_id <> 's100' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;").unwrap();
        let control = super::super::transcript::PopulationControl::discovery();
        let first = usage_pass(&mut conn, &scan, &control).unwrap();
        assert_eq!(first.population.completed, 0);
        assert_eq!(first.write_failures.len(), 100);
        drop(conn);
        let mut conn = crate::store::open_db(&path).unwrap();
        let second = usage_pass(&mut conn, &scan, &control).unwrap();
        assert_eq!(
            second.population.completed, 1,
            "failed neighbors must not starve usage after restart"
        );
        assert!(second.is_partial());
        conn.execute_batch("DROP TRIGGER fail_usage_prefix")
            .unwrap();
        let recovered = usage_pass(&mut conn, &scan, &control).unwrap();
        assert_eq!(recovered.population.completed, 101);
        assert!(!recovered.is_partial());
    }

    #[test]
    fn usage_population_is_bounded_and_resumes_without_reopening_completed_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut sessions = Vec::new();
        for n in 0..101 {
            let path = dir.path().join(format!("{n}.jsonl"));
            std::fs::write(&path, "{}\n").unwrap();
            let mut session = t(&n.to_string());
            session.path = path.to_string_lossy().into_owned();
            sessions.push(session);
        }
        let scan = Scan {
            sessions,
            ..Default::default()
        };
        let mut conn = db();
        let reads = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = reads.clone();
        super::super::usage::READ_OBSERVER.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_| observed.set(observed.get() + 1)))
        });
        assert!(usage_pass(
            &mut conn,
            &scan,
            &super::super::transcript::PopulationControl::default()
        )
        .unwrap()
        .is_partial());
        assert_eq!(reads.get(), 100);
        reads.set(0);
        assert!(!usage_pass(
            &mut conn,
            &scan,
            &super::super::transcript::PopulationControl::default()
        )
        .unwrap()
        .is_partial());
        assert_eq!(reads.get(), 1);
        super::super::usage::READ_OBSERVER.with(|hook| *hook.borrow_mut() = None);
    }

    #[test]
    fn live_metadata_is_bounded_checkpointed_and_preserves_newer_facts() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        for n in 0..101 {
            std::fs::write(project.join(format!("{n}.jsonl")), r#"{"type":"user","cwd":"/work","gitBranch":"main","timestamp":"2026-10-01T00:00:00Z","message":{"content":"opening ask"}}
"#).unwrap();
        }
        let database = dir.path().join("metadata.db");
        let mut conn = Connection::open(&database).unwrap();
        crate::store::migrate(&conn).unwrap();
        let scan = super::super::transcript::corpus(dir.path());
        // This spans several live passes and a restart. File-count bounds and
        // merge semantics must not depend on the machine finishing all of them
        // within one production pass's 60-second deadline.
        let control = super::super::transcript::PopulationControl::discovery();
        let first = metadata_pass(&mut conn, &scan, &control).unwrap();
        assert_eq!(first.completed, 100);
        assert_eq!(first.status, PopulationStatus::Deferred);
        assert_eq!(
            usage_profile(&conn).unwrap().metadata_population,
            Some(first)
        );
        drop(conn);
        let mut conn = Connection::open(&database).unwrap();
        let second = metadata_pass(&mut conn, &scan, &control).unwrap();
        assert_eq!(second.completed, 101);
        assert_eq!(second.unchanged, 100);
        usage_pass(&mut conn, &scan, &control).unwrap();
        let profile = usage_profile(&conn).unwrap();
        assert_eq!(profile.by_directory[0].cwd, "/work");
        assert_eq!(profile.by_directory[0].sessions, 100);
        std::fs::write(project.join("1.jsonl"), "{\"cwd\":\"/updated\",\"gitBranch\":\"topic\",\"timestamp\":\"2026-10-15T00:00:00Z\"}\n").unwrap();
        let changed = metadata_pass(&mut conn, &scan, &control).unwrap();
        assert_eq!(changed.status, PopulationStatus::Complete);
        assert_eq!(changed.unchanged, 100);
        assert_eq!(
            conn.query_row(
                "SELECT git_branch FROM claude_session WHERE session_id='1'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "topic"
        );

        conn.execute("UPDATE claude_session SET name='my name',cwd='/newer-hook',last_activity_at='2026-10-16T00:00:00Z' WHERE session_id='0'", []).unwrap();
        std::fs::write(project.join("0.jsonl"), r#"{"type":"user","cwd":"/old-disk","gitBranch":"changed","timestamp":"2026-10-02T00:00:00Z","message":{"content":"changed ask"}}
"#).unwrap();
        let changed = metadata_pass(&mut conn, &scan, &control).unwrap();
        assert_eq!(changed.status, PopulationStatus::Complete);
        assert_eq!(changed.unchanged, 100);
        let row: (String,String,String,String) = conn.query_row("SELECT name,cwd,last_activity_at,opening_prompt FROM claude_session WHERE session_id='0'", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert_eq!(
            row,
            (
                "my name".into(),
                "/newer-hook".into(),
                "2026-10-16T00:00:00Z".into(),
                "changed ask".into()
            )
        );
        let warm = metadata_pass(&mut conn, &scan, &control).unwrap();
        assert_eq!(warm.unchanged, 101);
        control.cancel();
        assert_eq!(
            metadata_pass(&mut conn, &scan, &control).unwrap().status,
            PopulationStatus::Cancelled
        );
    }

    #[test]
    fn metadata_timeout_retains_facts_and_checkpoint_until_next_pass() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timeout.jsonl");
        std::fs::write(&path, "{\"cwd\":\"/before\"}\n").unwrap();
        let scan = Scan {
            sessions: vec![Transcript {
                session_id: "timeout".into(),
                path: path.to_string_lossy().into_owned(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut conn = db();
        let control = super::super::transcript::PopulationControl::discovery();
        assert_eq!(
            metadata_pass(&mut conn, &scan, &control).unwrap().completed,
            1
        );
        let checkpoint = |conn: &Connection| -> String {
            conn.query_row(
                "SELECT value FROM settings WHERE key='claude_metadata_checkpoint:timeout'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        let accepted = checkpoint(&conn);
        std::fs::write(&path, "{\"cwd\":\"/changed-directory\"}\n").unwrap();
        let expired = control.with_budget(std::time::Duration::ZERO);
        let stopped = metadata_pass(&mut conn, &scan, &expired).unwrap();
        assert_eq!(stopped.status, PopulationStatus::TimedOut);
        assert_eq!(stopped.completed, 0);
        assert_eq!(checkpoint(&conn), accepted);
        assert_eq!(
            conn.query_row(
                "SELECT cwd FROM claude_session WHERE session_id='timeout'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "/before"
        );
        let recovered = metadata_pass(&mut conn, &scan, &control).unwrap();
        assert_eq!(recovered.status, PopulationStatus::Complete);
        assert_eq!(recovered.unchanged, 0);
        assert_ne!(checkpoint(&conn), accepted);
        assert_eq!(
            conn.query_row(
                "SELECT cwd FROM claude_session WHERE session_id='timeout'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "/changed-directory"
        );
    }

    #[test]
    fn metadata_checkpoint_failure_rolls_back_row_and_retries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.jsonl");
        std::fs::write(&path, "{\"cwd\":\"/work\"}\n").unwrap();
        let scan = Scan {
            sessions: vec![Transcript {
                session_id: "new".into(),
                path: path.to_string_lossy().into_owned(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut conn = db();
        conn.execute_batch("CREATE TRIGGER reject_metadata BEFORE INSERT ON settings WHEN NEW.key LIKE 'claude_metadata_checkpoint:%' BEGIN SELECT RAISE(FAIL,'checkpoint rejected'); END;").unwrap();
        let control = super::super::transcript::PopulationControl::discovery();
        assert_eq!(metadata_pass(&mut conn, &scan, &control).unwrap().failed, 1);
        assert_eq!(
            conn.query_row("SELECT count(*) FROM claude_session", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        conn.execute_batch("DROP TRIGGER reject_metadata").unwrap();
        assert_eq!(
            metadata_pass(&mut conn, &scan, &control).unwrap().completed,
            1
        );
        assert_eq!(
            conn.query_row("SELECT cwd FROM claude_session", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "/work"
        );
    }

    #[test]
    fn graph_cursor_write_failure_rolls_back_graph() {
        let mut conn = db();
        conn.execute("INSERT INTO claude_session_pr(session_id,repo,number,url) VALUES ('old','o/r',1,'https://github.com/o/r/pull/1')", []).unwrap();
        conn.execute_batch("CREATE TRIGGER reject_cursor BEFORE INSERT ON claude_link_baseline BEGIN SELECT RAISE(FAIL,'cursor rejected'); END;").unwrap();
        assert!(import(&mut conn, Scan::default()).is_err());
        assert_eq!(prs_for_session(&conn, "old").unwrap().len(), 1);
    }

    #[test]
    fn accepted_graph_starts_links_with_deferred_usage() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        for n in 0..101 {
            std::fs::write(project.join(format!("{n}.jsonl")), "{}\n").unwrap();
        }
        let mut conn = db();
        let result = import(&mut conn, super::super::transcript::scan(dir.path())).unwrap();
        assert_eq!(result.population.status, PopulationStatus::Deferred);
        use std::io::Write;
        let mut source = std::fs::OpenOptions::new()
            .append(true)
            .open(project.join("0.jsonl"))
            .unwrap();
        writeln!(source, "{{\"type\":\"pr-link\",\"sessionId\":\"0\",\"prNumber\":918,\"prUrl\":\"https://github.com/o/r/pull/918\",\"prRepository\":\"o/r\"}}").unwrap();
        let refresh = super::super::linkscan::refresh(
            &mut conn,
            &super::super::transcript::corpus(dir.path()),
            1024 * 1024,
        )
        .unwrap();
        assert!(
            !refresh.not_started,
            "accepted graph must install a cursor despite usage backlog"
        );
        assert_eq!(prs_for_session(&conn, "0").unwrap()[0].number, 918);
    }

    #[test]
    fn empty_directory_discovery_race_cannot_delete_accepted_links() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("empty-project");
        std::fs::create_dir(&project).unwrap();
        let old = super::super::transcript::scan(dir.path());
        std::fs::write(project.join("new.jsonl"), "{}\n").unwrap();
        let mut conn = db();
        conn.execute("INSERT INTO claude_session_pr(session_id,repo,number,url) VALUES ('new','octocat/hello-world',1,'https://github.com/octocat/hello-world/pull/1')", []).unwrap();
        let result = import(&mut conn, old).unwrap();
        assert!(result.is_partial());
        assert_eq!(prs_for_session(&conn, "new").unwrap().len(), 1);
    }

    #[test]
    fn changed_discovery_source_cannot_replace_accepted_links() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let path = project.join("session.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let scan = super::super::transcript::scan(dir.path());
        std::fs::write(&path, "{}\n{}\n").unwrap();
        let mut conn = db();
        conn.execute("INSERT INTO claude_session_pr(session_id,repo,number,url) VALUES ('session','octocat/hello-world',1,'https://github.com/octocat/hello-world/pull/1')", []).unwrap();
        let result = import(&mut conn, scan).unwrap();
        assert!(result.is_partial());
        assert_eq!(prs_for_session(&conn, "session").unwrap().len(), 1);
    }

    #[test]
    fn paused_usage_reader_allows_competing_writer_and_rejects_superseded_generation() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("shared.db");
        let mut conn = crate::store::open_db(&db_path).unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n",
        )
        .unwrap();
        let mut session = t("s");
        session.path = path.to_string_lossy().into_owned();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut first = true;
            super::super::usage::READ_OBSERVER.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move |_| {
                    if first {
                        first = false;
                        ready_tx.send(()).unwrap();
                        resume_rx.recv().unwrap();
                    }
                }))
            });
            let result = import(
                &mut conn,
                Scan {
                    sessions: vec![session],
                    ..Default::default()
                },
            )
            .unwrap();
            super::super::usage::READ_OBSERVER.with(|hook| *hook.borrow_mut() = None);
            result
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let writer = crate::store::open_db(&db_path).unwrap();
        writer
            .busy_timeout(std::time::Duration::from_millis(250))
            .unwrap();
        let advanced = super::super::transcript::population_generation(&writer, "usage");
        resume_tx.send(()).unwrap();
        assert!(advanced.is_ok(), "reader held SQLite writer: {advanced:?}");
        assert!(worker.join().unwrap().is_partial());
        assert_eq!(
            writer
                .query_row(
                    "SELECT COUNT(*) FROM claude_usage_ledger WHERE fresh=1",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            writer
                .query_row("SELECT COUNT(*) FROM claude_session_usage", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn profile_shows_usage_progress_after_metadata_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("progress.db");
        let mut conn = crate::store::open_db(&db_path).unwrap();
        let reader = crate::store::open_db(&db_path).unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let mut session = t("s");
        session.path = path.to_string_lossy().into_owned();
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = observed.clone();
        super::super::usage::READ_OBSERVER.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_| {
                captured
                    .lock()
                    .unwrap()
                    .push(usage_profile(&reader).unwrap().population.unwrap());
            }));
        });
        let result = import(
            &mut conn,
            Scan {
                sessions: vec![session],
                ..Default::default()
            },
        );
        super::super::usage::READ_OBSERVER.with(|hook| *hook.borrow_mut() = None);
        result.unwrap();
        let observed = observed.lock().unwrap();
        assert!(!observed.is_empty());
        assert!(
            observed.iter().all(
                |p| p.phase == super::super::transcript::PopulationPhase::Usage
                    && p.status == PopulationStatus::Running
                    && p.total == 1
                    && p.completed == 0
            ),
            "{observed:?}"
        );
    }

    #[test]
    fn cancelled_usage_pass_restarts_from_accepted_file_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let mut sessions = Vec::new();
        for id in ["first", "second"] {
            let path = dir.path().join(format!("{id}.jsonl"));
            std::fs::write(
                &path,
                "{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n",
            )
            .unwrap();
            let mut session = t(id);
            session.path = path.to_string_lossy().into_owned();
            sessions.push(session);
        }
        let mut conn = db();
        let control = super::super::transcript::PopulationControl::default();
        let cancel = control.clone();
        let mut reads = 0;
        super::super::usage::READ_OBSERVER.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_| {
                reads += 1;
                if reads == 2 {
                    cancel.cancel();
                }
            }))
        });
        let scan = Scan {
            sessions,
            ..Default::default()
        };
        let result = usage_pass(&mut conn, &scan, &control).unwrap();
        super::super::usage::READ_OBSERVER.with(|hook| *hook.borrow_mut() = None);
        assert!(result.is_partial());
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM claude_usage_ledger WHERE fresh=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        let reads = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = reads.clone();
        super::super::usage::READ_OBSERVER.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_| observed.set(observed.get() + 1)))
        });
        usage_pass(
            &mut conn,
            &scan,
            &super::super::transcript::PopulationControl::default(),
        )
        .unwrap();
        super::super::usage::READ_OBSERVER.with(|hook| *hook.borrow_mut() = None);
        assert_eq!(reads.get(), 1, "restart reads only the uncommitted file");
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM claude_usage_ledger WHERE fresh=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
    }

    #[test]
    fn unchanged_oversized_usage_is_not_written_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.jsonl");
        let line = "{\"type\":\"assistant\",\"message\":{\"model\":\"test\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n";
        std::fs::write(
            &path,
            line.repeat((super::super::usage::BUDGET_BYTES as usize / line.len()) + 2),
        )
        .unwrap();
        let mut session = t("large");
        session.path = path.to_string_lossy().into_owned();
        let mut conn = db();
        let bytes = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let observed = bytes.clone();
        super::super::usage::READ_OBSERVER.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |n| observed.set(observed.get() + n)))
        });
        conn.execute_batch("CREATE TABLE usage_writes(n INTEGER); INSERT INTO usage_writes VALUES(0); CREATE TRIGGER count_usage AFTER INSERT ON claude_session_usage BEGIN UPDATE usage_writes SET n=n+1; END;").unwrap();
        import(
            &mut conn,
            Scan {
                sessions: vec![session.clone()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(bytes.get() as u64, std::fs::metadata(&path).unwrap().len());
        bytes.set(0);
        conn.execute("UPDATE usage_writes SET n=0", []).unwrap();
        import(
            &mut conn,
            Scan {
                sessions: vec![session.clone()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            conn.query_row("SELECT n FROM usage_writes", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(bytes.get(), 0, "unchanged import performs zero body reads");
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(line.as_bytes())
            .unwrap();
        import(
            &mut conn,
            Scan {
                sessions: vec![session],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            conn.query_row("SELECT n FROM usage_writes", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(bytes.get() as u64, std::fs::metadata(&path).unwrap().len());
        super::super::usage::READ_OBSERVER.with(|hook| *hook.borrow_mut() = None);
    }

    #[test]
    fn partial_discovery_preserves_links_and_attribution() {
        let mut conn = db();
        let mut child = t("child");
        child.cwd = Some(agent_cwd("cccccccc3333"));
        let mut map = crate::claude::subagent::Map::default();
        map_note(&mut map, "cccccccc3333", "parent", "2026-09-14T01:00:00Z");
        map.pr_links.push(crate::claude::subagent::PrLink {
            session_id: "parent".into(),
            repo: "octocat/hello-world".into(),
            number: 1,
            url: "https://github.com/octocat/hello-world/pull/1".into(),
            first_seen_at: None,
        });
        import(
            &mut conn,
            Scan {
                sessions: vec![t("parent"), child.clone()],
                subagents: map,
                ..Default::default()
            },
        )
        .unwrap();
        let mut failed_map = crate::claude::subagent::Map::default();
        failed_map.unreadable.push("failed map".into());
        for partial in [
            Scan {
                sessions: vec![child],
                unreadable_dirs: vec!["unknown subtree".into()],
                ..Default::default()
            },
            Scan {
                subagents: failed_map,
                ..Default::default()
            },
        ] {
            import(&mut conn, partial).unwrap();
            assert_eq!(prs_for_session(&conn, "parent").unwrap().len(), 1);
            let parent: String = conn
                .query_row(
                    "SELECT parent_session_id FROM claude_subagent WHERE session_id = 'child'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(parent, "parent");
        }
        import(&mut conn, Scan::default()).unwrap();
        assert!(prs_for_session(&conn, "parent").unwrap().is_empty());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM claude_subagent", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    fn t(id: &str) -> Transcript {
        Transcript {
            session_id: id.into(),
            path: format!("/Users/acme/.claude/projects/slug/{id}.jsonl"),
            cwd: Some("/Users/acme/code/widget".into()),
            git_branch: Some("feat/x".into()),
            claude_version: Some("2.1.270".into()),
            name: Some("Fix the retry backoff".into()),
            opening_prompt: Some("make the backoff jittered".into()),
            first_seen_at: Some("2026-09-01T10:00:00Z".into()),
            last_activity_at: Some("2026-09-01T11:30:00Z".into()),
            cwd_record: Some(5),
        }
    }

    fn one(
        conn: &Connection,
        id: &str,
    ) -> (Option<String>, Option<String>, String, Option<String>) {
        conn.query_row(
            "SELECT cwd, name, first_seen_at, last_activity_at
             FROM claude_session WHERE session_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    }

    /// A rescan is idempotent: the same corpus produces the same rows.
    ///
    /// This is the property that licenses #914's "no incremental
    /// machinery" decision. If a second import doubled the list, an
    /// offset or a dedupe pass would be mandatory.
    #[test]
    fn a_second_import_of_the_same_corpus_changes_nothing() {
        let mut conn = db();
        let scan = Scan {
            sessions: vec![t("s1"), t("s2")],
            ..Default::default()
        };
        let first = import(&mut conn, scan.clone()).unwrap();
        assert_eq!(first.sessions, 2);
        let second = import(&mut conn, scan).unwrap();
        assert_eq!(second.sessions, 2);

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2, "upsert on session_id, not insert");
    }

    /// The transcript wins on `cwd` -- the upstream-9188 fix.
    ///
    /// A hook can record a STALE cwd after `/exit` then `--continue`. The
    /// import must overwrite it from disk. Preferring the stored value
    /// would preserve the wrong directory forever, since nothing else
    /// revisits it.
    #[test]
    fn the_transcript_overwrites_a_stale_cwd() {
        let mut conn = db();
        conn.execute(
            "INSERT INTO claude_session
                (session_id, cwd, git_branch, claude_version, first_seen_at)
             VALUES ('s1', '/Users/acme/code/WRONG', 'stale-branch', '2.0.1',
                     '2026-09-01T10:00:00Z')",
            [],
        )
        .unwrap();

        import(
            &mut conn,
            Scan {
                sessions: vec![t("s1")],
                ..Default::default()
            },
        )
        .unwrap();

        let (cwd, _, _, _) = one(&conn, "s1");
        assert_eq!(
            cwd.as_deref(),
            Some("/Users/acme/code/widget"),
            "disk is ground truth; a stale hook cwd must not survive"
        );
        let (branch, version): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT git_branch, claude_version FROM claude_session WHERE session_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(branch.as_deref(), Some("feat/x"));
        assert_eq!(version.as_deref(), Some("2.1.270"));
    }

    /// A user's rename survives a rescan. An import never renames.
    #[test]
    fn an_import_never_overwrites_a_name() {
        let mut conn = db();
        conn.execute(
            "INSERT INTO claude_session (session_id, name, first_seen_at)
             VALUES ('s1', 'My own label', '2026-09-01T10:00:00Z')",
            [],
        )
        .unwrap();

        import(
            &mut conn,
            Scan {
                sessions: vec![t("s1")],
                ..Default::default()
            },
        )
        .unwrap();

        let (_, name, _, _) = one(&conn, "s1");
        assert_eq!(name.as_deref(), Some("My own label"));
    }

    /// A transcript with nothing to say still overwrites nothing.
    ///
    /// The sabotage direction of the rule above: if the COALESCE order
    /// were reversed for cwd, a `None` from an unparseable transcript
    /// would BLANK a good stored value. It must not.
    #[test]
    fn a_transcript_with_no_metadata_blanks_nothing() {
        let mut conn = db();
        conn.execute(
            "INSERT INTO claude_session (session_id, cwd, git_branch, first_seen_at)
             VALUES ('s1', '/Users/acme/code/known', 'main', '2026-09-01T10:00:00Z')",
            [],
        )
        .unwrap();

        let blank = Transcript {
            session_id: "s1".into(),
            path: "/x/s1.jsonl".into(),
            ..Default::default()
        };
        import(
            &mut conn,
            Scan {
                sessions: vec![blank],
                ..Default::default()
            },
        )
        .unwrap();

        let (cwd, _, _, _) = one(&conn, "s1");
        assert_eq!(
            cwd.as_deref(),
            Some("/Users/acme/code/known"),
            "absent is not a correction"
        );
    }

    /// The time range only ever widens.
    #[test]
    fn the_activity_range_widens_and_never_narrows() {
        let mut conn = db();
        let mut early = t("s1");
        early.first_seen_at = Some("2026-01-01T00:00:00Z".into());
        early.last_activity_at = Some("2026-01-02T00:00:00Z".into());
        import(
            &mut conn,
            Scan {
                sessions: vec![early],
                ..Default::default()
            },
        )
        .unwrap();

        let mut later = t("s1");
        later.first_seen_at = Some("2026-06-01T00:00:00Z".into());
        later.last_activity_at = Some("2026-06-02T00:00:00Z".into());
        import(
            &mut conn,
            Scan {
                sessions: vec![later],
                ..Default::default()
            },
        )
        .unwrap();

        let (_, _, first, last) = one(&conn, "s1");
        assert_eq!(first, "2026-01-01T00:00:00Z", "earliest wins");
        assert_eq!(last.as_deref(), Some("2026-06-02T00:00:00Z"), "latest wins");
    }

    /// A subagent session is recorded as one, with its parent (#1002).
    ///
    /// The end-to-end shape: a child whose `cwd` is an agent worktree, a
    /// parent transcript that mentions the agent id, and a
    /// `claude_subagent` row joining them.
    #[test]
    fn a_subagent_is_stored_with_the_session_that_spawned_it() {
        let mut conn = db();
        let mut child = t("child-1");
        child.cwd = Some(agent_cwd("aaaaaaaa1111"));
        let parent = t("parent-1");

        let mut map = crate::claude::subagent::Map::default();
        map_note(&mut map, "aaaaaaaa1111", "parent-1", "2026-09-14T01:00:00Z");

        let out = import(
            &mut conn,
            Scan {
                sessions: vec![parent, child],
                subagents: map,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(out.subagents, 1, "one session ran in an agent worktree");
        assert_eq!(out.subagents_attributed, 1);
        let (agent, parent_id, why): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT agent_id, parent_session_id, why FROM claude_subagent
                 WHERE session_id = 'child-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(agent, "aaaaaaaa1111");
        assert_eq!(parent_id.as_deref(), Some("parent-1"));
        assert!(why.is_none(), "an attributed child needs no excuse");
    }

    /// The happy-path pair: an ordinary session gets no subagent row.
    ///
    /// Without this the test above passes for a build that files EVERY
    /// session as a subagent, which would hide the entire list.
    #[test]
    fn an_ordinary_session_is_not_recorded_as_a_subagent() {
        let mut conn = db();
        let out = import(
            &mut conn,
            Scan {
                sessions: vec![t("s1")],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out.subagents, 0);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_subagent", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    /// An ambiguous child is stored UNATTRIBUTED, with the reason.
    ///
    /// #1002's central rule: where earliest-mention cannot decide, the
    /// child stays unattributed rather than being assigned a probable
    /// parent. A wrong rollup is worse than no rollup.
    #[test]
    fn an_ambiguous_child_is_stored_unattributed_with_its_reason() {
        let mut conn = db();
        let mut child = t("child-1");
        child.cwd = Some(agent_cwd("bbbbbbbb2222"));

        // Two candidate parents naming the agent at the SAME instant.
        let mut map = crate::claude::subagent::Map::default();
        map_note(&mut map, "bbbbbbbb2222", "cand-a", "2026-09-14T01:00:00Z");
        map_note(&mut map, "bbbbbbbb2222", "cand-b", "2026-09-14T01:00:00Z");

        let out = import(
            &mut conn,
            Scan {
                sessions: vec![t("cand-a"), t("cand-b"), child],
                subagents: map,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(out.subagents, 1);
        assert_eq!(
            out.subagents_attributed, 0,
            "a tie must not be resolved to a probable parent"
        );
        let (parent_id, why): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT parent_session_id, why FROM claude_subagent
                 WHERE session_id = 'child-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(parent_id.is_none(), "got {parent_id:?}");
        let why = why.expect("an unattributed child must say why");
        assert!(why.contains("cannot be told apart"), "{why}");
    }

    /// The map is rewritten WHOLE, so a stale attribution cannot survive.
    ///
    /// The conclusion is over the whole corpus and can change for a
    /// session nothing about which changed -- a parent's transcript being
    /// deleted, say. An upsert would leave yesterday's answer in place
    /// with nothing to remove it.
    #[test]
    fn a_rescan_replaces_the_attribution_rather_than_adding_to_it() {
        let mut conn = db();
        let mut child = t("child-1");
        child.cwd = Some(agent_cwd("cccccccc3333"));

        let mut first = crate::claude::subagent::Map::default();
        map_note(
            &mut first,
            "cccccccc3333",
            "parent-old",
            "2026-09-14T01:00:00Z",
        );
        import(
            &mut conn,
            Scan {
                sessions: vec![t("parent-old"), child.clone()],
                subagents: first,
                ..Default::default()
            },
        )
        .unwrap();

        // A second scan in which the old parent is gone and a new one
        // names the agent.
        let mut second = crate::claude::subagent::Map::default();
        map_note(
            &mut second,
            "cccccccc3333",
            "parent-new",
            "2026-09-14T02:00:00Z",
        );
        import(
            &mut conn,
            Scan {
                sessions: vec![t("parent-new"), child],
                subagents: second,
                ..Default::default()
            },
        )
        .unwrap();

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_subagent", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "the table is restated, never appended to");
        let parent_id: Option<String> = conn
            .query_row(
                "SELECT parent_session_id FROM claude_subagent WHERE session_id = 'child-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(parent_id.as_deref(), Some("parent-new"));
    }

    /// A child never adopts itself.
    ///
    /// A child's own transcript carries its own agent id and is always the
    /// earliest mention of it, so without the exclusion every subagent
    /// would be filed as its own parent.
    #[test]
    fn a_child_is_not_filed_as_its_own_parent() {
        let mut conn = db();
        let mut child = t("child-1");
        child.cwd = Some(agent_cwd("dddddddd4444"));

        let mut map = crate::claude::subagent::Map::default();
        // The child mentions it FIRST, the real parent later.
        map_note(&mut map, "dddddddd4444", "child-1", "2026-09-14T01:00:00Z");
        map_note(&mut map, "dddddddd4444", "parent-1", "2026-09-14T02:00:00Z");

        import(
            &mut conn,
            Scan {
                sessions: vec![t("parent-1"), child],
                subagents: map,
                ..Default::default()
            },
        )
        .unwrap();

        let parent_id: Option<String> = conn
            .query_row(
                "SELECT parent_session_id FROM claude_subagent WHERE session_id = 'child-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            parent_id.as_deref(),
            Some("parent-1"),
            "the child's own mention must not win"
        );
    }

    /// An unreadable transcript costs an ATTRIBUTION, not a session.
    ///
    /// So it travels in its own field and is deliberately kept out of
    /// `is_partial`, which drives "this LIST may be incomplete". Every row
    /// is still present; only the rollup is short.
    #[test]
    fn a_map_read_failure_does_not_claim_the_list_is_short() {
        let mut conn = db();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s1.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let mut session = t("s1");
        session.path = path.to_string_lossy().into_owned();
        let mut map = crate::claude::subagent::Map::default();
        map.unreadable.push("/x/y.jsonl: could not open it".into());
        let out = import(
            &mut conn,
            Scan {
                sessions: vec![session],
                subagents: map,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out.subagent_map_unreadable.len(), 1);
        assert!(
            !out.is_partial(),
            "an unreadable transcript costs an attribution, not a row"
        );
    }

    /// An agent worktree cwd, built with `join` so this is right on
    /// Windows -- `format!("{}/…")` has cost this repo six Windows-only
    /// failures.
    fn agent_cwd(agent: &str) -> String {
        std::path::Path::new("/Users/x/code/widget")
            .join(".claude")
            .join("worktrees")
            .join(format!("agent-{agent}"))
            .to_string_lossy()
            .into_owned()
    }

    /// Record one mention in a map, through the module's own builder so
    /// the tests exercise the real note/resolve path.
    fn map_note(map: &mut crate::claude::subagent::Map, agent: &str, session: &str, at: &str) {
        map.note_for_test(agent, session, at);
    }

    /// An import writes no runs, because it observed no process.
    ///
    /// Migration 11 makes `claude_run.pid` NOT NULL so an unobserved
    /// process cannot be recorded as observed. A transcript-sourced
    /// session therefore has zero runs, and the liveness layer reports
    /// that as unknown rather than dead.
    #[test]
    fn an_import_invents_no_run() {
        let mut conn = db();
        import(
            &mut conn,
            Scan {
                sessions: vec![t("s1")],
                ..Default::default()
            },
        )
        .unwrap();
        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 0);
    }

    /// A transcript with no timestamps still stores, and does not claim
    /// activity it cannot prove.
    #[test]
    fn an_undated_transcript_stores_without_inventing_activity() {
        let mut conn = db();
        let undated = Transcript {
            session_id: "s1".into(),
            path: "/x/s1.jsonl".into(),
            cwd: Some("/tmp/x".into()),
            ..Default::default()
        };
        let got = import(
            &mut conn,
            Scan {
                sessions: vec![undated],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(got.sessions, 1);
        let (_, _, first, last) = one(&conn, "s1");
        assert!(!first.is_empty(), "NOT NULL needs a value");
        assert_eq!(last, None, "no activity was observed, so none is claimed");
    }

    /// The unreadable counts reach the caller, not a log line.
    #[test]
    fn what_could_not_be_read_survives_the_storage_boundary() {
        let mut conn = db();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s1.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let mut session = t("s1");
        session.path = path.to_string_lossy().into_owned();
        let got = import(
            &mut conn,
            Scan {
                sessions: vec![session],
                subagent_files_skipped: 1370,
                unreadable_dirs: vec!["/x/secret: Permission denied".into()],
                unreadable_files: vec!["/x/a.jsonl: Permission denied".into()],
                metadata_beyond_first_record: 1,
                elapsed_ms: 42,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(got.is_partial());
        assert_eq!(got.subagent_files_skipped, 1370);
        assert_eq!(got.unreadable_dirs.len(), 1);
        assert_eq!(got.unreadable_files.len(), 1);
        assert_eq!(got.metadata_beyond_first_record, 1);
        assert_eq!(got.elapsed_ms, 42);
    }

    /// The truncated-session backfill (#1213).
    ///
    /// The bulk pass reads at most `BUDGET_BYTES` per transcript, which
    /// on a real machine leaves 14 of 1,453 sessions reporting a floor
    /// -- and those 14 hold 53.6% of all session bytes.
    mod backfill {
        use super::*;

        fn usage(messages: u64, truncated: bool) -> super::super::super::usage::Usage {
            super::super::super::usage::Usage {
                messages,
                input_tokens: 100,
                output_tokens: 500,
                cache_read_tokens: 10,
                cache_creation_tokens: 5,
                truncated,
                ..Default::default()
            }
        }

        /// A transcript on disk, so `summarise_whole` has something real
        /// to read. In its own `TempDir`, removed when the guard drops
        /// (#1554).
        struct Tmp(
            std::path::PathBuf,
            // Never read: held so the directory lives exactly as long as this.
            #[allow(dead_code)] tempfile::TempDir,
        );
        impl Tmp {
            fn new(name: &str, messages: usize) -> Self {
                let dir = tempfile::TempDir::new().unwrap();
                let p = dir.path().join(format!("headstate-backfill-{name}.jsonl"));
                let line = r#"{"type":"assistant","message":{"model":"claude-opus-5","usage":{"input_tokens":7,"output_tokens":11,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
                let body = std::iter::repeat_n(line, messages)
                    .collect::<Vec<_>>()
                    .join("\n");
                std::fs::write(&p, body).unwrap();
                Self(p, dir)
            }
        }

        #[test]
        fn only_truncated_rows_are_listed() {
            // THE bound. Listing everything would re-read the whole
            // corpus, which is the 3.8 s cost `BUDGET_BYTES` exists to
            // prevent -- the opposite of what this change is for.
            let conn = db();
            record_usage(&conn, "whole", &usage(10, false), "2026-09-01T00:00:00Z").unwrap();
            record_usage(&conn, "floor", &usage(20, true), "2026-09-01T00:00:00Z").unwrap();

            assert_eq!(
                truncated_sessions(&conn).unwrap(),
                vec!["floor".to_string()]
            );
        }

        #[test]
        fn nothing_to_do_is_an_empty_list_rather_than_an_error() {
            // The steady state after one backfill has run. It must be
            // cheap and silent, not a failure.
            let conn = db();
            record_usage(&conn, "whole", &usage(10, false), "2026-09-01T00:00:00Z").unwrap();
            assert!(truncated_sessions(&conn).unwrap().is_empty());
        }

        /// The whole point: a transcript LARGER than the budget must be
        /// read whole, not re-truncated.
        ///
        /// The earlier version of this test used a three-record fixture,
        /// which the capped reader also reads whole -- so swapping
        /// `summarise_whole` for `summarise` passed it. That is the one
        /// substitution this change exists to prevent, so the fixture
        /// has to exceed `BUDGET_BYTES`.
        ///
        /// 8 MB of writes is slower than a unit test should be, so the
        /// assertion is on `truncated` rather than on a token sum: the
        /// capped reader sets it, the whole reader clears it, and that
        /// difference is the entire contract.
        #[test]
        fn a_transcript_over_the_budget_is_read_whole() {
            let conn = db();
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("headstate-backfill-over-budget.jsonl");
            // One record, then padding past the cap. The padding lines
            // are not usage records, so they change no figure -- they
            // exist only to push the file past `BUDGET_BYTES`.
            let mut body = String::from(
                r#"{"type":"assistant","message":{"model":"claude-opus-5","usage":{"input_tokens":7,"output_tokens":11,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            );
            let filler = format!(r#"{{"type":"system","pad":"{}"}}"#, "x".repeat(4096));
            while body.len() as u64 <= super::super::super::usage::BUDGET_BYTES {
                body.push('\n');
                body.push_str(&filler);
            }
            std::fs::write(&p, &body).unwrap();

            record_usage(&conn, "s", &usage(1, true), "2026-09-01T00:00:00Z").unwrap();
            backfill_one(&conn, "s", &p, "2026-09-02T00:00:00Z").unwrap();

            let truncated: i64 = conn
                .query_row(
                    "SELECT truncated FROM claude_session_usage WHERE session_id = 's'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                truncated, 0,
                "a capped read would leave this at 1 -- the file is over BUDGET_BYTES"
            );
        }

        #[test]
        fn a_backfilled_session_stops_being_listed() {
            // It runs ONCE. A session re-measured whole must not come
            // back on the next pass, or the backfill never terminates.
            let conn = db();
            let f = Tmp::new("once", 3);
            record_usage(&conn, "s", &usage(1, true), "2026-09-01T00:00:00Z").unwrap();
            assert_eq!(truncated_sessions(&conn).unwrap().len(), 1);

            assert!(backfill_one(&conn, "s", &f.0, "2026-09-02T00:00:00Z").unwrap());
            assert!(
                truncated_sessions(&conn).unwrap().is_empty(),
                "a whole read must clear the floor"
            );
        }

        #[test]
        fn the_re_measured_figures_replace_the_floor() {
            // The point of the exercise: the stored numbers change to
            // the complete ones, not merely the flag.
            let conn = db();
            let f = Tmp::new("figures", 3);
            record_usage(&conn, "s", &usage(1, true), "2026-09-01T00:00:00Z").unwrap();

            backfill_one(&conn, "s", &f.0, "2026-09-02T00:00:00Z").unwrap();

            let (messages, truncated): (i64, i64) = conn
                .query_row(
                    "SELECT messages, truncated FROM claude_session_usage WHERE session_id = 's'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(messages, 3, "all three records, not the stored floor of 1");
            assert_eq!(truncated, 0);
        }

        #[test]
        fn a_vanished_transcript_leaves_the_previous_measurement_alone() {
            // Absent is not zero. A session whose file is temporarily
            // unreadable -- an unmounted volume, a pruned directory --
            // has not suddenly cost nothing, and writing zeros over its
            // last known figures would claim exactly that.
            let conn = db();
            record_usage(&conn, "s", &usage(42, true), "2026-09-01T00:00:00Z").unwrap();

            let gone = std::path::Path::new("/no/such/transcript.jsonl");
            assert!(
                !backfill_one(&conn, "s", gone, "2026-09-02T00:00:00Z").unwrap(),
                "an unreadable transcript reports that it did nothing"
            );

            let (messages, truncated): (i64, i64) = conn
                .query_row(
                    "SELECT messages, truncated FROM claude_session_usage WHERE session_id = 's'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(messages, 42, "the previous measurement survives");
            assert_eq!(truncated, 1, "and is still honestly marked a floor");
        }
    }

    /// An absent root survives the storage boundary WITHOUT making the
    /// import partial (#970).
    ///
    /// The path has to reach the frontend -- it is what lets the empty
    /// state say where Headstate looked -- and it must not arrive as
    /// evidence of a failed read, which is what `unreadable_dirs` was.
    #[test]
    fn an_absent_root_survives_the_boundary_without_claiming_a_failure() {
        let mut conn = db();
        let got = import(
            &mut conn,
            Scan {
                absent_root: Some("/Users/acme/.claude/projects".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(got.sessions, 0);
        assert_eq!(
            got.absent_root.as_deref(),
            Some("/Users/acme/.claude/projects")
        );
        assert!(
            !got.is_partial(),
            "a machine with no history has a complete list of nothing"
        );
    }
    /// #1132: both directions of the pull-request link.
    #[test]
    fn pull_request_links_round_trip_in_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();

        let link = |session: &str, n: u64| super::super::subagent::PrLink {
            session_id: session.to_string(),
            repo: "acme/api".to_string(),
            number: n,
            url: format!("https://github.com/acme/api/pull/{n}"),
            first_seen_at: Some("2026-09-11T12:00:00Z".to_string()),
        };

        let tx = conn.transaction().unwrap();
        for l in [link("s1", 7), link("s1", 8), link("s2", 7)] {
            tx.execute(
                "INSERT OR REPLACE INTO claude_session_pr
                     (session_id, repo, number, url, first_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    l.session_id,
                    l.repo,
                    l.number as i64,
                    l.url,
                    l.first_seen_at
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();

        // Forward: what did this session produce.
        let forward = prs_for_session(&conn, "s1").unwrap();
        assert_eq!(forward.len(), 2, "one session can produce several");

        // Reverse: which sessions produced this. The direction the PR
        // view asks, and the one the app could not answer at all. By
        // number since #1557; the caller picks the repository.
        let reverse = sessions_for_pr_number(&conn, 7).unwrap();
        assert_eq!(reverse.len(), 2);
        assert!(reverse.iter().any(|l| l.session_id == "s1"));
        assert!(reverse.iter().any(|l| l.session_id == "s2"));
    }

    /// #1545: a bare number is looked up in the link table itself, across
    /// every repository that carries it -- merged or not, tracked or not.
    #[test]
    fn a_number_alone_finds_its_sessions_in_every_repository() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
        for (session, repo, n) in [
            ("s1", "acme/api", 7),
            ("s2", "acme/ui", 7),
            ("s3", "acme/api", 8),
        ] {
            conn.execute(
                "INSERT OR REPLACE INTO claude_session_pr
                     (session_id, repo, number, url, first_seen_at)
                 VALUES (?1, ?2, ?3, 'u', '2026-09-11T12:00:00Z')",
                rusqlite::params![session, repo, n],
            )
            .unwrap();
        }

        let got = sessions_for_pr_number(&conn, 7).unwrap();
        let pairs: Vec<(&str, &str)> = got
            .iter()
            .map(|l| (l.session_id.as_str(), l.repo.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [("s1", "acme/api"), ("s2", "acme/ui")],
            "both repositories' #7, ordered by repository, and not #8"
        );
        assert!(
            sessions_for_pr_number(&conn, 9).unwrap().is_empty(),
            "a number no session linked is an empty answer, not an error"
        );
    }

    /// The primary key is the dedup rule made structural: a session
    /// re-links the same PR on every turn, and without this the table
    /// would grow without bound on every rescan.
    #[test]
    fn re_storing_one_link_does_not_duplicate_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
        for _ in 0..3 {
            conn.execute(
                "INSERT OR REPLACE INTO claude_session_pr
                     (session_id, repo, number, url, first_seen_at)
                 VALUES ('s1', 'acme/api', 7, 'u', '2026-09-11T12:00:00Z')",
                [],
            )
            .unwrap();
        }
        assert_eq!(prs_for_session(&conn, "s1").unwrap().len(), 1);
    }

    /// #1134: the aggregation, and the denominators that qualify it.
    #[test]
    fn usage_sums_across_sessions_with_its_denominators() {
        let conn = db();
        let u = |messages, out_tokens, truncated| super::super::usage::Usage {
            messages,
            input_tokens: 100,
            output_tokens: out_tokens,
            cache_read_tokens: 10,
            cache_creation_tokens: 5,
            models: vec![super::super::usage::ModelCount {
                model: "claude-opus-5".into(),
                messages,
            }],
            truncated,
            ..Default::default()
        };
        record_usage(&conn, "s1", &u(10, 500, false), "2026-09-01T00:00:00Z").unwrap();
        record_usage(&conn, "s2", &u(20, 700, true), "2026-09-01T00:00:00Z").unwrap();

        let p = usage_profile(&conn).unwrap();
        assert_eq!(p.output_tokens, 1200);
        assert_eq!(p.messages, 30);
        assert_eq!(
            p.sessions_measured, 2,
            "the denominator travels with the total"
        );
        assert_eq!(p.sessions_truncated, 1, "and so does what makes it a floor");
        assert!(
            p.partial(),
            "one truncated session makes the whole sum a floor"
        );
        assert_eq!(p.models[0].model, "claude-opus-5");
        assert_eq!(p.models[0].messages, 30, "models sum across sessions");
    }

    /// A profile over nothing is zeros with a zero denominator -- which
    /// is honest, and distinguishable from a real total by
    /// `sessions_measured`.
    #[test]
    fn an_empty_corpus_reports_a_zero_denominator() {
        let conn = db();
        let p = usage_profile(&conn).unwrap();
        assert_eq!(p.sessions_measured, 0);
        assert!(
            !p.partial(),
            "nothing measured is not a truncated measurement"
        );
    }

    /// A session whose cwd was never recorded is EXCLUDED from the
    /// directory breakdown rather than bucketed as empty: an unknown
    /// directory is not a directory.
    #[test]
    fn a_session_with_no_cwd_is_not_a_directory() {
        let mut conn = db();
        let t = |id: &str, cwd: Option<&str>| Transcript {
            // #1133 landed while this branch was open; the directory
            // breakdown does not depend on it.
            opening_prompt: None,
            session_id: id.into(),
            path: format!("/p/{id}.jsonl"),
            cwd: cwd.map(str::to_string),
            git_branch: None,
            claude_version: None,
            name: None,
            first_seen_at: Some("2026-09-01T10:00:00Z".into()),
            last_activity_at: Some("2026-09-01T11:00:00Z".into()),
            cwd_record: Some(3),
        };
        let tx = conn.transaction().unwrap();
        for s in [t("s1", Some("/code/widget")), t("s2", None)] {
            upsert(&tx, &s, "2026-09-01T00:00:00Z").unwrap();
        }
        tx.commit().unwrap();

        let u = super::super::usage::Usage {
            messages: 1,
            output_tokens: 100,
            ..Default::default()
        };
        record_usage(&conn, "s1", &u, "2026-09-01T00:00:00Z").unwrap();
        record_usage(&conn, "s2", &u, "2026-09-01T00:00:00Z").unwrap();

        let p = usage_profile(&conn).unwrap();
        assert_eq!(p.sessions_measured, 2, "both are measured");
        assert_eq!(
            p.by_directory.len(),
            1,
            "but only the one with a known directory is bucketed"
        );
        assert_eq!(p.by_directory[0].cwd, "/code/widget");
    }
}
