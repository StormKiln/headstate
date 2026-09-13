//! Aggregates for the Claude Code overview page (#921, epic #910).
//!
//! One question decides what is in this file and what is not: *does this
//! help me see what is going on, or resurrect something?* #921 lists five
//! candidates and asks for a cut rather than all five, and the cut below
//! is argued from measurements on the real corpus rather than from taste.
//!
//! # What the real corpus says, and it reframes the feature
//!
//! Measured by `tests::real_corpus_overview` on the development machine:
//!
//! ```text
//! sessions                  1461
//! live right now               3
//! cwd still exists           248      <- 17%
//! cwd gone                  1213      <- 83%
//! resumable (not live, cwd exists)   246
//!   ... last active within 7 days     126
//! distinct working directories       665
//! distinct claude versions            37
//! days with any activity              41
//! ```
//!
//! So the population is overwhelmingly ARCHAEOLOGY: 83% of sessions ran
//! in a directory that no longer exists, almost all of them deleted agent
//! worktrees. The overview's job is to find the 17% that is still live
//! work, and everything here is shaped by that ratio.
//!
//! # The correction to #921, and it is the important one
//!
//! #921 proposes that the single most valuable number is "how many
//! sessions are resurrection candidates -- dead pid, no clean end", and
//! calls that the feature's whole reason to exist. The premise is right
//! and the PREDICATE is wrong, measurably:
//!
//! **"dead pid, no `SessionEnd`" matches zero sessions on this machine,
//! and will keep matching almost none.**
//!
//! A pid reaches `claude_run` only from the `SessionStart` hook (#912,
//! #913). Every one of the 1,461 sessions here was imported from a
//! transcript (#914), and a transcript import is forbidden from writing
//! `claude_run` at all -- `store.rs` says why, and migration 11 declares
//! `claude_run.pid NOT NULL` precisely so an unobserved process cannot be
//! recorded as an observed one. So every session has zero runs, no pid
//! was ever recorded, and a "dead pid" filter selects nothing.
//!
//! That is not a transient state of an unfinished epic either. Sessions
//! that ran BEFORE the hook was installed can never acquire a pid
//! retroactively, and that is the entire history on any machine that
//! adopts Headstate after using Claude Code -- which is the normal case.
//!
//! The predicate that actually selects the actionable set is
//! **not running, and its directory still exists**:
//!
//! | predicate | matches here | what it means |
//! |---|---|---|
//! | dead pid, no `SessionEnd` | **0** | needs a pid we never recorded |
//! | not running + cwd exists | **246** | resumable INTO the tree it came from |
//! | not running + cwd gone | 1212 | resumable, but landing anywhere |
//!
//! 246 is the number worth putting at the top of a page, because it is
//! the count of sessions a user can act on today. `Counts::resumable`
//! is therefore that predicate, and [`Counts::orphaned_runs`] carries
//! #921's predicate alongside it rather than instead of it -- it becomes
//! the right answer once the hook is installed and starts observing
//! processes, and reporting it as a separate figure is how a reader can
//! tell "we never watched" from "we watched and it died".
//!
//! # Liveness is NOT derived here
//!
//! #917 owns that (`claude/liveness.rs`): a three-state derivation from
//! `(pid, pid_start_time)` against the process table, plus the live
//! registry under `~/.claude/sessions`. Re-deriving it here would give
//! the page two answers to one question, and the two would disagree the
//! first time either changed.
//!
//! What this module does instead is count over the SAME rows the list
//! already resolved. `running_session_ids` comes in as a parameter, from
//! whatever already asked the machine, and everything else is SQL over
//! `claude_session` / `claude_run`. There is no process probe in this
//! file and no registry read.
//!
//! # Absent is not zero, and a chart is the worst place to break it
//!
//! [`Overview`] is returned whole or not at all: a failed query is an
//! `Err` the view renders as "could not tell", never a struct of zeros.
//! That matters more here than almost anywhere else in the app, because
//! a bar chart of zeros does not LOOK absent -- a flat line reads as a
//! measured quiet week. A tile showing "0 resumable" on a machine with
//! 246 of them is a confidently wrong answer, and #846 is the same bug
//! one view over: a `= []` default made a REJECTED scan read as "no
//! CLAUDE.md files in this repository".
//!
//! So the two counts that can be partially unknown carry their unknowns
//! as data -- [`Counts::cwd_unknown`] is sessions whose directory could
//! not be checked, and it is neither "exists" nor "gone".

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// How many days of daily buckets the activity chart gets.
///
/// 30, and it is a measurement rather than a round number: the corpus
/// spans 41 days with any activity at all, of which the last 30 hold
/// 1,375 of 1,461 sessions (94%). A 90-day window would be 60 empty
/// columns either side of the real data, and an all-time window on a
/// machine used for a year would compress the shape that matters into a
/// few pixels.
///
/// Mirrored by `ACTIVITY_DAYS` in `ClaudeOverviewPage.tsx`, which quotes
/// it in the chart's own subtitle -- `mirroredConstants.test.ts` is the
/// house mechanism for keeping a Rust literal and the sentence that
/// describes it from drifting.
pub const ACTIVITY_DAYS: i64 = 30;

