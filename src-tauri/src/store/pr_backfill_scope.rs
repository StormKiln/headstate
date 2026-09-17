//! Which scopes the background worker is allowed to walk, and how far back
//! (#1092, design #1094).
//!
//! # Why a worker needs a list at all
//!
//! Left to itself a backfill has two bad options: walk everything the
//! token can see, or walk nothing. The first spends the user's hourly
//! budget on organisations they have never opened -- and `Scope::All` has
//! no bound on the repositories it touches, which #1094 flags as an open
//! risk. The second is a feature that does not run.
//!
//! So a scope is registered when the user OPENS it. Background spend then
//! follows demonstrated interest: the scopes that get walked are exactly
//! the ones somebody has looked at, and a scope nobody returns to stops
//! mattering as soon as the horizon behind it is filled.
//!
//! # `last_worked` is what stops the first scope starving the second
//!
//! One group per tick is deliberately small (~1 point), so a worker that
//! always picked the same scope would finish it before touching another.
//! Ordering by `last_worked` with nulls first means a newly opened scope
//! is served next, and thereafter the least recently advanced one is.

use super::schema::StoreError;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

/// A scope the worker may walk.
#[derive(Debug, Clone, PartialEq)]
pub struct BackfillScope {
    /// `StatsQuery::cache_key` with `@me` resolved -- the same string
    /// `pr_history` and `pr_slice` are keyed on, so the three tables
    /// cannot disagree about which question they describe.
    pub scope_key: String,
    /// `repo` | `org` | `user` | `all`, and its value: exactly the
    /// scalars `stats_board` takes, so the worker reconstructs the same
    /// `Scope` the user's click made rather than a parallel encoding of
    /// it.
    pub scope_kind: String,
    pub scope_value: String,
    /// `merged` | `opened`.
    ///
    /// Recorded because the two are SEPARATE backfills over the same
    /// days: a row about merges says nothing about openings. #1094 notes
    /// this is latent today -- the page loads only `merged` -- and that a
    /// later change would quietly double the background spend. Storing
    /// the measure is what makes that visible rather than surprising.
    pub measure: String,
    /// How many days back this scope is walked.
    pub horizon_days: u32,
}

