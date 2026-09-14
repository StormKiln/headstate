//! Recording what the registry sweep found (#913).
//!
//! [`super::registry`] classifies each `~/.claude/sessions/<pid>.json`
//! file; this writes the classification into `claude_session` and
//! `claude_run`.
//!
//! # Why an orphan is recorded as a crash and not as a missing end record
//!
//! MEASURED (epic #910): a SIGKILLed session leaves its registry file
//! behind, still present twenty seconds later, carrying `sessionId`,
//! `procStart`, `cwd`, and a `status: "busy"` frozen at the instant of
//! death.
//!
//! So this is strictly better evidence than the design #913 was written
//! against. Inferring a crash from a missing `SessionEnd` is the weak
//! form, because absence is ALSO what a running session looks like:
//! a run with no `ended_at` is either live or dead and the row cannot say
//! which. An orphan file is a positive fact -- a process existed, it is
//! gone, and nothing cleaned up after it -- and it covers the case the
//! handoff file structurally cannot: a session that died before ever
//! appending an end record, which is every crash.
//!
//! # `end_reason` says which of the two it was
//!
//! `"crashed"` is written by this module only, and nothing else writes
//! it: the hook's own vocabulary is
//! `clear|resume|logout|prompt_input_exit|other`, all of which mean a
//! clean exit ran. So the column distinguishes "ended, and told us why"
//! from "ended, and we found out by looking", and the Resume button in
//! #918 keys on the second.
//!
//! # An `ended_at` we did not witness
//!
//! We know the session is over; we do not know WHEN. The file's mtime is
//! the closest thing available and it is not good enough to store as an
//! end time: the owner rewrites the file every few seconds while running,
//! so its mtime is the last heartbeat before death, which is up to that
//! interval early -- and after a reboot it is whatever the filesystem
//! preserved.
//!
//! So `ended_at` is the moment we OBSERVED the crash, which is a recorded
//! fact about our own observation rather than a claim about the session,
//! and it is set ONCE: `COALESCE` keeps the first observation, so a
//! second sweep of the same orphan does not walk the time forward every
//! tick. Without that, an orphan left on disk would appear to have
//! crashed a few seconds ago, forever.

use std::collections::HashMap;

use rusqlite::Connection;

use super::registry::{Live, Sweep};

/// The `end_reason` this module writes, and nothing else does.
///
/// Distinct from every value the hook can write
/// (`clear|resume|logout|prompt_input_exit|other`), all of which mean a
/// clean exit ran and none of which describe a session we found dead.
pub const CRASHED: &str = "crashed";

/// What a sweep changed.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Recorded {
    /// Sessions seen running, with their pid and confirmed start time.
    pub running: usize,
    /// Orphans recorded as crashed on THIS sweep -- first observations
    /// only, so a stale orphan is not re-counted every tick.
    pub crashed: usize,
    /// Orphans we had already recorded. Reported separately so a caller
    /// can tell "three new crashes" from "the same three orphans are
    /// still on disk".
    pub crashed_already_known: usize,
    /// WHICH sessions were recorded as crashed on this sweep (#979).
    ///
    /// `crashed` is the count and this is the identity, and a notifier
    /// needs both: "1 crash" is a number, and "claude in ~/code/ghstat
    /// died" is something a user can act on. One entry per increment of
    /// `crashed`, so the two cannot disagree.
    ///
    /// Carried on `Recorded` rather than re-queried by the notifier
    /// because the FIRST-observation property lives here: `record_crashed`
    /// is what knows whether our observation is the one that stuck, and a
    /// notifier that re-read the table afterwards could not tell a crash
    /// found this tick from one found an hour ago. That is the
    /// re-fires-every-minute bug `crashed_already_known` exists to
    /// prevent.
    pub crashed_sessions: Vec<Crashed>,
    /// Records whose liveness could not be determined -- the pid is in
    /// use but `procStart` would not confirm it is ours. Neither running
    /// nor crashed, per migration 11.
    pub unknown: usize,
    /// Records with no `session_id`, which cannot key a row.
    pub without_session_id: usize,
    /// Rows the database refused, with why.
    pub write_failures: Vec<String>,
    /// Registry files that could not be read, carried through from the
    /// sweep rather than dropped at the storage boundary.
    pub unreadable: Vec<String>,
}

impl Recorded {
    pub fn is_partial(&self) -> bool {
        !self.write_failures.is_empty() || !self.unreadable.is_empty()
    }
}