/// How many rows the resumable list returns.
///
/// The list is the page's centrepiece, not a leaderboard, so this is a
/// working set rather than a ranking: 246 sessions qualify here and
/// nobody scrolls 246 rows looking for the one they want. 12 is about a
/// screen, and `Counts::resumable` states the real total beside it so the
/// cut is never silent -- the house rule (#846) forbids quietly showing
/// fewer rows than exist, not showing a stated subset.
pub const RESUMABLE_SHOWN: usize = 12;

/// Whether a session's recorded working directory is still there.
///
/// Three states, for the reason #918's `CwdState` has four: "gone" and
/// "could not check" have opposite remedies. A permission error means
/// the tree may well be there and resuming into it would have worked;
/// reporting that as "gone" sends someone looking for work that was
/// never lost.
///
/// This is a COUNTING type, so it is deliberately coarser than #918's
/// per-row one -- there is no path to report in an aggregate, and
/// `not-recorded` folds into `Unknown` here because both mean "we cannot
/// say this session is resumable into its own tree".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cwd {
    Exists,
    Gone,
    Unknown,
}

/// Check one recorded directory.
///
/// `symlink_metadata` rather than `exists()`: `exists()` collapses every
/// error into `false`, which is exactly the fail-open #918 rejects --
/// a directory behind an unreadable parent would be counted as deleted.
fn check_cwd(cwd: Option<&str>) -> Cwd {
    let Some(path) = cwd.filter(|s| !s.is_empty()) else {
        return Cwd::Unknown;
    };
    match std::fs::symlink_metadata(path) {
        Ok(_) => Cwd::Exists,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Cwd::Gone,
        // Permission denied, a broken mount, a path too long: we did not
        // establish that it is gone, so we do not say so.
        Err(_) => Cwd::Unknown,
    }
}

/// The headline figures, each answering "what can I act on".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    /// Every session Headstate has a row for.
    ///
    /// Present as the DENOMINATOR the other figures are read against,
    /// not as a hero number: "1,461 sessions ever" answers nothing on
    /// its own, which is why #921's total-sessions tile is cut and this
    /// appears only as context under the tiles that matter.
    pub sessions: usize,
    /// Sessions whose process is running right now.
    ///
    /// Counted from the ids the caller resolved, never re-derived here.
    pub running: usize,
    /// Not running, and the recorded directory still exists.
    ///
    /// **The page's headline number.** 246 of 1,461 here -- the sessions
    /// a user can resume back into the tree they came from. See the
    /// module comment for why this, and not "dead pid", is the
    /// resurrection predicate.
    pub resumable: usize,
    /// Not running, and the recorded directory is gone.
    ///
    /// 1,212 of 1,461 here, so this is the NORMAL state and must not be
    /// rendered as damage. These are still resumable by id -- `claude
    /// --resume` works from anywhere -- but they land in whatever
    /// directory the command is run from, which is #918's whole subject.
    pub archived: usize,
    /// Not running, and the directory could not be CHECKED.
    ///
    /// Neither `resumable` nor `archived`, and kept out of both rather
    /// than folded into the larger one. A session counted as archived
    /// because a stat failed is a row the user is told to give up on.
    pub cwd_unknown: usize,
    /// Runs the hook observed starting and never observed ending.
    ///
    /// #921's proposed predicate, reported rather than used as the
    /// headline, because it is **0** on every machine whose history
    /// predates the hook -- see the module comment. It becomes the
    /// sharper signal once #912/#913 are installed and observing, since
    /// a run with no `ended_at` whose process is gone is a session that
    /// was KILLED rather than one that exited: `SessionEnd` does not
    /// fire on SIGKILL, which is the epic's founding measurement.
    ///
    /// Counted from runs whose session is not currently running, so a
    /// live session's own open run is not reported as an orphan.
    pub orphaned_runs: usize,
    /// Sessions the hook has never observed at all (`runs = 0`).
    ///
    /// The honest companion to `orphaned_runs`: without it a reader
    /// cannot tell "nothing crashed" from "nothing was watched", and on
    /// this corpus the answer is the second for all 1,461 rows.
    pub never_observed: usize,
}

