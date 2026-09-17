//! The ledger: which date ranges have been retrieved, which were refused,
//! and -- by their ABSENCE -- which have never been asked for (#1092,
//! #1093, design #1094).
//!
//! # The three-way question
//!
//! [`super::pr_history`] stores pull requests. It cannot say whether a
//! range is COMPLETE, because "no rows for 2026-08-14" is equally
//! consistent with a quiet day and a day nobody asked about. This module
//! is what separates them:
//!
//! | The reader's question | The answer here |
//! |---|---|
//! | "we hold this range complete" | a row with [`SliceState::Complete`] |
//! | "we asked and GitHub could not" | [`SliceState::Refused`] / [`SliceState::Irreducible`] |
//! | **"we have never asked"** | **no row at all** |
//!
//! # Absent is not zero
//!
//! The last line is the whole point, and it is where this feature is most
//! likely to ship a defect. A day with no row is **uncovered**. It is not
//! a measured zero, and nothing may render it as one -- the root
//! `CLAUDE.md` states the rule, and #846 is the time this repo shipped its
//! violation. [`coverage`] therefore returns which days ARE covered rather
//! than a count of pull requests, so a caller cannot reach a zero without
//! first passing a day list that says whether the zero was measured.
//!
//! # There is no `pending` state
//!
//! In-flight is process state, not durable state. A `pending` row written
//! before a fetch would survive a crash mid-tick with nothing left to move
//! it out, and every later tick would skip the range forever -- #1042's
//! trap exactly, where a column skeletoned indefinitely because nothing
//! advanced it out of Pending.
//!
//! So a slice being worked on right now has **no row**, and reads as
//! uncovered. That is true while the work is in flight, and it
//! self-corrects the instant the work lands. A crash costs one re-fetch of
//! one slice -- measured at ~1 point for five of them.
//!
//! # A row and its pull requests land together
//!
//! [`record_with_rows`] writes both in ONE transaction. Two commits would
//! leave a window in which the ledger claims a range is retrieved and the
//! rows are not yet there; a reader in that window gets a confident,
//! wrong, and permanent answer, because nothing would ever revisit a range
//! the ledger says is done.

use super::pr_history::StoredPr;
use super::schema::StoreError;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rusqlite::{params, Connection};
use std::collections::BTreeSet;

/// What happened when a range was asked for.
///
/// No `Pending`: see the module docs. Every variant here is a DURABLE
/// fact about a completed attempt, which is what makes a crash mid-tick
/// leave nothing behind rather than a row nothing can clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceState {
    /// Every pull request GitHub reported for the range was retrieved.
    Complete,
    /// The range was fetched and came back short, or GitHub refused
    /// fields on it. The rows that did arrive are stored and are a floor;
    /// the range is worth asking again.
    Refused,
    /// A single day holding more than GitHub will return. The date
    /// grammar has no finer unit (`slice.rs`), so this cannot be fixed by
    /// subdividing and re-asking is pointless -- which is exactly why it
    /// is distinct from [`SliceState::Refused`] rather than folded into
    /// it. A worker that retried this forever would spend the budget on a
    /// question that has no better answer.
    Irreducible,
}

impl SliceState {
    /// The stored spelling.
    ///
    /// Text rather than an integer so a human reading the database with
    /// `sqlite3` sees the state, and so an unknown future variant is
    /// legible rather than an unexplained `4`.
    pub fn as_str(self) -> &'static str {
        match self {
            SliceState::Complete => "complete",
            SliceState::Refused => "refused",
            SliceState::Irreducible => "irreducible",
        }
    }

    /// Parse a stored spelling.
    ///
    /// An unrecognised value reads as [`SliceState::Refused`] -- the
    /// state that invites a re-fetch. A row this process cannot interpret
    /// must never read as `Complete`, which would make the worker skip a
    /// range on the strength of a value it does not understand. Erring
    /// toward asking again costs points; erring toward `Complete` costs
    /// correctness permanently.
    fn parse(s: &str) -> Self {
        match s {
            "complete" => SliceState::Complete,
            "irreducible" => SliceState::Irreducible,
            _ => SliceState::Refused,
        }
    }

    /// Whether a worker should leave this range alone.
    ///
    /// `Complete` because there is nothing more to get, `Irreducible`
    /// because there is nothing better to get. `Refused` is the one state
    /// worth spending a request on again.
    pub fn settled(self) -> bool {
        matches!(self, SliceState::Complete | SliceState::Irreducible)
    }
}