/// Register a scope, or refresh its `last_seen`.
///
/// Called when a user loads a board. `INSERT OR REPLACE` on `scope_key`,
/// preserving `last_worked` -- a re-opened scope must not lose its place
/// in the rotation and jump the queue ahead of scopes that have been
/// waiting.
pub fn note_seen(
    conn: &Connection,
    scope: &BackfillScope,
    seen_at: DateTime<Utc>,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO pr_backfill_scope
           (scope_key, scope_kind, scope_value, measure, horizon_days, last_seen, last_worked)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)
         ON CONFLICT(scope_key) DO UPDATE SET
            scope_kind = excluded.scope_kind,
            scope_value = excluded.scope_value,
            measure = excluded.measure,
            horizon_days = MAX(horizon_days, excluded.horizon_days),
            last_seen = excluded.last_seen",
        params![
            scope.scope_key,
            scope.scope_kind,
            scope.scope_value,
            scope.measure,
            scope.horizon_days as i64,
            seen_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// The scope the worker should advance next, or `None` when none is
/// registered.
///
/// Least recently worked first, nulls first, so a newly opened scope is
/// served before one already being advanced and no scope can starve.
/// `last_seen DESC` breaks the tie toward the scope the user looked at
/// most recently, which is the one they are most likely still watching.
pub fn next_to_work(conn: &Connection) -> Result<Option<BackfillScope>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT scope_key, scope_kind, scope_value, measure, horizon_days
           FROM pr_backfill_scope
          ORDER BY last_worked IS NOT NULL, last_worked ASC, last_seen DESC
          LIMIT 1",
    )?;
    let mut rows = stmt.query_map([], |r| {
        Ok(BackfillScope {
            scope_key: r.get(0)?,
            scope_kind: r.get(1)?,
            scope_value: r.get(2)?,
            measure: r.get(3)?,
            horizon_days: r.get::<_, i64>(4)?.max(0) as u32,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Record that the worker advanced this scope.
///
/// Written whether or not the attempt retrieved anything, which is what
/// makes the rotation move: a scope whose tick failed must still yield to
/// the next one, or a persistently failing scope holds the worker forever.
pub fn note_worked(
    conn: &Connection,
    scope_key: &str,
    worked_at: DateTime<Utc>,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE pr_backfill_scope SET last_worked = ?2 WHERE scope_key = ?1",
        params![scope_key, worked_at.to_rfc3339()],
    )?;
    Ok(())
}

/// Drop every registered scope.
///
/// Goes with `pr_history::clear` and `pr_slice::clear`, in the same place
/// and for the same event: the identity behind `@me` changed, so a
/// `scope_key` resolved against the previous login describes a question
/// nobody here asked.
pub fn clear(conn: &Connection) -> Result<usize, StoreError> {
    Ok(conn.execute("DELETE FROM pr_backfill_scope", [])?)
}

/// How many scopes are registered.
pub fn total_rows(conn: &Connection) -> Result<usize, StoreError> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pr_backfill_scope", [], |r| r.get(0))?;
    Ok(n.max(0) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::migrate;
    use chrono::TimeZone;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, 12, 0, 0).unwrap()
    }

    fn scope(key: &str, value: &str) -> BackfillScope {
        BackfillScope {
            scope_key: key.into(),
            scope_kind: "org".into(),
            scope_value: value.into(),
            measure: "merged".into(),
            horizon_days: 30,
        }
    }

    /// The table is actually written to and read back.
    #[test]
    fn the_table_is_actually_written_to() {
        let conn = db();
        note_seen(&conn, &scope("k1", "X"), at(1)).unwrap();
        assert_eq!(total_rows(&conn).unwrap(), 1);
        assert_eq!(next_to_work(&conn).unwrap().unwrap().scope_value, "X");
    }

    /// With nothing registered the worker has nothing to do, and says so
    /// rather than inventing a scope.
    #[test]
    fn no_registered_scope_means_no_work() {
        let conn = db();
        assert_eq!(next_to_work(&conn).unwrap(), None);
    }

    /// A newly opened scope is served before one already being advanced,
    /// so no scope starves behind another.
    #[test]
    fn the_least_recently_worked_scope_is_served_first() {
        let conn = db();
        note_seen(&conn, &scope("k1", "X"), at(1)).unwrap();
        note_worked(&conn, "k1", at(2)).unwrap();
        note_seen(&conn, &scope("k2", "Y"), at(3)).unwrap();

        assert_eq!(
            next_to_work(&conn).unwrap().unwrap().scope_key,
            "k2",
            "a scope never worked must come before one already advanced"
        );
        note_worked(&conn, "k2", at(4)).unwrap();
        assert_eq!(
            next_to_work(&conn).unwrap().unwrap().scope_key,
            "k1",
            "then the least recently advanced one"
        );
    }

    /// Re-opening a scope refreshes `last_seen` and does NOT reset its
    /// place in the rotation -- otherwise a scope the user keeps
    /// revisiting would starve every other one.
    #[test]
    fn re_opening_a_scope_does_not_jump_the_queue() {
        let conn = db();
        note_seen(&conn, &scope("k1", "X"), at(1)).unwrap();
        note_worked(&conn, "k1", at(2)).unwrap();
        note_seen(&conn, &scope("k2", "Y"), at(3)).unwrap();
        note_worked(&conn, "k2", at(4)).unwrap();

        // The user re-opens the scope that was worked most recently.
        note_seen(&conn, &scope("k2", "Y"), at(5)).unwrap();
        assert_eq!(
            next_to_work(&conn).unwrap().unwrap().scope_key,
            "k1",
            "re-opening must not clear `last_worked`; the other scope is \
             still the one that has waited longest"
        );
    }

    /// A wider horizon wins. A user who asked for 30 days after asking for
    /// 7 must not have the longer walk silently shortened back.
    #[test]
    fn the_widest_horizon_asked_for_is_kept() {
        let conn = db();
        let mut s = scope("k1", "X");
        s.horizon_days = 30;
        note_seen(&conn, &s, at(1)).unwrap();
        s.horizon_days = 7;
        note_seen(&conn, &s, at(2)).unwrap();
        assert_eq!(next_to_work(&conn).unwrap().unwrap().horizon_days, 30);
    }

    /// The measure is part of the identity: merged and opened are
    /// separate backfills over the same days.
    #[test]
    fn the_two_measures_are_separate_scopes() {
        let conn = db();
        let mut merged = scope("board|merged|*|org:X", "X");
        merged.measure = "merged".into();
        let mut opened = scope("board|opened|*|org:X", "X");
        opened.measure = "opened".into();
        note_seen(&conn, &merged, at(1)).unwrap();
        note_seen(&conn, &opened, at(1)).unwrap();
        assert_eq!(
            total_rows(&conn).unwrap(),
            2,
            "a row about merges says nothing about openings"
        );
    }

    /// `clear` empties the table, for the identity-change event.
    #[test]
    fn clear_drops_every_row() {
        let conn = db();
        note_seen(&conn, &scope("k1", "X"), at(1)).unwrap();
        assert_eq!(clear(&conn).unwrap(), 1);
        assert_eq!(total_rows(&conn).unwrap(), 0);
    }
}