/// One day's bucket for the activity chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayCount {
    /// `YYYY-MM-DD`, UTC.
    pub day: String,
    /// Sessions whose FIRST activity fell on this day -- sessions
    /// started, not sessions touched. A session resumed over four days
    /// counts once, on the day it began, so the series reads as intake.
    pub started: usize,
}

/// One resumable session, with just enough to decide whether it is the
/// one you want.
///
/// A deliberately thin row. The full shape -- liveness, the resume
/// command with its `cd`, the caveat -- is #917/#918's `SessionRow`, and
/// duplicating it here would be a second definition of the same thing.
/// This carries the id, so the page hands off to the list rather than
/// re-implementing the actions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumableRow {
    pub session_id: String,
    /// Claude's own `aiTitle`. `None` for the handful that never got
    /// one -- never the UUID dressed up as a name, per #914.
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub last_activity_at: Option<String>,
}

/// Everything the overview page draws.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Overview {
    pub counts: Counts,
    /// Exactly [`ACTIVITY_DAYS`] buckets, oldest first, INCLUDING days
    /// with no sessions.
    ///
    /// The empty days are the point. A series of only the days that had
    /// activity draws a dense chart with no gaps, so a week off reads as
    /// a week of steady work at whatever the neighbouring values were --
    /// the axis lies about the shape. Filled server-side so the chart
    /// component cannot get it wrong.
    pub activity: Vec<DayCount>,
    /// The newest [`RESUMABLE_SHOWN`] of `counts.resumable`.
    pub resumable: Vec<ResumableRow>,
}

/// [`Overview`] plus what could not be established about liveness.
///
/// The two are separate fields rather than one merged struct because they
/// FAIL separately, and the page renders the two failures differently. A
/// database we could not read is an `Err` and the page shows nothing but
/// the reason; a live registry we could not read still leaves every count
/// over stored history valid, and the honest rendering is the page with a
/// banner saying the running figure is not to be trusted.
///
/// Collapsing them would force one of two wrong behaviours: hiding 1,461
/// sessions' worth of real aggregates because a 3-file directory was
/// unreadable, or -- worse -- showing "0 running" as though it were an
/// answer. The second is #841's fail-open and it is the one a user acts
/// on: "nothing is running" is what makes a Resume button look safe.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OverviewReport {
    #[serde(flatten)]
    pub overview: Overview,
    /// Why the live session registry could not be listed. `None` means it
    /// WAS read, so `counts.running` is a real answer -- including when
    /// it is zero.
    pub live_failure: Option<String>,
    /// Registry records present but unusable, with why. Each one hides a
    /// session that may be running, so a non-empty list makes
    /// `counts.running` a floor rather than a count.
    pub live_unreadable: Vec<String>,
}

/// A stored row, before the directory check.
struct Row {
    session_id: String,
    name: Option<String>,
    cwd: Option<String>,
    git_branch: Option<String>,
    first_seen_at: String,
    last_activity_at: Option<String>,
}