/// One range, and what asking about it produced.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceRow {
    pub from: String,
    pub to: String,
    pub state: SliceState,
    /// GitHub's own `issueCount` for the range.
    ///
    /// Exact even when retrieval came back short: the 1,000-result cap
    /// limits what can be RETRIEVED, never what is counted (`slice.rs`).
    /// So summing this across covered ranges gives an exact denominator
    /// for those ranges -- which is what lets a board say "1,240 of 2,942"
    /// rather than "1,240 of an unknown number".
    pub issue_count: u64,
    /// How many pull requests were actually stored from the range.
    pub retrieved: u64,
    /// Fields GitHub refused across the responses for this range.
    pub refused_fields: u64,
}

impl SliceRow {
    /// Whether this range's rows are all present.
    ///
    /// Both halves, because either alone lies: a `Complete` state with
    /// fewer rows than GitHub counted is a claim contradicted by its own
    /// numbers, and a full row count under a `Refused` state still had
    /// fields refused on it.
    pub fn is_whole(&self) -> bool {
        self.state == SliceState::Complete
            && self.retrieved >= self.issue_count
            && self.refused_fields == 0
    }
}

/// Record a completed attempt at one range, with the rows it produced,
/// atomically.
///
/// **The single transaction is the point.** The ledger claims a range is
/// retrieved; the rows are the evidence for that claim. Committed
/// separately there is a window in which the claim is true and the
/// evidence is missing -- and because nothing revisits a range the ledger
/// calls settled, a reader who lands in that window gets a permanently
/// wrong answer rather than a temporarily incomplete one.
///
/// Returns how many pull request rows were written.
pub fn record_with_rows(
    conn: &mut Connection,
    scope_key: &str,
    row: &SliceRow,
    prs: &[StoredPr],
    measured_at: DateTime<Utc>,
) -> Result<usize, StoreError> {
    let tx = conn.transaction()?;
    let n = super::pr_history::put_many_in(&tx, scope_key, &row.from, &row.to, prs, measured_at)?;
    put_in(&tx, scope_key, row, measured_at)?;
    tx.commit()?;
    Ok(n)
}

/// Record a whole load: its rows, and the claims about the days they
/// came from, atomically.
///
/// The foreground counterpart to [`record_with_rows`], which records ONE
/// range. A foreground load fetches many ranges at once and must write
/// them under a single commit for the same reason: the ledger claims a
/// range is retrieved and the rows are the evidence, and a reader landing
/// between two commits gets a permanently wrong answer rather than a
/// temporarily incomplete one.
///
/// Rows are written once for the whole window rather than per range, so
/// a pull request whose range earned no ledger row -- a wide slice, or an
/// alias that never answered -- is still stored. Dropping those would
/// lose pull requests that were already paid for, which is the failure
/// #1004 exists to prevent.
///
/// Returns how many pull request rows were written.
pub fn record_all_with_rows(
    conn: &mut Connection,
    scope_key: &str,
    window_from: &str,
    window_to: &str,
    rows: &[SliceRow],
    prs: &[StoredPr],
    measured_at: DateTime<Utc>,
) -> Result<usize, StoreError> {
    let tx = conn.transaction()?;
    let n =
        super::pr_history::put_many_in(&tx, scope_key, window_from, window_to, prs, measured_at)?;
    for row in rows {
        put_in(&tx, scope_key, row, measured_at)?;
    }
    tx.commit()?;
    Ok(n)
}

