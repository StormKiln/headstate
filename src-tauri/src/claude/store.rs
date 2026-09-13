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
}

impl Imported {
    /// Whether anything could not be read or written.
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
             transcript_path, first_seen_at, last_activity_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(session_id) DO UPDATE SET
            cwd              = COALESCE(excluded.cwd, claude_session.cwd),
            git_branch       = COALESCE(excluded.git_branch, claude_session.git_branch),
            claude_version   = COALESCE(excluded.claude_version, claude_session.claude_version),
            transcript_path  = COALESCE(excluded.transcript_path, claude_session.transcript_path),
            name             = COALESCE(claude_session.name, excluded.name),
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
pub fn import(conn: &mut Connection, scan: Scan) -> Result<Imported, rusqlite::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut out = Imported {
        subagent_files_skipped: scan.subagent_files_skipped,
        unreadable_dirs: scan.unreadable_dirs,
        unreadable_files: scan.unreadable_files,
        metadata_beyond_first_record: scan.metadata_beyond_first_record,
        elapsed_ms: scan.elapsed_ms,
        ..Default::default()
    };

    let tx = conn.transaction()?;
    for t in &scan.sessions {
        match upsert(&tx, t, &now) {
            Ok(()) => out.sessions += 1,
            Err(e) => out
                .write_failures
                .push(format!("{}: could not store it: {e}", t.session_id)),
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
}