/// Aggregate the stored corpus for the overview.
///
/// `running_session_ids` is the set of sessions the caller has already
/// established are alive -- #917's liveness derivation, passed in rather
/// than repeated. An EMPTY set is ambiguous on its own ("nothing is
/// running" or "we could not look"), and this function does not try to
/// resolve that: the caller knows which, and the view says so from its
/// own `live_failure`. What this guarantees is only that a session IN the
/// set is never counted as resumable, so a live session the caller told
/// us about can never appear in the list offering to resurrect it.
///
/// # The direction this cannot guarantee, and who has to say so
///
/// The converse does not hold, and it is the half worth stating because
/// it is the one that misleads an ACTION. A session is classified as
/// resumable exactly when it is NOT in the set and its directory exists.
/// So a session that is running but absent from the set -- because the
/// registry could not be listed, or because its record could not be
/// parsed -- lands in the resumable list, and resuming a session that is
/// already alive starts a second copy of it.
///
/// That is not fixable here: this function is given a set and cannot know
/// whether the set is complete. It is the caller's failure to report, and
/// [`OverviewReport`] carries `live_failure` and `live_unreadable` for
/// exactly that reason -- the page names the over-count in both banners
/// rather than claiming the figures below are unaffected, which is true
/// of the history and false of the one figure a user acts on.
///
/// # Cost
///
/// One `SELECT` over `claude_session`, one over `claude_run`, and one
/// `symlink_metadata` per session. The stat is the expensive part at
/// 1,461 rows and it is the same work #917's list already does per row;
/// measured at 22-31 ms warm for the whole function by
/// `tests::real_corpus_overview`, which is why there is no cache here.
pub fn aggregate(
    conn: &Connection,
    running_session_ids: &HashSet<String>,
    today: chrono::DateTime<chrono::Utc>,
) -> Result<Overview, rusqlite::Error> {
    let rows = stored_rows(conn)?;
    let runs = run_summary(conn)?;

    let mut counts = Counts {
        sessions: rows.len(),
        ..Default::default()
    };
    // Pre-sized to the window rather than grown, and keyed by date
    // string so a bucket can only be written by a day that exists in the
    // window -- an off-by-one in the arithmetic below cannot invent a
    // 31st column.
    let mut buckets: HashMap<String, usize> = HashMap::new();
    let mut resumable: Vec<ResumableRow> = Vec::new();

    for row in &rows {
        let running = running_session_ids.contains(&row.session_id);
        if running {
            counts.running += 1;
        } else {
            match check_cwd(row.cwd.as_deref()) {
                Cwd::Exists => {
                    counts.resumable += 1;
                    resumable.push(ResumableRow {
                        session_id: row.session_id.clone(),
                        name: row.name.clone(),
                        cwd: row.cwd.clone(),
                        git_branch: row.git_branch.clone(),
                        last_activity_at: row.last_activity_at.clone(),
                    });
                }
                Cwd::Gone => counts.archived += 1,
                Cwd::Unknown => counts.cwd_unknown += 1,
            }
        }

        match runs.get(&row.session_id) {
            None => counts.never_observed += 1,
            Some(summary) => {
                // Only a session we do NOT believe is running: a live
                // session's open run is the run of the thing that is
                // alive, not an orphan.
                if !running {
                    counts.orphaned_runs += summary.open;
                }
            }
        }

        // The day a session STARTED, which is `first_seen_at`. Buckets
        // by the UTC date the timestamp carries, parsed rather than
        // string-sliced: a stored value in a non-UTC offset would slice
        // to the wrong day, and both sources of this column write
        // RFC3339 without guaranteeing the offset is `Z`.
        if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&row.first_seen_at) {
            let day = t.with_timezone(&chrono::Utc).date_naive();
            let key = day.to_string();
            if in_window(day, today) {
                *buckets.entry(key).or_default() += 1;
            }
        }
    }

    // Newest activity first, and a session with NO recorded activity
    // sorts last rather than first -- `None` means "we never saw a
    // timestamp", and letting it sort as the newest would put the rows
    // we know least about at the top of the actionable list. Matches the
    // ordering #917's list uses, so the two never disagree about which
    // session is most recent.
    resumable.sort_by(|a, b| {
        b.last_activity_at
            .is_some()
            .cmp(&a.last_activity_at.is_some())
            .then_with(|| b.last_activity_at.cmp(&a.last_activity_at))
    });
    resumable.truncate(RESUMABLE_SHOWN);

    Ok(Overview {
        counts,
        activity: fill_window(&buckets, today),
        resumable,
    })
}

/// Whether a date falls in the chart's window.
///
/// Inclusive of today and of the day [`ACTIVITY_DAYS`] - 1 back, so the
/// window is exactly that many days wide and today is its right edge.
fn in_window(day: chrono::NaiveDate, today: chrono::DateTime<chrono::Utc>) -> bool {
    let end = today.date_naive();
    let start = end - chrono::Duration::days(ACTIVITY_DAYS - 1);
    day >= start && day <= end
}

/// Every day in the window, oldest first, zero-filled.
///
/// A zero here is a MEASURED zero -- the window is complete, we counted
/// every session, and this day had none. That is different from the
/// absent case, which is an `Err` out of [`aggregate`] and never reaches
/// this function at all. The distinction is the whole reason the return
/// type is a `Result` rather than a struct with an error field in it.
fn fill_window(
    buckets: &HashMap<String, usize>,
    today: chrono::DateTime<chrono::Utc>,
) -> Vec<DayCount> {
    let end = today.date_naive();
    (0..ACTIVITY_DAYS)
        .rev()
        .map(|back| {
            let day = end - chrono::Duration::days(back);
            let key = day.to_string();
            DayCount {
                started: buckets.get(&key).copied().unwrap_or(0),
                day: key,
            }
        })
        .collect()
}