/// One session this sweep newly recorded as crashed (#979).
///
/// Enough to NAME it in a notification and nothing more. No liveness, no
/// stored status: liveness stays derived per read, which is migration
/// 11's central correction, and a struct carrying a status field here
/// would be the first place to store one.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Crashed {
    /// The `claude --resume` handle, and the only field guaranteed
    /// present -- a record without one cannot key a row and never reaches
    /// here (`without_session_id` counts those instead).
    pub session_id: String,
    /// The registry's own `name`, which exists in NO other source. `None`
    /// for a record that carried none: a notification headed by a raw
    /// UUID is still better than one headed by a fabricated name, and the
    /// caller decides which to show.
    pub name: Option<String>,
    /// Where it was running. `None` when the record carried none.
    pub cwd: Option<String>,
}

/// `pid_start_time` as it goes into the column: RFC 3339, or NULL.
///
/// NULL is migration 11's "cannot confirm", which liveness must report as
/// Unknown and never as Running.
fn pid_start(rec: &Live) -> Option<String> {
    rec.pid_start_epoch
        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
        .map(|d| d.to_rfc3339())
}

/// The session row for a registry record.
///
/// Fills NULLs only, exactly as the handoff consumer does and for the
/// same reason: the transcript importer (#914) owns `cwd` and
/// `claude_version` because disk is ground truth. `name` is the one field
/// this source contributes that no other has -- the hook payload carries
/// no name and the transcript has `aiTitle` instead -- and it still only
/// fills a NULL, so a user's own rename (#922) survives.
fn upsert_session(conn: &Connection, rec: &Live, now: &str) -> Result<(), rusqlite::Error> {
    let session_id = rec.session_id.as_deref().expect("checked by the caller");
    conn.execute(
        "INSERT INTO claude_session
            (session_id, name, cwd, claude_version, first_seen_at, last_activity_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL)
         ON CONFLICT(session_id) DO UPDATE SET
            name           = COALESCE(claude_session.name, excluded.name),
            cwd            = COALESCE(claude_session.cwd, excluded.cwd),
            claude_version = COALESCE(claude_session.claude_version, excluded.claude_version),
            first_seen_at  = MIN(claude_session.first_seen_at, excluded.first_seen_at)",
        rusqlite::params![
            session_id,
            rec.name,
            rec.cwd,
            rec.claude_version,
            // `first_seen_at` is NOT NULL. A registry file says the
            // session was alive at least as recently as now, and the
            // MIN above means a real earlier time from another source
            // still wins -- so this can only ever be an upper bound that
            // gets corrected, never a claim that displaces a fact.
            rec.pid_start_epoch
                .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
                .map(|d| d.to_rfc3339())
                .unwrap_or_else(|| now.to_owned()),
        ],
    )?;
    Ok(())
}

/// Record a running session's run.
///
/// `started_at` is the process's own start time when we could confirm it,
/// which makes the row addressable by the same key the handoff consumer
/// uses and therefore idempotent across sweeps: the same live process
/// yields the same `(session_id, pid, started_at)` every tick.
///
/// When we could NOT confirm it there is no stable key available, so the
/// run is not inserted at all -- a sweep every 60 seconds would otherwise
/// write a new row per tick for one session. The session row is still
/// written, so nothing is lost except a run we could not identify.
fn record_running(conn: &Connection, rec: &Live) -> Result<bool, rusqlite::Error> {
    // `pid_start` is already `None` for a missing or unparseable
    // `procStart`, so this one check covers both: no confirmed start
    // time means no stable key, which means no run.
    let Some(started) = pid_start(rec) else {
        return Ok(false);
    };
    conn.execute(
        // No `source`: the registry does not record why a session
        // started, and only the hook knows. A row inserted here leaves
        // it NULL for the hook's record to fill.
        "INSERT INTO claude_run
            (session_id, pid, pid_start_time, source, started_at)
         VALUES (?1, ?2, ?3, NULL, ?3)
         ON CONFLICT(session_id, pid, started_at) DO UPDATE SET
            pid_start_time = COALESCE(claude_run.pid_start_time, excluded.pid_start_time)",
        rusqlite::params![
            rec.session_id.as_deref().expect("checked by the caller"),
            rec.pid,
            started
        ],
    )?;
    Ok(true)
}