/// Record a completed attempt at one range, with no rows.
///
/// For the probe pass, which learns a range's `issue_count` before
/// fetching anything from it. The state such a row carries is
/// [`SliceState::Refused`] or [`SliceState::Irreducible`] -- never
/// `Complete`, because nothing has been retrieved yet and a `Complete`
/// row with no rows behind it is the lying ledger this module exists to
/// prevent.
pub fn put(
    conn: &Connection,
    scope_key: &str,
    row: &SliceRow,
    measured_at: DateTime<Utc>,
) -> Result<(), StoreError> {
    put_in(conn, scope_key, row, measured_at)
}

/// The write itself, against anything that can execute SQL.
///
/// `INSERT OR REPLACE`: a later attempt at a range supersedes an earlier
/// one, which is how a `Refused` range becomes `Complete` when a
/// subsequent fetch gets the rest of it.
fn put_in(
    conn: &Connection,
    scope_key: &str,
    row: &SliceRow,
    measured_at: DateTime<Utc>,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO pr_slice
           (scope_key, slice_from, slice_to, state, issue_count, retrieved,
            refused_fields, measured_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            scope_key,
            row.from,
            row.to,
            row.state.as_str(),
            row.issue_count as i64,
            row.retrieved as i64,
            row.refused_fields as i64,
            measured_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Every ledger row for one scope overlapping `[from, to]`.
///
/// Overlap rather than containment: a month-sized slice recorded for a
/// sparse scope covers days inside a 30-day window that the slice itself
/// extends beyond, and requiring containment would report those days
/// uncovered and re-fetch them.
pub fn rows_overlapping(
    conn: &Connection,
    scope_key: &str,
    from: &str,
    to: &str,
) -> Result<Vec<SliceRow>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT slice_from, slice_to, state, issue_count, retrieved, refused_fields
           FROM pr_slice
          WHERE scope_key = ?1 AND slice_from <= ?3 AND slice_to >= ?2
          ORDER BY slice_from, slice_to",
    )?;
    let rows = stmt.query_map(params![scope_key, from, to], |r| {
        Ok(SliceRow {
            from: r.get(0)?,
            to: r.get(1)?,
            state: SliceState::parse(&r.get::<_, String>(2)?),
            issue_count: r.get::<_, i64>(3)?.max(0) as u64,
            retrieved: r.get::<_, i64>(4)?.max(0) as u64,
            refused_fields: r.get::<_, i64>(5)?.max(0) as u64,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// What is known about a window: which days are covered, and the exact
/// total for them.
///
/// # Why days and not a percentage
///
/// A pull request count cannot distinguish "40% of every day" from "100%
/// of 40% of the days", and the second tells the reader WHICH PART of the
/// chart to trust. #1094 asks for the distinction by name; it is also the
/// only form in which absent-is-not-zero survives a round trip, because a
/// day that is not in [`Coverage::days`] is one nothing may draw a bar
/// for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Coverage {
    /// The days of the window a settled ledger row covers, `YYYY-MM-DD`.
    ///
    /// Sorted and unique. A day NOT in here has never been successfully
    /// retrieved -- it is uncovered, and is not a zero.
    pub days: Vec<String>,
    /// GitHub's exact total across the covered ranges.
    ///
    /// `None` when nothing is covered: a denominator nobody has measured
    /// must never render as 0, which would make "400 collected" read as
    /// "400 of 0" or, worse, complete. This is the `Option` #1094 requires
    /// and the reason `Board::total` became one.
    pub total: Option<u64>,
    /// How many pull requests are held across the covered ranges.
    pub retrieved: u64,
    /// Whether any covered range came back short or refused fields.
    pub partial: bool,
}

impl Coverage {
    /// How many days of the window are covered.
    pub fn days_covered(&self) -> usize {
        self.days.len()
    }
}

/// Which days of `[from, to]` the ledger covers, and their exact total.
///
/// Only SETTLED rows contribute days. A `Refused` range has rows and they
/// are real, but the range is not covered -- its days stay uncovered so
/// the worker revisits them and the UI does not claim them. That is the
/// conservative direction on purpose: over-claiming coverage renders a
/// partial day as a whole one, which is the silent wrongness this whole
/// design is against.
///
/// `issue_count` is summed over settled rows only, for the same reason and
/// with a second one: a refused range's count is exact but its rows are
/// not, so adding it to the denominator without adding its pull requests
/// to the numerator would make the shortfall look larger than it is.
pub fn coverage(
    conn: &Connection,
    scope_key: &str,
    from: &str,
    to: &str,
) -> Result<Coverage, StoreError> {
    let rows = rows_overlapping(conn, scope_key, from, to)?;
    Ok(coverage_from(&rows, from, to))
}

/// [`coverage`] over rows already in hand.
///
/// Split out so the day arithmetic -- the part that can be wrong in a way
/// a database cannot -- is testable without one.
pub fn coverage_from(rows: &[SliceRow], from: &str, to: &str) -> Coverage {
    let mut days: BTreeSet<String> = BTreeSet::new();
    let mut total: u64 = 0;
    let mut retrieved: u64 = 0;
    let mut partial = false;
    let mut any = false;
    for row in rows {
        if !row.state.settled() {
            // Not covered, and deliberately not counted either way. Its
            // days stay uncovered so they are revisited.
            partial = true;
            continue;
        }
        any = true;
        total = total.saturating_add(row.issue_count);
        retrieved = retrieved.saturating_add(row.retrieved);
        if !row.is_whole() {
            partial = true;
        }
        for day in clamped_days(&row.from, &row.to, from, to) {
            days.insert(day);
        }
    }
    Coverage {
        days: days.into_iter().collect(),
        // `None` rather than 0 when nothing settled. The distinction is
        // the feature: a board with rows and an unknown denominator must
        // say so rather than print a total it has not measured.
        total: any.then_some(total),
        retrieved,
        partial,
    }
}

/// The days of `[row_from, row_to]` that fall inside `[from, to]`.
///
/// Clamped rather than taken whole: a month-sized slice overlapping a
/// 30-day window covers only the days inside it, and counting the rest
/// would report more days covered than the window has -- a coverage figure
/// over 100%, which is a number the reader cannot act on.
///
/// An unparseable date yields NO days rather than a guess. A range this
/// cannot read must not contribute coverage it cannot verify; the days
/// then stay uncovered and are re-fetched, which is the safe direction.
fn clamped_days(row_from: &str, row_to: &str, from: &str, to: &str) -> Vec<String> {
    let (Some(rf), Some(rt), Some(wf), Some(wt)) = (
        parse_day(row_from),
        parse_day(row_to),
        parse_day(from),
        parse_day(to),
    ) else {
        return Vec::new();
    };
    let start = rf.max(wf);
    let end = rt.min(wt);
    let mut out = Vec::new();
    let mut d = start;
    while d <= end {
        out.push(d.to_string());
        d += Duration::days(1);
    }
    out
}

fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// Every day of `[from, to]` with no settled ledger row covering it.
///
/// What the worker asks for next, and the direct expression of
/// absent-is-not-zero: a day is returned because nothing says it was
/// retrieved, never because something said it was empty.
///
/// Oldest first, so a horizon is walked back from the edge of what is
/// known rather than in an order the user cannot predict.
pub fn uncovered_days(
    conn: &Connection,
    scope_key: &str,
    from: &str,
    to: &str,
) -> Result<Vec<String>, StoreError> {
    let covered = coverage(conn, scope_key, from, to)?;
    let held: BTreeSet<&str> = covered.days.iter().map(String::as_str).collect();
    Ok(clamped_days(from, to, from, to)
        .into_iter()
        .filter(|d| !held.contains(d.as_str()))
        .collect())
}

/// Drop every ledger row.
///
/// Goes with `pr_history::clear`, in the same place and for the same
/// event. Clearing the rows without the ledger would leave it claiming
/// ranges are retrieved whose pull requests are gone -- and because the
/// worker skips settled ranges, those days would never be re-fetched. A
/// ledger that lies is worse than no ledger.
pub fn clear(conn: &Connection) -> Result<usize, StoreError> {
    Ok(conn.execute("DELETE FROM pr_slice", [])?)
}

/// Total ledger rows, across every scope.
pub fn total_rows(conn: &Connection) -> Result<usize, StoreError> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pr_slice", [], |r| r.get(0))?;
    Ok(n.max(0) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::migrate;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    fn row(from: &str, to: &str, state: SliceState, count: u64, retrieved: u64) -> SliceRow {
        SliceRow {
            from: from.into(),
            to: to.into(),
            state,
            issue_count: count,
            retrieved,
            refused_fields: 0,
        }
    }

    fn pr(repo: &str, number: u64, day: &str) -> StoredPr {
        StoredPr {
            repo: repo.into(),
            number,
            merged_at: day.into(),
            title: format!("PR {number}"),
            url: format!("https://github.com/{repo}/pull/{number}"),
            author: "alice".into(),
            cycle_time_hours: 1.0,
            size: 10,
            additions: 6,
            deletions: 4,
            changed_files: 3,
            reviews_received: 1,
        }
    }

    /// The table is actually written to and read back.
    ///
    /// Named for the reason `stats::the_table_is_actually_written_to` is:
    /// `merge_history` sat empty on every install, and a permanently-empty
    /// table implying a feature that does not exist is a failure this repo
    /// has already shipped once.
    #[test]
    fn the_table_is_actually_written_to() {
        let mut conn = db();
        let n = record_with_rows(
            &mut conn,
            "board|merged|*|org:X",
            &row("2026-08-01", "2026-08-01", SliceState::Complete, 2, 2),
            &[pr("o/a", 1, "2026-08-01"), pr("o/b", 2, "2026-08-01")],
            Utc::now(),
        )
        .unwrap();
        assert_eq!(n, 2);
        assert_eq!(total_rows(&conn).unwrap(), 1);
    }

    /// **THE load-bearing property.** A day with no row is UNCOVERED, and
    /// is never reported as a measured zero.
    ///
    /// This is the defect this feature is most likely to ship, so it is
    /// asserted directly rather than inferred from a coverage percentage:
    /// the uncovered day is absent from `days`, and the denominator for a
    /// window nothing covers is `None` rather than 0.
    #[test]
    fn a_day_with_no_row_is_uncovered_and_not_a_zero() {
        let conn = db();
        let key = "board|merged|*|org:X";

        // Nothing asked at all.
        let cov = coverage(&conn, key, "2026-08-01", "2026-08-03").unwrap();
        assert!(cov.days.is_empty(), "no row means no day is covered");
        assert_eq!(
            cov.total, None,
            "a denominator nobody measured must be None, never 0 -- \
             a 0 renders as `400 of 0` or as complete"
        );
        assert_eq!(
            uncovered_days(&conn, key, "2026-08-01", "2026-08-03")
                .unwrap()
                .len(),
            3
        );

        // One day of three measured, and genuinely empty.
        put(
            &conn,
            key,
            &row("2026-08-02", "2026-08-02", SliceState::Complete, 0, 0),
            Utc::now(),
        )
        .unwrap();
        let cov = coverage(&conn, key, "2026-08-01", "2026-08-03").unwrap();
        assert_eq!(
            cov.days,
            vec!["2026-08-02"],
            "only the measured day is covered"
        );
        assert_eq!(
            cov.total,
            Some(0),
            "a MEASURED zero is Some(0) -- distinct from the unmeasured None above"
        );
        assert_eq!(
            uncovered_days(&conn, key, "2026-08-01", "2026-08-03").unwrap(),
            vec!["2026-08-01", "2026-08-03"],
            "the days nothing asked about stay uncovered"
        );
    }

    /// A measured zero and an unmeasured day are DIFFERENT, and the
    /// difference survives a round trip through the database.
    ///
    /// Stated separately from the test above because this is the pair a
    /// renderer branches on, and collapsing them is what #846 shipped.
    #[test]
    fn a_measured_zero_is_distinguishable_from_an_unasked_day() {
        let conn = db();
        let key = "k";
        put(
            &conn,
            key,
            &row("2026-08-01", "2026-08-01", SliceState::Complete, 0, 0),
            Utc::now(),
        )
        .unwrap();
        let measured = coverage(&conn, key, "2026-08-01", "2026-08-01").unwrap();
        let unasked = coverage(&conn, key, "2026-08-05", "2026-08-05").unwrap();

        assert_eq!(measured.total, Some(0));
        assert_eq!(unasked.total, None);
        assert_eq!(measured.days_covered(), 1);
        assert_eq!(unasked.days_covered(), 0);
        assert_ne!(
            measured, unasked,
            "a day measured at zero and a day never asked about must not \
             be the same value"
        );
    }

    /// There is no `pending` state to get stuck in: a range being worked
    /// on has NO row, so a crash mid-tick leaves nothing behind.
    ///
    /// Asserted as the property a crash would violate -- after a write
    /// that never happened, the range is still offered as work -- rather
    /// than by asserting an enum has no variant, which the type system
    /// already does.
    #[test]
    fn an_interrupted_attempt_leaves_no_row_to_get_stuck() {
        let conn = db();
        let key = "k";
        // A tick begins here. Nothing is written before the fetch, by
        // construction: `record_with_rows` is the only write and it runs
        // after. Simulate the crash by simply never calling it.
        let after_crash = uncovered_days(&conn, key, "2026-08-01", "2026-08-02").unwrap();
        assert_eq!(
            after_crash,
            vec!["2026-08-01", "2026-08-02"],
            "a crashed tick must leave its range still offered as work; a \
             `pending` row would leave it claimed forever (#1042)"
        );
        assert_eq!(total_rows(&conn).unwrap(), 0);
    }

    /// The ledger row and its pull requests land in ONE transaction, so
    /// the ledger can never claim a range whose rows are absent.
    #[test]
    fn the_ledger_and_its_rows_land_together() {
        let mut conn = db();
        let key = "k";
        record_with_rows(
            &mut conn,
            key,
            &row("2026-08-01", "2026-08-01", SliceState::Complete, 2, 2),
            &[pr("o/a", 1, "2026-08-01"), pr("o/a", 2, "2026-08-01")],
            Utc::now(),
        )
        .unwrap();
        let cov = coverage(&conn, key, "2026-08-01", "2026-08-01").unwrap();
        assert_eq!(cov.total, Some(2));
        assert_eq!(
            crate::store::pr_history::count(&conn, key, "2026-08-01", "2026-08-01").unwrap(),
            2,
            "the rows the ledger claims must actually be there"
        );
    }

    /// **A failed write rolls BOTH back**: no rows, and no claim about
    /// them.
    ///
    /// Driven through a real constraint violation -- a `NOT NULL` column
    /// fed a NULL by a trigger armed for this test -- so what is exercised
    /// is the transaction the production path uses, not a source-position
    /// check that would pass over code nothing runs.
    ///
    /// The direction matters. Rows without a claim are merely
    /// unacknowledged work, and the next tick re-fetches them. A claim
    /// without rows is a ledger that LIES, and because nothing revisits a
    /// range the ledger calls settled, the lie is permanent.
    #[test]
    fn a_failed_write_leaves_neither_the_rows_nor_the_claim() {
        let mut conn = db();
        let key = "k";
        // Make the LEDGER write fail, after the rows have been inserted
        // inside the same transaction. A trigger is the one way to force a
        // failure at exactly that point without reaching into the module.
        conn.execute_batch(
            "CREATE TRIGGER refuse_ledger BEFORE INSERT ON pr_slice
             BEGIN SELECT RAISE(ABORT, 'forced'); END;",
        )
        .unwrap();

        let err = record_with_rows(
            &mut conn,
            key,
            &row("2026-08-01", "2026-08-01", SliceState::Complete, 2, 2),
            &[pr("o/a", 1, "2026-08-01"), pr("o/a", 2, "2026-08-01")],
            Utc::now(),
        );
        assert!(err.is_err(), "the forced failure must surface");

        assert_eq!(
            total_rows(&conn).unwrap(),
            0,
            "no claim survives a failed write"
        );
        assert_eq!(
            crate::store::pr_history::count(&conn, key, "2026-08-01", "2026-08-01").unwrap(),
            0,
            "and neither do the rows it was writing -- one transaction, or \
             the ledger can describe a state the table is not in"
        );
        // And the range is still offered as work, which is the
        // user-visible consequence of rolling back correctly.
        assert_eq!(
            uncovered_days(&conn, key, "2026-08-01", "2026-08-01").unwrap(),
            vec!["2026-08-01"]
        );
    }

    /// A refused range does NOT count as covered: its days stay uncovered
    /// so the worker returns to them.
    #[test]
    fn a_refused_range_is_not_covered() {
        let conn = db();
        let key = "k";
        put(
            &conn,
            key,
            &row("2026-08-01", "2026-08-02", SliceState::Refused, 100, 40),
            Utc::now(),
        )
        .unwrap();
        let cov = coverage(&conn, key, "2026-08-01", "2026-08-02").unwrap();
        assert!(cov.days.is_empty(), "a refused range covers no days");
        assert_eq!(
            cov.total, None,
            "a refused range's count must not become a denominator its \
             rows cannot match"
        );
        assert!(cov.partial);
        assert_eq!(
            uncovered_days(&conn, key, "2026-08-01", "2026-08-02")
                .unwrap()
                .len(),
            2
        );
    }

    /// An irreducible range IS covered -- re-asking cannot improve it --
    /// but the window is still flagged partial.
    #[test]
    fn an_irreducible_range_is_covered_but_partial() {
        let conn = db();
        let key = "k";
        put(
            &conn,
            key,
            &row(
                "2026-08-01",
                "2026-08-01",
                SliceState::Irreducible,
                1200,
                1000,
            ),
            Utc::now(),
        )
        .unwrap();
        let cov = coverage(&conn, key, "2026-08-01", "2026-08-01").unwrap();
        assert_eq!(cov.days, vec!["2026-08-01"]);
        assert_eq!(
            cov.total,
            Some(1200),
            "the count is exact even over the cap"
        );
        assert_eq!(cov.retrieved, 1000);
        assert!(
            cov.partial,
            "a day whose nodes are a sample by construction must stay flagged"
        );
        assert!(
            uncovered_days(&conn, key, "2026-08-01", "2026-08-01")
                .unwrap()
                .is_empty(),
            "re-asking an irreducible day cannot help, so it is not offered as work"
        );
    }

    /// A range wider than the window contributes only the days inside it.
    ///
    /// A month-sized slice over a 30-day window must not report more days
    /// covered than the window holds -- a coverage figure over 100% is a
    /// number the reader cannot act on.
    #[test]
    fn a_wide_range_is_clamped_to_the_window() {
        let conn = db();
        let key = "k";
        put(
            &conn,
            key,
            &row("2026-07-01", "2026-07-31", SliceState::Complete, 12, 12),
            Utc::now(),
        )
        .unwrap();
        let cov = coverage(&conn, key, "2026-07-30", "2026-08-02").unwrap();
        assert_eq!(
            cov.days,
            vec!["2026-07-30", "2026-07-31"],
            "only the overlap counts"
        );
        assert_eq!(
            uncovered_days(&conn, key, "2026-07-30", "2026-08-02").unwrap(),
            vec!["2026-08-01", "2026-08-02"]
        );
    }

    /// Two windows over the same days SHARE the ledger -- which is the
    /// whole re-key. A 30-day question and a 90-day question must not
    /// duplicate each other's work.
    #[test]
    fn a_second_window_reads_the_first_windows_work() {
        let mut conn = db();
        let key = "k";
        // The 7-day board retrieves three days.
        for day in ["2026-08-01", "2026-08-02", "2026-08-03"] {
            record_with_rows(
                &mut conn,
                key,
                &row(day, day, SliceState::Complete, 1, 1),
                &[pr("o/a", day.replace('-', "").parse().unwrap(), day)],
                Utc::now(),
            )
            .unwrap();
        }
        // A 30-day board over a range CONTAINING those days finds them
        // already covered, and has only the rest left to ask for.
        let left = uncovered_days(&conn, key, "2026-08-01", "2026-08-05").unwrap();
        assert_eq!(
            left,
            vec!["2026-08-04", "2026-08-05"],
            "the wider window must not re-request days the narrower one banked"
        );
        let cov = coverage(&conn, key, "2026-08-01", "2026-08-05").unwrap();
        assert_eq!(cov.days_covered(), 3);
        assert_eq!(cov.total, Some(3));
    }

    /// A later attempt supersedes an earlier one, so a refused range can
    /// become complete.
    #[test]
    fn a_later_attempt_supersedes_an_earlier_one() {
        let conn = db();
        let key = "k";
        put(
            &conn,
            key,
            &row("2026-08-01", "2026-08-01", SliceState::Refused, 10, 4),
            Utc::now(),
        )
        .unwrap();
        put(
            &conn,
            key,
            &row("2026-08-01", "2026-08-01", SliceState::Complete, 10, 10),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(total_rows(&conn).unwrap(), 1, "one range, one row");
        let cov = coverage(&conn, key, "2026-08-01", "2026-08-01").unwrap();
        assert_eq!(cov.total, Some(10));
        assert_eq!(cov.retrieved, 10);
        assert!(!cov.partial);
    }

    /// Rows do not leak between scopes. Two accounts share this database.
    #[test]
    fn rows_do_not_leak_across_scopes() {
        let conn = db();
        put(
            &conn,
            "board|merged|*|org:X",
            &row("2026-08-01", "2026-08-01", SliceState::Complete, 5, 5),
            Utc::now(),
        )
        .unwrap();
        let other = coverage(&conn, "board|merged|*|org:Y", "2026-08-01", "2026-08-01").unwrap();
        assert!(other.days.is_empty());
        assert_eq!(other.total, None);
    }

    /// A state this build does not recognise reads as `Refused` -- never
    /// as `Complete`.
    ///
    /// The direction matters: an unreadable row that read as complete
    /// would make the worker skip a range on the strength of a value it
    /// cannot interpret, permanently.
    #[test]
    fn an_unknown_state_invites_a_refetch_rather_than_claiming_completeness() {
        let conn = db();
        conn.execute(
            "INSERT INTO pr_slice (scope_key, slice_from, slice_to, state,
                issue_count, retrieved, refused_fields, measured_at)
             VALUES ('k','2026-08-01','2026-08-01','from-a-future-version',5,5,0,'2026-08-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let rows = rows_overlapping(&conn, "k", "2026-08-01", "2026-08-01").unwrap();
        assert_eq!(rows[0].state, SliceState::Refused);
        assert!(!rows[0].state.settled());
        assert!(coverage(&conn, "k", "2026-08-01", "2026-08-01")
            .unwrap()
            .days
            .is_empty());
    }

    /// `clear` empties the ledger, for the identity-change event.
    #[test]
    fn clear_drops_every_row() {
        let conn = db();
        put(
            &conn,
            "k",
            &row("2026-08-01", "2026-08-01", SliceState::Complete, 1, 1),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(clear(&conn).unwrap(), 1);
        assert_eq!(total_rows(&conn).unwrap(), 0);
    }

    /// An unparseable range contributes no coverage rather than a guess.
    #[test]
    fn an_unparseable_range_covers_nothing() {
        let rows = vec![row("not-a-date", "2026-08-01", SliceState::Complete, 3, 3)];
        let cov = coverage_from(&rows, "2026-08-01", "2026-08-01");
        assert!(
            cov.days.is_empty(),
            "a range whose dates cannot be read must not claim days"
        );
    }
}