fn stored_rows(conn: &Connection) -> Result<Vec<Row>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT session_id, name, cwd, git_branch, first_seen_at, last_activity_at
         FROM claude_session",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Row {
            session_id: r.get(0)?,
            name: r.get(1)?,
            cwd: r.get(2)?,
            git_branch: r.get(3)?,
            first_seen_at: r.get(4)?,
            last_activity_at: r.get(5)?,
        })
    })?;
    rows.collect()
}

/// How many runs of each session the hook saw start and not end.
struct RunSummary {
    open: usize,
}

fn run_summary(conn: &Connection) -> Result<HashMap<String, RunSummary>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT session_id, SUM(ended_at IS NULL)
         FROM claude_run GROUP BY session_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            RunSummary {
                open: r.get::<_, i64>(1)?.max(0) as usize,
            },
        ))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (id, summary) = row?;
        out.insert(id, summary);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    fn insert(conn: &Connection, id: &str, cwd: Option<&str>, first: &str, last: Option<&str>) {
        conn.execute(
            "INSERT INTO claude_session (session_id, name, cwd, first_seen_at, last_activity_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, format!("about {id}"), cwd, first, last],
        )
        .unwrap();
    }

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    fn today() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-09-13T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    /// A directory that could not be CHECKED is neither resumable nor
    /// archived.
    ///
    /// The fail-open this guards is `Path::exists()`, which returns
    /// `false` for a permission error just as it does for a deleted
    /// directory. Sabotage: replacing `check_cwd`'s match with
    /// `Path::new(path).exists()` moves this session into `archived`,
    /// telling the user to give up on work that is still on disk.
    ///
    /// Uses a path under an unreadable PARENT rather than an unreadable
    /// file, because a directory is not a portable proxy for an
    /// unreadable path -- and `temp_dir()` rather than `/tmp`, which
    /// does not exist on Windows (two agents in this epic hit exactly
    /// that).
    #[test]
    fn a_directory_that_could_not_be_checked_is_not_counted_as_gone() {
        // A path with an interior NUL cannot be handed to the OS at all,
        // so the stat fails with an error that is NOT NotFound -- on
        // every platform, which is what makes it the portable way to
        // reach the third arm. A permission fixture would need root on
        // one platform and does not exist on another.
        let conn = db();
        insert(
            &conn,
            "unknowable",
            Some("a\0b"),
            "2026-09-12T10:00:00Z",
            Some("2026-09-12T10:00:00Z"),
        );

        let out = aggregate(&conn, &none(), today()).unwrap();
        assert_eq!(
            out.counts.cwd_unknown, 1,
            "the stat failed, so we cannot say"
        );
        assert_eq!(out.counts.archived, 0, "never reported as deleted");
        assert_eq!(out.counts.resumable, 0, "and never offered as resumable");
        assert!(
            out.resumable.is_empty(),
            "a session we could not check must not appear in the resumable list"
        );
    }

    /// A session with no recorded cwd is `cwd_unknown`, not archived.
    ///
    /// Distinct from the case above in cause and identical in treatment:
    /// there is no path, so there is nothing to report as missing.
    #[test]
    fn a_session_with_no_recorded_directory_is_not_counted_as_gone() {
        let conn = db();
        insert(&conn, "nocwd", None, "2026-09-12T10:00:00Z", None);
        let out = aggregate(&conn, &none(), today()).unwrap();
        assert_eq!(out.counts.cwd_unknown, 1);
        assert_eq!(out.counts.archived, 0);
    }

    /// A running session is never counted as resumable.
    ///
    /// The fail-open that matters most on this page: resuming a session
    /// that is in fact alive starts a SECOND copy of it, so a live row
    /// in the resumable list is the wrong action confidently offered.
    /// Sabotage: dropping the `if running` branch puts the live session
    /// in `resumable` AND in the list, since its cwd exists.
    #[test]
    fn a_running_session_is_never_offered_as_resumable() {
        let conn = db();
        let dir = std::env::temp_dir();
        let cwd = dir.to_str().unwrap();
        insert(
            &conn,
            "live",
            Some(cwd),
            "2026-09-13T10:00:00Z",
            Some("2026-09-13T11:00:00Z"),
        );
        insert(
            &conn,
            "dead",
            Some(cwd),
            "2026-09-12T10:00:00Z",
            Some("2026-09-12T11:00:00Z"),
        );

        let mut running = HashSet::new();
        running.insert("live".to_string());
        let out = aggregate(&conn, &running, today()).unwrap();

        assert_eq!(out.counts.running, 1);
        assert_eq!(out.counts.resumable, 1, "only the dead one");
        assert_eq!(
            out.resumable
                .iter()
                .map(|r| r.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["dead"],
        );
    }

    /// A session missing from the set IS counted resumable, and that is
    /// the caller's problem to report.
    ///
    /// Pinned rather than left implicit, because it is the one direction
    /// `aggregate` cannot get right on its own and the page's banners are
    /// the whole mitigation. If this behaviour ever changed -- if some
    /// future edit tried to guess at liveness here -- the banners would
    /// become wrong in the other direction, and a reader of this file
    /// would have no way to know the wording depends on it.
    #[test]
    fn a_session_absent_from_the_running_set_is_counted_resumable() {
        let conn = db();
        let dir = std::env::temp_dir();
        let cwd = dir.to_str().unwrap();
        insert(
            &conn,
            "actually-running",
            Some(cwd),
            "2026-09-13T10:00:00Z",
            Some("2026-09-13T11:00:00Z"),
        );

        // The caller could not read the registry, so it passes an empty
        // set even though this session is alive.
        let out = aggregate(&conn, &none(), today()).unwrap();
        assert_eq!(
            out.counts.resumable, 1,
            "this function is given a set and cannot know it is incomplete"
        );
        assert_eq!(out.counts.running, 0);
        // Which is exactly why `OverviewReport` carries `live_failure`
        // and the page says a running session may be counted here.
    }

    /// #921's predicate is reported, and it is zero for imported history.
    ///
    /// The measurement behind the module comment's correction, pinned as
    /// a test so the claim stays checkable: a transcript-imported
    /// session has no run at all, so "dead pid, no SessionEnd" cannot
    /// match it, and `never_observed` is what says so.
    #[test]
    fn an_imported_session_is_never_observed_rather_than_orphaned() {
        let conn = db();
        insert(&conn, "imported", None, "2026-09-12T10:00:00Z", None);
        let out = aggregate(&conn, &none(), today()).unwrap();
        assert_eq!(out.counts.never_observed, 1);
        assert_eq!(
            out.counts.orphaned_runs, 0,
            "no pid was ever recorded, so nothing can be a dead pid"
        );
    }

    /// A run the hook saw start and never saw end IS an orphan, once
    /// there is a hook.
    ///
    /// The other half of the correction: the predicate is right, it just
    /// needs the pipeline. A session whose run has no `ended_at` and
    /// which is not running is the SIGKILL case the epic is founded on.
    #[test]
    fn a_run_that_never_ended_is_orphaned_when_the_session_is_not_running() {
        let conn = db();
        insert(&conn, "killed", None, "2026-09-12T10:00:00Z", None);
        conn.execute(
            "INSERT INTO claude_run (session_id, pid, started_at, ended_at)
             VALUES ('killed', 4242, '2026-09-12T10:00:00Z', NULL)",
            [],
        )
        .unwrap();

        let out = aggregate(&conn, &none(), today()).unwrap();
        assert_eq!(out.counts.orphaned_runs, 1);
        assert_eq!(out.counts.never_observed, 0, "this one WAS observed");

        // And a live session's own open run is not an orphan: it is the
        // run of the thing that is alive. Sabotage: counting `open`
        // unconditionally reports every running session as a crash.
        let mut running = HashSet::new();
        running.insert("killed".to_string());
        let live = aggregate(&conn, &running, today()).unwrap();
        assert_eq!(
            live.counts.orphaned_runs, 0,
            "the open run belongs to the process that is running"
        );
    }

    /// The window is always exactly `ACTIVITY_DAYS` buckets, zero-filled.
    ///
    /// A series of only the days that had activity draws a dense chart
    /// with no gaps, so a week off reads as a week of steady work.
    /// Sabotage: returning `buckets` as-is gives 1 point instead of 30.
    #[test]
    fn the_activity_window_is_complete_and_zero_filled() {
        let conn = db();
        insert(&conn, "one", None, "2026-09-13T09:00:00Z", None);
        let out = aggregate(&conn, &none(), today()).unwrap();

        assert_eq!(out.activity.len() as i64, ACTIVITY_DAYS);
        assert_eq!(out.activity.first().unwrap().day, "2026-08-15");
        assert_eq!(out.activity.last().unwrap().day, "2026-09-13");
        assert_eq!(out.activity.last().unwrap().started, 1);
        assert_eq!(
            out.activity.iter().filter(|d| d.started == 0).count(),
            (ACTIVITY_DAYS - 1) as usize,
            "every other day is a measured zero, and present",
        );
    }

    /// A session older than the window is counted, and NOT bucketed.
    ///
    /// Both halves matter. It belongs in `sessions` because it exists;
    /// it must not land in a bucket because there is no column for its
    /// day, and clamping it into the oldest one would draw a spike that
    /// never happened.
    #[test]
    fn a_session_older_than_the_window_is_counted_but_not_charted() {
        let conn = db();
        insert(&conn, "ancient", None, "2026-01-01T09:00:00Z", None);
        let out = aggregate(&conn, &none(), today()).unwrap();
        assert_eq!(out.counts.sessions, 1);
        assert_eq!(
            out.activity.iter().map(|d| d.started).sum::<usize>(),
            0,
            "outside the window, so it is in no bucket"
        );
    }

    /// A stored timestamp in a non-UTC offset buckets by its UTC day.
    ///
    /// Sabotage: `&row.first_seen_at[..10]` passes every other test in
    /// this file and puts this session in the wrong column -- 22:00 on
    /// the 12th at -05:00 is 03:00 on the 13th in UTC, and the chart's
    /// own subtitle says UTC.
    #[test]
    fn a_non_utc_timestamp_buckets_by_its_utc_day() {
        let conn = db();
        insert(&conn, "offset", None, "2026-09-12T22:00:00-05:00", None);
        let out = aggregate(&conn, &none(), today()).unwrap();
        let day = out
            .activity
            .iter()
            .find(|d| d.started > 0)
            .expect("it is inside the window");
        assert_eq!(day.day, "2026-09-13", "03:00Z on the 13th");
    }

    /// The resumable list is newest-first and states its total.
    ///
    /// `resumable` in `counts` is the real number; the list is a stated
    /// subset. Sabotage: truncating without the count makes the page
    /// claim 12 resumable sessions when there are 246.
    #[test]
    fn the_resumable_list_is_a_stated_subset_newest_first() {
        let conn = db();
        let dir = std::env::temp_dir();
        let cwd = dir.to_str().unwrap();
        for i in 0..RESUMABLE_SHOWN + 5 {
            insert(
                &conn,
                &format!("s{i:02}"),
                Some(cwd),
                "2026-09-01T00:00:00Z",
                Some(&format!("2026-09-{:02}T00:00:00Z", i + 1)),
            );
        }
        let out = aggregate(&conn, &none(), today()).unwrap();

        assert_eq!(out.counts.resumable, RESUMABLE_SHOWN + 5, "the real total");
        assert_eq!(out.resumable.len(), RESUMABLE_SHOWN, "the stated subset");
        // Newest activity first.
        let days: Vec<&str> = out
            .resumable
            .iter()
            .map(|r| r.last_activity_at.as_deref().unwrap())
            .collect();
        let mut sorted = days.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(days, sorted);
    }

    /// A session with no recorded activity sorts LAST, not first.
    ///
    /// `None` means "we never saw a timestamp", and an ascending sort
    /// that puts nulls first would top the actionable list with the rows
    /// we know least about.
    #[test]
    fn a_session_with_no_activity_sorts_last_in_the_resumable_list() {
        let conn = db();
        let dir = std::env::temp_dir();
        let cwd = dir.to_str().unwrap();
        insert(&conn, "timeless", Some(cwd), "2026-09-10T00:00:00Z", None);
        insert(
            &conn,
            "dated",
            Some(cwd),
            "2026-09-10T00:00:00Z",
            Some("2026-09-10T01:00:00Z"),
        );
        let out = aggregate(&conn, &none(), today()).unwrap();
        assert_eq!(
            out.resumable
                .iter()
                .map(|r| r.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["dated", "timeless"],
        );
    }

    /// An empty database gives a complete window of measured zeros, not
    /// an empty series.
    ///
    /// The distinction the type exists for: this is the SUCCESS case on
    /// a machine with no sessions, and it is reachable only because the
    /// error case is an `Err` instead. The page renders "no sessions"
    /// here and "could not tell" there.
    #[test]
    fn an_empty_corpus_is_a_complete_window_of_zeros() {
        let out = aggregate(&db(), &none(), today()).unwrap();
        assert_eq!(out.counts, Counts::default());
        assert_eq!(out.activity.len() as i64, ACTIVITY_DAYS);
        assert!(out.activity.iter().all(|d| d.started == 0));
        assert!(out.resumable.is_empty());
    }

    /// A missing table is an `Err`, never a zeroed [`Overview`].
    ///
    /// The single most important test in this file, and the one #921's
    /// brief names: a chart that plots nothing because the query failed
    /// is worse than no chart, because a flat line reads as real data.
    /// A zeroed struct would render 30 columns of honest-looking zero
    /// and a tile saying "0 resumable" on a machine with 246.
    ///
    /// Sabotage: `.unwrap_or_default()` on either query in `aggregate`
    /// makes this return `Ok(Overview::default())` -- which is exactly
    /// the shape `an_empty_corpus_is_a_complete_window_of_zeros`
    /// asserts is legitimate, so the two tests together are what pin
    /// the difference.
    #[test]
    fn a_failed_query_is_an_error_and_never_a_page_of_zeros() {
        let conn = Connection::open_in_memory().unwrap();
        // No migration, so `claude_session` does not exist.
        let err = aggregate(&conn, &none(), today());
        assert!(
            err.is_err(),
            "a query that could not run must not become a struct of zeros"
        );
    }

    /// Measured against the real corpus on this machine.
    ///
    /// `--ignored` because it depends on a populated database rather
    /// than a fixture, and prints rather than asserts the counts: the
    /// figures climb as the machine is used, so pinning them would fail
    /// tomorrow for the right reason. What it ASSERTS is the shape the
    /// module comment's argument rests on -- that the corpus is mostly
    /// archaeology, and that the timing claim holds.
    #[test]
    #[ignore = "needs the real ~/.claude corpus; run with --ignored"]
    fn real_corpus_overview() {
        let scan = crate::claude::scan_default().expect("a real ~/.claude/projects");
        let mut conn = db();
        crate::claude::store::import(&mut conn, scan).unwrap();

        let t0 = std::time::Instant::now();
        let out = aggregate(&conn, &none(), chrono::Utc::now()).unwrap();
        let elapsed = t0.elapsed();

        println!("sessions            {}", out.counts.sessions);
        println!("resumable           {}", out.counts.resumable);
        println!("archived (cwd gone) {}", out.counts.archived);
        println!("cwd unknown         {}", out.counts.cwd_unknown);
        println!("never observed      {}", out.counts.never_observed);
        println!("orphaned runs       {}", out.counts.orphaned_runs);
        println!(
            "charted in window   {}",
            out.activity.iter().map(|d| d.started).sum::<usize>()
        );
        println!("aggregate elapsed   {:?}", elapsed);

        // And again WITH the live registry, which is the path the command
        // actually takes -- so a live session is proved to be subtracted
        // from the resumable count rather than offered for resurrection.
        let live = crate::claude::live::running_ids_default();
        let t1 = std::time::Instant::now();
        let with_live = aggregate(&conn, &live.ids, chrono::Utc::now()).unwrap();
        println!("--- with the live registry ---");
        println!("live ids            {}", live.ids.len());
        println!("running             {}", with_live.counts.running);
        println!("resumable           {}", with_live.counts.resumable);
        println!("live failure        {:?}", live.failure);
        println!("elapsed             {:?}", t1.elapsed());
        assert_eq!(
            with_live.counts.running,
            live.ids.len(),
            "every live id must be a session we have a row for"
        );
        // The whole point of passing the set in: a running session is not
        // in the resumable list.
        assert!(
            with_live
                .resumable
                .iter()
                .all(|r| !live.ids.contains(&r.session_id)),
            "a live session must never be offered as resumable"
        );

        assert!(out.counts.sessions > 100, "a real corpus");
        assert_eq!(out.activity.len() as i64, ACTIVITY_DAYS);
        // The ratio the whole design rests on: most sessions ran in a
        // directory that is gone.
        assert!(
            out.counts.archived > out.counts.resumable,
            "the corpus is mostly archaeology: {} archived vs {} resumable",
            out.counts.archived,
            out.counts.resumable,
        );
        // The correction to #921, asserted rather than only claimed.
        assert_eq!(
            out.counts.never_observed, out.counts.sessions,
            "every session is transcript-imported until the hook lands"
        );
    }
}