/// Record an orphan as a crashed run.
///
/// Returns whether this was a FIRST observation. A sweep runs on a timer,
/// so an orphan left on disk is seen again every tick; counting it once
/// is what lets a caller say "one new crash" rather than "one crash,
/// again".
///
/// # Why the UPDATE only ever fills
///
/// `ended_at = COALESCE(claude_run.ended_at, ?)` keeps the FIRST
/// observation. Overwriting it would walk the crash time forward on every
/// tick, so an orphan from last week would read as having crashed
/// seconds ago -- a stored value refreshed into a lie, which is the
/// defect class migration 11 was written against.
///
/// `end_reason` is filled the same way, so a session that ended cleanly
/// (the hook wrote `prompt_input_exit`) and whose registry file was left
/// behind anyway is not relabelled as a crash. That ordering matters:
/// the hook's reason is a first-hand report and ours is an inference.
///
/// # The key when `procStart` will not parse
///
/// `started_at` is part of the primary key, so the value chosen decides
/// whether repeated sweeps of one orphan address one row or many. With a
/// confirmable start time that is easy: the process's own start time,
/// which is also the key the handoff consumer would have used, so a crash
/// lands ON the existing run rather than beside it.
///
/// Without one there is no process-derived key, and the obvious fallback
/// -- the observation time -- is WRONG, which was found by the test rather
/// than by foresight: `now` differs on every tick, so a 60-second sweep
/// minted a fresh row and a fresh "new crash" every minute for one
/// session that crashed once. **Measured: four sweeps produced four
/// rows.**
///
/// So the fallback reuses the `started_at` of a crashed run already
/// recorded for this `(session_id, pid)`, and only mints an observation
/// time when there is none. That makes the second sweep address the first
/// sweep's row, which is the property the primary key cannot give us on
/// its own here.
fn record_crashed(conn: &Connection, rec: &Live, now: &str) -> Result<bool, rusqlite::Error> {
    let session_id = rec.session_id.as_deref().expect("checked by the caller");
    let started = match pid_start(rec) {
        Some(s) => s,
        None => conn
            .query_row(
                // The run WE recorded for this pid, if any. Narrowed to
                // our own `end_reason` so this cannot adopt the key of a
                // cleanly-ended run the hook recorded and then relabel
                // it -- that row is found by the INSERT's conflict clause
                // when the start times genuinely match, and must not be
                // reached by a fallback that guesses.
                "SELECT started_at FROM claude_run
                  WHERE session_id = ?1 AND pid = ?2 AND end_reason = ?3
                  ORDER BY started_at
                  LIMIT 1",
                rusqlite::params![session_id, rec.pid, CRASHED],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_else(|_| now.to_owned()),
    };
    conn.execute(
        "INSERT INTO claude_run
            (session_id, pid, pid_start_time, source, end_reason,
             started_at, ended_at)
         VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)
         ON CONFLICT(session_id, pid, started_at) DO UPDATE SET
            pid_start_time = COALESCE(claude_run.pid_start_time, excluded.pid_start_time),
            end_reason     = COALESCE(claude_run.end_reason, excluded.end_reason),
            ended_at       = COALESCE(claude_run.ended_at, excluded.ended_at)",
        rusqlite::params![session_id, rec.pid, pid_start(rec), CRASHED, started, now],
    )?;

    // Whether OUR observation is the one that stuck. A run the hook had
    // already closed keeps its own reason, and is not a new crash.
    let is_new: bool = conn.query_row(
        "SELECT end_reason = ?4 AND ended_at = ?5
           FROM claude_run
          WHERE session_id = ?1 AND pid = ?2 AND started_at = ?3",
        rusqlite::params![session_id, rec.pid, started, CRASHED, now],
        |r| r.get(0),
    )?;
    Ok(is_new)
}

