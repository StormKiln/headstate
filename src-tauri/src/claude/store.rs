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

use rusqlite::Connection;

use super::subagent::{Kind, Parent};
use super::transcript::{Scan, Transcript};

/// What an import changed, and what it could not read.
///
/// The unreadable counts are carried through from the [`Scan`] rather
/// than dropped at the storage boundary, because the caller that renders
/// the list is the one that has to say "this may be incomplete". A count
/// that only reaches a log line is a count nobody sees.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Imported {
    /// Sessions written (inserted or updated).
    pub sessions: usize,
    /// Rows the database refused, with why. Counted, not swallowed: a
    /// scan that read 1,430 transcripts and stored 1,200 must say so.
    pub write_failures: Vec<String>,
    pub subagent_files_skipped: usize,
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
        !self.unreadable_dirs.is_empty()
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

/// Write a completed [`Scan`] into `claude_session`.
///
/// One transaction, so a rescan interrupted halfway leaves the previous
/// contents rather than a half-merged list. Per-session write failures
/// are collected and reported instead of aborting the import: one
/// malformed row must not cost the user the other 1,429.
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

/// The sessions that produced one pull request (#1132).
///
/// The reverse direction, and the one the PR view asks. Indexed on
/// `(repo, number)` so it does not scan the table.
pub fn sessions_for_pr(
    conn: &Connection,
    repo: &str,
    number: u64,
) -> Result<Vec<super::subagent::PrLink>, rusqlite::Error> {
    let mut q = conn.prepare(
        "SELECT session_id, repo, number, url, first_seen_at
           FROM claude_session_pr WHERE repo = ?1 AND number = ?2
          ORDER BY first_seen_at, session_id",
    )?;
    let rows = q.query_map(rusqlite::params![repo, number as i64], |r| {
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

pub fn import(conn: &mut Connection, scan: Scan) -> Result<Imported, rusqlite::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut out = Imported {
        subagent_files_skipped: scan.subagent_files_skipped,
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

    let tx = conn.transaction()?;
    for t in &scan.sessions {
        match upsert(&tx, t, &now) {
            Ok(()) => out.sessions += 1,
            Err(e) => out
                .write_failures
                .push(format!("{}: could not store it: {e}", t.session_id)),
        }
    }

    // The attribution, rewritten WHOLE rather than upserted (#1002).
    //
    // The map is one derived artefact over the whole corpus: a session
    // that was unattributed last scan can resolve on this one because
    // some OTHER transcript grew a mention, and one that was attributed
    // can stop being so if its parent's transcript is deleted. An upsert
    // would leave the stale conclusion in place with nothing to remove
    // it, so the table is cleared and re-stated by the pass that owns it.
    //
    // Inside the same transaction as the sessions, so a reader never sees
    // a list whose rows and whose attributions came from different scans.
    // The pull-request links, on the same terms and for the same reason
    // (#1132). They are derived from the corpus as a whole: a transcript
    // deleted since the last scan should take its links with it, and an
    // upsert would leave them behind with nothing to remove them.
    //
    // Inside this transaction so a reader never sees sessions and links
    // from different scans.
    tx.execute("DELETE FROM claude_session_pr", [])?;
    for l in &scan.subagents.pr_links {
        if let Err(e) = tx.execute(
            "INSERT OR REPLACE INTO claude_session_pr
                 (session_id, repo, number, url, first_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            // `as i64`: SQLite integers are signed, and a PR number is
            // never near the boundary -- GitHub's largest is six digits.
            rusqlite::params![
                l.session_id,
                l.repo,
                l.number as i64,
                l.url,
                l.first_seen_at
            ],
        ) {
            out.write_failures.push(format!(
                "{}: could not store its pull request link: {e}",
                l.session_id
            ));
        }
    }

    // Token usage, measured on the same pass (#1134).
    //
    // BOUNDED per session by `summarise`, not `summarise_whole`: the
    // budget is what keeps the whole-corpus read at 3.8 s, and a
    // truncated measurement is carried as a floor rather than dropped.
    // A session whose transcript is gone keeps whatever was measured
    // last -- absent is not zero.
    for t in &scan.sessions {
        // A transcript that could not be read leaves the PREVIOUS
        // measurement in place rather than writing zeros over it: absent
        // is not zero, and a session whose file is temporarily
        // unreadable has not suddenly cost nothing.
        let Ok(u) = super::usage::summarise(std::path::Path::new(&t.path)) else {
            continue;
        };
        if let Err(e) = record_usage(&tx, &t.session_id, &u, &now) {
            out.write_failures
                .push(format!("{}: could not store its usage: {e}", t.session_id));
        }
    }

    tx.execute("DELETE FROM claude_subagent", [])?;
    for t in &scan.sessions {
        let Kind::Subagent { agent_id } = Kind::classify(t.cwd.as_deref()) else {
            continue;
        };
        let siblings = own.get(&agent_id).cloned().unwrap_or_default();
        let (parent, why) = match scan.subagents.parent_of(&agent_id, &siblings) {
            Parent::Session { session_id } => (Some(session_id), None),
            Parent::Unattributed { why } => (None, Some(why)),
        };
        out.subagents += 1;
        if parent.is_some() {
            out.subagents_attributed += 1;
        }
        if let Err(e) = tx.execute(
            "INSERT INTO claude_subagent
                (session_id, agent_id, parent_session_id, why, resolved_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![t.session_id, agent_id, parent, why, now],
        ) {
            out.write_failures.push(format!(
                "{}: could not store which session spawned it: {e}",
                t.session_id
            ));
        }
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
        let mut map = crate::claude::subagent::Map::default();
        map.unreadable.push("/x/y.jsonl: could not open it".into());
        let out = import(
            &mut conn,
            Scan {
                sessions: vec![t("s1")],
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
        let got = import(
            &mut conn,
            Scan {
                sessions: vec![t("s1")],
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
        // view asks, and the one the app could not answer at all.
        let reverse = sessions_for_pr(&conn, "acme/api", 7).unwrap();
        assert_eq!(reverse.len(), 2);
        assert!(reverse.iter().any(|l| l.session_id == "s1"));
        assert!(reverse.iter().any(|l| l.session_id == "s2"));
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