/// Write a whole sweep.
///
/// One transaction, so a sweep interrupted halfway leaves the previous
/// contents rather than a half-recorded list. Per-record failures are
/// collected and reported instead of aborting: one malformed record must
/// not cost the user the other three.
pub fn record(conn: &mut Connection, sweep: &Sweep) -> Result<Recorded, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut out = Recorded {
        unreadable: sweep.unreadable.clone(),
        ..Default::default()
    };

    let tx = conn
        .transaction()
        .map_err(|e| format!("could not begin a transaction: {e}"))?;

    // Running and unknown first, so a session that appears in both this
    // sweep's running list and (impossibly) its orphan list cannot have
    // the crash overwritten -- and so the counts below describe the same
    // ordering every time.
    for (records, counter, crashed) in [
        (&sweep.running, &mut out.running, false),
        (&sweep.unknown, &mut out.unknown, false),
        (&sweep.orphaned, &mut out.crashed, true),
    ] {
        for rec in records {
            if rec.session_id.is_none() {
                // No id, no identity, no row. Counted rather than
                // dropped: it means a registry file exists for a session
                // we cannot name, which is worth someone seeing.
                out.without_session_id += 1;
                continue;
            }
            if let Err(e) = upsert_session(&tx, rec, &now) {
                out.write_failures
                    .push(format!("pid {}: could not store the session: {e}", rec.pid));
                continue;
            }
            let wrote = if crashed {
                record_crashed(&tx, rec, &now)
            } else {
                // An `unknown` record's liveness is unconfirmed, so no run
                // is written for it -- a run row is a claim that we
                // observed a process, and we did not. `record_running`
                // declines on a missing start time for the same reason.
                record_running(&tx, rec)
            };
            match wrote {
                Ok(true) => {
                    *counter += 1;
                    // The identity, beside the count, for #979's
                    // notifier. Recorded on the SAME arm that increments
                    // `crashed`, so the list and the count cannot drift
                    // -- a notifier reading a list assembled anywhere
                    // else could fire for an orphan already announced,
                    // which is the every-tick-forever bug
                    // `crashed_already_known` exists to prevent.
                    if crashed {
                        out.crashed_sessions.push(Crashed {
                            session_id: rec
                                .session_id
                                .clone()
                                .expect("checked at the top of this loop"),
                            name: rec.name.clone(),
                            cwd: rec.cwd.clone(),
                        });
                    }
                }
                Ok(false) if crashed => out.crashed_already_known += 1,
                Ok(false) => *counter += 1,
                Err(e) => out
                    .write_failures
                    .push(format!("pid {}: could not store the run: {e}", rec.pid)),
            }
        }
    }
    tx.commit()
        .map_err(|e| format!("could not commit the sweep: {e}"))?;
    Ok(out)
}

/// The confirmed start times a sweep found, keyed by pid.
///
/// This is what the handoff consumer takes for its `pid_start_time`: the
/// hook cannot afford to `sysctl` its own parent inside a 1.5 second
/// budget, and this side has the answer from a file read it is already
/// doing. A pid absent from this map leaves the column NULL, which is
/// migration 11's "cannot confirm".
///
/// Built from the RUNNING records only. An orphan's `procStart` describes
/// a process that no longer exists, so handing it to the consumer would
/// let a pid that has since been recycled inherit a start time from a
/// dead session and read as confirmed.
pub fn start_times(sweep: &Sweep) -> HashMap<u32, i64> {
    sweep
        .running
        .iter()
        .filter_map(|r| r.pid_start_epoch.map(|s| (r.pid, s)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    fn live(pid: u32, sid: Option<&str>, epoch: Option<i64>) -> Live {
        Live {
            pid,
            session_id: sid.map(str::to_owned),
            proc_start_raw: epoch.map(|_| "Fri Sep 11 09:43:48 2026".to_owned()),
            pid_start_epoch: epoch,
            cwd: Some("/Users/acme/code/widget".into()),
            name: Some(format!("widget-{pid}")),
            claude_version: Some("2.1.268".into()),
            status: Some("busy".into()),
            path: PathBuf::from(format!("/Users/acme/.claude/sessions/{pid}.json")),
        }
    }

    fn runs(conn: &Connection) -> Vec<(String, u32, Option<String>, Option<String>)> {
        conn.prepare(
            "SELECT session_id, pid, end_reason, pid_start_time
               FROM claude_run ORDER BY session_id, pid",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    /// THE CRASH RECORD: an orphan becomes a run ended by crash.
    ///
    /// The measured finding this whole module exists for (epic #910): a
    /// SIGKILLed session leaves its registry file behind, so a file whose
    /// pid is gone is positive evidence of a crash rather than the
    /// absence-of-a-SessionEnd inference the issue was written against.
    #[test]
    fn an_orphan_is_recorded_as_a_crash() {
        let mut conn = db();
        let sweep = Sweep {
            orphaned: vec![live(80043, Some("s-crashed"), Some(1_789_119_828))],
            ..Default::default()
        };

        let got = record(&mut conn, &sweep).unwrap();
        assert_eq!(got.crashed, 1);
        assert_eq!(got.crashed_already_known, 0);

        let rows = runs(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "s-crashed");
        assert_eq!(rows[0].1, 80043);
        assert_eq!(
            rows[0].2.as_deref(),
            Some(CRASHED),
            "and it is distinguishable from every clean-exit reason the \
             hook can write"
        );
        let ended: Option<String> = conn
            .query_row("SELECT ended_at FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert!(ended.is_some(), "a crashed run is an ENDED run");
    }

    /// The identities beside the count, for #979's notifier.
    ///
    /// `crashed` is a number and a notification needs a name: "1 crash"
    /// is not something a user can act on and "widget-80043 in
    /// /Users/acme/code/widget died" is. The registry's `name` and `cwd`
    /// exist in no other source, which is why they are carried rather
    /// than re-queried.
    ///
    /// Sabotage: move the `crashed_sessions.push` out of the `Ok(true)`
    /// arm -- to the top of the loop, say -- and
    /// `only_a_new_crash_is_named` below fails.
    #[test]
    fn a_new_crash_is_named_as_well_as_counted() {
        let mut conn = db();
        let sweep = Sweep {
            orphaned: vec![live(80043, Some("s-crashed"), Some(1_789_119_828))],
            ..Default::default()
        };

        let got = record(&mut conn, &sweep).unwrap();
        assert_eq!(got.crashed, 1);
        assert_eq!(
            got.crashed_sessions,
            vec![Crashed {
                session_id: "s-crashed".into(),
                name: Some("widget-80043".into()),
                cwd: Some("/Users/acme/code/widget".into()),
            }],
            "the count and the identity must describe the same crash"
        );
    }

    /// **The notifier's whole correctness condition**: a second sweep of
    /// the same orphan names nobody.
    ///
    /// A sweep runs every 60 seconds and an orphan left on disk is seen
    /// on every one of them. A notifier reading a list that included
    /// already-known orphans would announce the same dead session once a
    /// minute, forever -- which is the failure `crashed_already_known`
    /// was split out to prevent, and `record_crashed`'s doc records the
    /// four-sweeps-four-rows bug that found it.
    ///
    /// Sabotage: push the identity before `record_crashed` decides, or on
    /// the `Ok(false)` arm, and this fails on the second sweep.
    #[test]
    fn only_a_new_crash_is_named() {
        let mut conn = db();
        let sweep = Sweep {
            orphaned: vec![live(80043, Some("s-crashed"), Some(1_789_119_828))],
            ..Default::default()
        };

        let first = record(&mut conn, &sweep).unwrap();
        assert_eq!(first.crashed_sessions.len(), 1);

        let second = record(&mut conn, &sweep).unwrap();
        assert_eq!(second.crashed, 0);
        assert_eq!(second.crashed_already_known, 1, "still reported as known");
        assert!(
            second.crashed_sessions.is_empty(),
            "a notifier reading this list must not announce the same dead \
             session every minute forever"
        );
    }

    /// A running session names nobody. The happy-path pair: the quiet
    /// case must stay quiet, or a notifier wired to this list would fire
    /// on every tick of a healthy machine.
    #[test]
    fn a_healthy_sweep_names_nobody() {
        let mut conn = db();
        let sweep = Sweep {
            running: vec![live(80044, Some("s-running"), Some(1_789_119_828))],
            ..Default::default()
        };

        let got = record(&mut conn, &sweep).unwrap();
        assert_eq!(got.running, 1);
        assert_eq!(got.crashed, 0);
        assert!(got.crashed_sessions.is_empty());
    }

    /// A run the HOOK already closed is not a new crash and is not named.
    ///
    /// The hook's reason is a first-hand report and ours is an inference,
    /// so a registry file left behind after a clean exit must not produce
    /// a "your session died" notification about a session that ended on
    /// purpose.
    #[test]
    fn a_cleanly_ended_run_is_not_named_as_a_crash() {
        let mut conn = db();
        conn.execute(
            "INSERT INTO claude_session (session_id, first_seen_at)
             VALUES ('s1', '2026-09-11T09:43:48+00:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO claude_run
                (session_id, pid, started_at, end_reason, ended_at)
             VALUES ('s1', 80043, '2026-09-11T09:43:48+00:00',
                     'prompt_input_exit', '2026-09-11T10:00:00+00:00')",
            [],
        )
        .unwrap();

        let sweep = Sweep {
            orphaned: vec![live(80043, Some("s1"), Some(1_789_119_828))],
            ..Default::default()
        };
        let got = record(&mut conn, &sweep).unwrap();
        assert!(
            got.crashed_sessions.is_empty(),
            "a session that ended on purpose must not be announced as dead"
        );
    }

    /// A record with no `session_id` names nobody, because it cannot key
    /// a row and never reaches the crash path at all. `Crashed` requires
    /// a `session_id` for exactly this reason -- an `Option` there would
    /// have made a notification headed by nothing representable.
    #[test]
    fn a_record_without_a_session_id_names_nobody() {
        let mut conn = db();
        let sweep = Sweep {
            orphaned: vec![live(80043, None, Some(1_789_119_828))],
            ..Default::default()
        };
        let got = record(&mut conn, &sweep).unwrap();
        assert_eq!(got.without_session_id, 1);
        assert_eq!(got.crashed, 0);
        assert!(got.crashed_sessions.is_empty());
    }

    /// A SECOND sweep of the SAME orphan does not re-count it, and does
    /// not move the crash time.
    ///
    /// A sweep runs on a timer, so an orphan left on disk is seen every
    /// tick. Overwriting `ended_at` would make a week-old orphan read as
    /// having crashed seconds ago -- forever -- which is a stored value
    /// refreshed into a lie.
    ///
    /// Sabotage: replacing `ended_at = COALESCE(claude_run.ended_at, ...)`
    /// with `excluded.ended_at` fails this on both assertions.
    #[test]
    fn the_same_orphan_is_not_a_new_crash_every_tick() {
        let mut conn = db();
        let sweep = Sweep {
            orphaned: vec![live(80043, Some("s-crashed"), Some(1_789_119_828))],
            ..Default::default()
        };

        let first = record(&mut conn, &sweep).unwrap();
        assert_eq!(first.crashed, 1);
        let when: String = conn
            .query_row("SELECT ended_at FROM claude_run", [], |r| r.get(0))
            .unwrap();

        let second = record(&mut conn, &sweep).unwrap();
        assert_eq!(second.crashed, 0, "not a NEW crash");
        assert_eq!(second.crashed_already_known, 1, "but still reported");

        let still: String = conn
            .query_row("SELECT ended_at FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            still, when,
            "the first observation is the one that stands; walking it \
             forward would make every old orphan look brand new"
        );
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "one orphan, one row");
    }

    /// A run the HOOK already closed keeps the hook's reason.
    ///
    /// The hook's `prompt_input_exit` is a first-hand report; our
    /// "crashed" is an inference from a file left behind. A registry file
    /// that survived a clean exit must not relabel it.
    ///
    /// Sabotage: `end_reason = excluded.end_reason` overwrites the hook's
    /// reason and this fails.
    #[test]
    fn a_cleanly_ended_run_is_not_relabelled_as_a_crash() {
        let mut conn = db();
        conn.execute(
            "INSERT INTO claude_session (session_id, first_seen_at)
             VALUES ('s1', '2026-09-11T09:43:48+00:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO claude_run
                (session_id, pid, pid_start_time, end_reason, started_at, ended_at)
             VALUES ('s1', 500, '2026-09-11T09:43:48+00:00', 'prompt_input_exit',
                     '2026-09-11T09:43:48+00:00', '2026-09-11T10:00:00+00:00')",
            [],
        )
        .unwrap();

        let got = record(
            &mut conn,
            &Sweep {
                orphaned: vec![live(500, Some("s1"), Some(1_789_119_828))],
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(got.crashed, 0, "a clean exit is not a crash");
        assert_eq!(got.crashed_already_known, 1);
        let reason: String = conn
            .query_row("SELECT end_reason FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            reason, "prompt_input_exit",
            "the hook's first-hand report outranks our inference"
        );
    }

    /// A RUNNING session is recorded with its confirmed start time and NO
    /// end.
    #[test]
    fn a_running_session_is_an_open_run_with_a_confirmed_start() {
        let mut conn = db();
        let got = record(
            &mut conn,
            &Sweep {
                running: vec![live(14779, Some("s-live"), Some(1_789_119_828))],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(got.running, 1);
        assert_eq!(got.crashed, 0);

        let rows = runs(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].3.as_deref(),
            Some("2026-09-11T09:43:48+00:00"),
            "the pid-reuse guard is stored, not inferred later"
        );
        assert_eq!(rows[0].2, None, "a running run has no end reason");
        let ended: Option<String> = conn
            .query_row("SELECT ended_at FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ended, None);
    }

    /// Sweeping a running session repeatedly writes ONE row.
    ///
    /// The key is `(session_id, pid, started_at)` and `started_at` is the
    /// process's own start time, so the same live process addresses the
    /// same row on every tick. Without that, a 60-second sweep would add
    /// 1,440 rows a day for one session.
    ///
    /// Sabotage: using `now` as `started_at` for a running record makes
    /// this fail with 3 rows.
    #[test]
    fn sweeping_a_running_session_repeatedly_writes_one_row() {
        let mut conn = db();
        let sweep = Sweep {
            running: vec![live(14779, Some("s-live"), Some(1_789_119_828))],
            ..Default::default()
        };
        for _ in 0..3 {
            record(&mut conn, &sweep).unwrap();
        }
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    /// An UNKNOWN record writes a session row but NO run.
    ///
    /// A run row is a claim that we observed a process. We did not: the
    /// pid is in use and `procStart` could not confirm it is ours.
    /// Writing a run would turn "cannot confirm" into a recorded
    /// observation, which is the fail-open migration 11 forbids.
    ///
    /// Sabotage: routing `unknown` through `record_crashed` fails this
    /// with a crashed row for a session that may well be running.
    #[test]
    fn an_unconfirmable_record_writes_no_run() {
        let mut conn = db();
        let got = record(
            &mut conn,
            &Sweep {
                unknown: vec![live(14779, Some("s-maybe"), None)],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(got.unknown, 1);
        assert_eq!(got.crashed, 0);

        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 1, "the session is still worth knowing about");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            n, 0,
            "but no run: a run row is a claim we observed a process, and \
             we could not confirm we did"
        );
    }

    /// An orphan with no confirmable start time is STILL recorded as a
    /// crash, with a NULL `pid_start_time`.
    ///
    /// The pid is gone, so there is nothing the missing start time could
    /// have disambiguated -- and the crash is the fact worth keeping.
    /// NULL is migration 11's "cannot confirm", which the liveness layer
    /// reports as Unknown; the run's own `ended_at` is what says it is
    /// over.
    #[test]
    fn an_orphan_with_no_start_time_is_still_a_crash_with_a_null_guard() {
        let mut conn = db();
        let got = record(
            &mut conn,
            &Sweep {
                orphaned: vec![live(80043, Some("s-crashed"), None)],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(got.crashed, 1);
        let rows = runs(&conn);
        assert_eq!(rows[0].2.as_deref(), Some(CRASHED));
        assert_eq!(rows[0].3, None, "cannot confirm is NULL, not a guess");
    }

    /// An orphan with NO confirmable start time is still only ONE crash,
    /// however often it is swept.
    ///
    /// This is the case the single-sweep test above cannot see, and it is
    /// the one that matters operationally: a registry file whose
    /// `procStart` will not parse has no process-derived key, so a naive
    /// fallback to "now" would mint a new `started_at` on every tick. On
    /// a 60-second sweep that is 1,440 rows a day, each counted as a
    /// fresh crash, for one session that crashed once.
    ///
    /// The fix is that the fallback key is derived from the RECORD, not
    /// from the clock -- see [`record_crashed`].
    #[test]
    fn an_orphan_with_no_start_time_is_still_only_one_crash() {
        let mut conn = db();
        let sweep = Sweep {
            orphaned: vec![live(80043, Some("s-crashed"), None)],
            ..Default::default()
        };

        let first = record(&mut conn, &sweep).unwrap();
        assert_eq!(first.crashed, 1);
        let when: String = conn
            .query_row("SELECT ended_at FROM claude_run", [], |r| r.get(0))
            .unwrap();

        for _ in 0..3 {
            record(&mut conn, &sweep).unwrap();
        }

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            n, 1,
            "four sweeps of one unparseable orphan must be one row -- a \
             clock-derived key would mint a new one every tick"
        );
        let still: String = conn
            .query_row("SELECT ended_at FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(still, when, "and the first observation still stands");
    }

    /// The registry's `name` fills a NULL and never overwrites.
    ///
    /// It is the one field this source has that no other does -- the hook
    /// payload carries no name and the transcript has `aiTitle` -- but a
    /// user's own rename (#922) must still survive a sweep.
    #[test]
    fn the_registry_name_fills_a_null_and_never_overwrites() {
        let mut conn = db();
        record(
            &mut conn,
            &Sweep {
                running: vec![live(14779, Some("s1"), Some(1_789_119_828))],
                ..Default::default()
            },
        )
        .unwrap();
        let name: Option<String> = conn
            .query_row("SELECT name FROM claude_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name.as_deref(), Some("widget-14779"));

        conn.execute("UPDATE claude_session SET name = 'My own label'", [])
            .unwrap();
        record(
            &mut conn,
            &Sweep {
                running: vec![live(14779, Some("s1"), Some(1_789_119_828))],
                ..Default::default()
            },
        )
        .unwrap();
        let after: Option<String> = conn
            .query_row("SELECT name FROM claude_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after.as_deref(), Some("My own label"));
    }

    /// A registry `cwd` never displaces one read from disk.
    ///
    /// Same rule as the handoff consumer, for the same reason: the
    /// transcript importer wins on `cwd` because disk is ground truth.
    #[test]
    fn a_registry_cwd_never_overwrites_one_from_disk() {
        let mut conn = db();
        conn.execute(
            "INSERT INTO claude_session (session_id, cwd, first_seen_at)
             VALUES ('s1', '/Users/acme/code/the-real-one', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        record(
            &mut conn,
            &Sweep {
                running: vec![live(14779, Some("s1"), Some(1_789_119_828))],
                ..Default::default()
            },
        )
        .unwrap();
        let cwd: Option<String> = conn
            .query_row("SELECT cwd FROM claude_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cwd.as_deref(), Some("/Users/acme/code/the-real-one"));
    }

    /// `first_seen_at` only ever moves EARLIER.
    #[test]
    fn first_seen_only_moves_earlier() {
        let mut conn = db();
        conn.execute(
            "INSERT INTO claude_session (session_id, first_seen_at)
             VALUES ('s1', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        record(
            &mut conn,
            &Sweep {
                running: vec![live(14779, Some("s1"), Some(1_789_119_828))],
                ..Default::default()
            },
        )
        .unwrap();
        let first: String = conn
            .query_row("SELECT first_seen_at FROM claude_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(first, "2026-01-01T00:00:00Z");
    }

    /// A record with no `session_id` is counted, not dropped silently.
    #[test]
    fn a_record_with_no_session_id_is_counted() {
        let mut conn = db();
        let got = record(
            &mut conn,
            &Sweep {
                orphaned: vec![live(80043, None, Some(1_789_119_828))],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(got.without_session_id, 1);
        assert_eq!(got.crashed, 0);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "no id, no identity, no row");
    }

    /// The unreadable count survives the storage boundary.
    ///
    /// A count that only reaches a log line is a count nobody sees --
    /// `store::import`'s rule, and the `ClaudeMdPage` defect (#846) it
    /// cites.
    #[test]
    fn what_could_not_be_read_survives_the_storage_boundary() {
        let mut conn = db();
        let got = record(
            &mut conn,
            &Sweep {
                unreadable: vec!["/Users/acme/.claude/sessions/7.json: broken".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(got.is_partial());
        assert_eq!(got.unreadable.len(), 1);
    }

    /// `start_times` carries only the RUNNING pids.
    ///
    /// An orphan's `procStart` describes a process that no longer exists.
    /// Handing it to the handoff consumer would let a pid the kernel has
    /// since recycled inherit a dead session's start time and read as
    /// confirmed -- the pid-reuse fail-open, reintroduced by the back
    /// door.
    ///
    /// Sabotage: chaining `sweep.orphaned` into the iterator fails this.
    #[test]
    fn the_start_times_handed_on_are_the_live_ones_only() {
        let sweep = Sweep {
            running: vec![live(14779, Some("s-live"), Some(1_789_119_828))],
            orphaned: vec![live(80043, Some("s-dead"), Some(1_789_293_439))],
            unknown: vec![live(500, Some("s-maybe"), Some(1_789_100_000))],
            ..Default::default()
        };
        let times = start_times(&sweep);
        assert_eq!(times.get(&14779), Some(&1_789_119_828));
        assert_eq!(
            times.get(&80043),
            None,
            "a dead process's start time must not confirm a recycled pid"
        );
        assert_eq!(times.get(&500), None, "nor an unconfirmed one");
    }
}
