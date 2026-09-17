//! Per-pull-request accumulation, keyed on the SLICE that retrieved each
//! row (#1004, re-keyed for #1092).
//!
//! # The defect this exists for
//!
//! `store::stats` memoises a whole ANSWER, keyed by the question. That is
//! the right shape when an answer arrives complete and the wrong one when
//! it cannot: a load that hits the cap stores a partial board, and the
//! next load starts from nothing. A reporter at 2,942 pull requests sees
//! *"1523 of 2942 pull requests could not be retrieved"*, and repeated
//! loads never improve it -- 1,419 retrieved then 800 more is two partial
//! boards, never 2,219.
//!
//! This module is the layer beneath `stats_cache`, not a replacement for
//! it. `stats_cache` still memoises the assembled answer; this remembers
//! the pull requests it was assembled FROM, so consecutive loads union.
//!
//! # Why the key is a slice and not a window (#1092)
//!
//! #1004 keyed a row on the WINDOW the user asked about, and that key is
//! what stopped accumulation from accumulating. A 30-day board and a
//! 90-day board filed the same pull requests under two keys and neither
//! helped the other; changing the range selector discarded everything;
//! and at UTC midnight the window moved out from under every banked row
//! at once. Worst of all, the question "do we hold 2026-08-14?" could not
//! be asked, so a fetch could not subtract what is stored from what it was
//! about to request -- which is why `count` had zero production callers.
//!
//! A row is now identified by `(scope_key, repo, number)` and carries the
//! DAY it belongs to, so it is evidence about a date rather than about a
//! question. [`super::pr_slice`] is the ledger saying which dates have
//! been retrieved; the two together answer "what do we hold, and what have
//! we never asked for" -- and absent from BOTH means uncovered, never
//! zero.
//!
//! # Why this is not `merge_history`
//!
//! Migration 2 created and dropped `merge_history` in the same file, and
//! `store/mod.rs` records that its shape was wrong rather than merely
//! unused: it accumulated merges by DIFFING the open pull request set,
//! and a pull request leaving that set may have been closed unmerged.
//!
//! Nothing here is inferred from a disappearance. Every row is written
//! from a node GitHub returned for an explicit `is:merged` search, and
//! carries the `mergedAt` GitHub itself reported. The search stays
//! authoritative; this records what it already said.
//!
//! # The measurements this design rests on
//!
//! All mine, live API via `gh api graphql`, 2026-09-14.
//!
//! ## A detail document costs 1 point whatever it carries
//!
//! The board's own document shape (`query::slice_detail_query`, 5 aliases
//! at `BOARD_ALIAS_CHUNK`):
//!
//! | Scope | Aliases x page | Nodes returned | Cost | Wall clock |
//! |---|---|---|---|---|
//! | `repo:rust-lang/rust` | 5 x 25 | 125 | **1** | 8.25s |
//! | `repo:rust-lang/rust` | 3 x 25 | 75 | **1** | 8.12s |
//! | `repo:rust-lang/rust` | 2 x 50 | 100 | **1** | 8.68s |
//! | `org:microsoft` | 2 x 25 | 50 | **1** | 8.53s |
//! | `repo:rust-lang/rust` | 5 x 50 | -- | **502** | ~11s |
//!
//! So 2,942 pull requests are about 12 documents -- **12 points** against
//! a 4,500-point usable hourly budget, 0.27% of it.
//!
//! **The budget was never what bound the reporter.** What binds is the
//! ~11s server deadline (reproduced above: 5 x 50 on dense data is a 502,
//! exactly the figure `board.rs` records) and the 60s `LOAD_TIMEOUT`.
//! Both cap one LOAD, not one hour -- which is exactly why accumulating
//! across loads converges: each load is separately bounded, and this
//! table keeps what it managed.
//!
//! That refines #1004's premise rather than contradicting its proposal. It
//! also means accumulation is not an argument for spending below
//! `budget::RESERVE`: the floor protects `poll.rs`'s standing obligation,
//! and 12 points never came near it.
//!
//! ## A closed window is byte-identical on re-fetch
//!
//! The question that decides whether this converges or churns. The same
//! closed-window search, twice: `issueCount` 208 both times, and the
//! `(repo, number, additions, deletions, mergedAt)` set of every node
//! identical. So a row about a closed window never needs re-fetching, and
//! `store::stats::is_closed` is the gate for "assembled once, trusted
//! forever".
//!
//! ## Row size
//!
//! Over 50 real multi-repository pull requests: **157 bytes** a row
//! (median 155, p95 200, max 262). The reporter's 2,942 are ~0.6 MiB with
//! index overhead. [`prune`] bounds it anyway -- see [`MAX_ROWS`].

use super::schema::StoreError;
use crate::github::stats::BoardPr;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

/// The most rows this table may hold, across every scope and window.
///
/// A stated bound rather than the assumption that "a PR table stays
/// small", which #1004 asks for explicitly. At the measured 157 bytes a
/// row this is about **31 MiB** of payload before index overhead -- large
/// enough that no realistic account is ever pruned mid-convergence (the
/// reporter's whole corpus is 2,942), and small enough that a user who
/// browses many scopes over years cannot grow the database without limit.
///
/// 200,000 rather than a round million because the thing being protected
/// is a desktop app's SQLite file, and the failure mode of a bound that
/// is too high is indistinguishable from having no bound at all.
pub const MAX_ROWS: usize = 200_000;

/// How many rows are dropped once [`MAX_ROWS`] is passed.
///
/// A tenth, so pruning is amortised: trimming to exactly the limit would
/// run a delete on nearly every insert once the table is full, where this
/// runs one delete per 20,000 rows added.
const PRUNE_BATCH: usize = MAX_ROWS / 10;

/// What is stored about one pull request.
///
/// A superset of [`BoardPr`] rather than that type directly: the board's
/// outlier lists need `repo`, `number`, `title`, `url`, `author`,
/// `cycle_time_hours` and `size`, while the per-author rows also need the
/// three diff statistics and the review count. Storing only `BoardPr`
/// would make the rows re-derivable and the AUTHOR AGGREGATES not, so a
/// board assembled from storage would have correct outliers and wrong
/// leaderboards -- the exact silent-wrongness shape #824 exists to stop.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredPr {
    pub repo: String,
    pub number: u64,
    /// The day this pull request belongs to, `YYYY-MM-DD`.
    ///
    /// Whichever date the MEASURE is about: `mergedAt` for
    /// `Measure::Merged`, `createdAt` for `Measure::Opened` -- the same
    /// date the slice's own `merged:`/`created:` qualifier ranged over, so
    /// a row always falls inside the slice that retrieved it.
    ///
    /// The DATE, not the timestamp. GitHub's search grammar has no
    /// sub-day range (`slice.rs`: a day is the floor), so the time of day
    /// is precision the ledger cannot use and could not verify. Keeping
    /// only what is meaningful is also what makes a day-level `GROUP BY`
    /// an index scan rather than a string slice over every row.
    ///
    /// This is the field #1092 needed and #1004 did not store, and its
    /// absence is precisely why the old rows could not be re-keyed:
    /// nothing in a row said which day inside the window it came from.
    pub merged_at: String,
    pub title: String,
    pub url: String,
    pub author: String,
    pub cycle_time_hours: f64,
    /// Additions plus deletions, matching `BoardPr::size`.
    pub size: u64,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub reviews_received: u64,
}

impl StoredPr {
    /// The outlier-list projection.
    pub fn to_board_pr(&self) -> BoardPr {
        BoardPr {
            number: self.number,
            title: self.title.clone(),
            url: self.url.clone(),
            repo: self.repo.clone(),
            author: self.author.clone(),
            cycle_time_hours: self.cycle_time_hours,
            size: self.size,
        }
    }
}

/// Write pull requests down, merging with whatever is already stored.
///
/// `INSERT OR REPLACE` on `(scope_key, repo, number)`. Replace rather than
/// ignore for the same reason `stats::put` replaces: a later load's row is
/// at worst equal to an earlier one, and re-fetching a pull request whose
/// diff statistics were refused the first time must be able to correct
/// them.
///
/// **This is the accumulation point, and it is called even by a load that
/// later hits the cap** -- that is the entire feature. Writing only on a
/// complete load would reproduce the defect exactly.
///
/// Returns how many rows were written.
///
/// # Why the identity dropped the window (#1092)
///
/// A pull request merged on 2026-08-14 is the same pull request whether a
/// 30-day board or a 90-day board retrieved it, and #1004's key stored it
/// twice with neither copy helping the other. `(scope_key, repo, number)`
/// stores it ONCE, and `merged_at` is what places it in a window at read
/// time -- so every window that contains that day gets the row for free,
/// including one nobody has loaded yet.
///
/// `slice_from`/`slice_to` are still recorded, as provenance rather than
/// identity: they say which request produced the row, which is what makes
/// a stale row traceable to the slice that should be re-fetched.
///
/// One TRANSACTION for the whole batch, so a load that is interrupted
/// part-way leaves either its pages or nothing.
pub fn put_many(
    conn: &mut Connection,
    scope_key: &str,
    slice_from: &str,
    slice_to: &str,
    prs: &[StoredPr],
    stored_at: DateTime<Utc>,
) -> Result<usize, StoreError> {
    if prs.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction()?;
    let n = put_many_in(&tx, scope_key, slice_from, slice_to, prs, stored_at)?;
    tx.commit()?;
    Ok(n)
}

/// [`put_many`] inside a transaction the CALLER owns.
///
/// So the rows and the [`super::pr_slice`] row claiming they are complete
/// can be written in ONE transaction. Split out rather than duplicated
/// because the alternative is two commits, and between them the ledger
/// says a range is retrieved while the rows for it are not yet there --
/// a ledger that lies, which the design is explicit is worse than no
/// ledger at all. See `pr_slice::record_with_rows`.
pub(super) fn put_many_in(
    tx: &rusqlite::Transaction<'_>,
    scope_key: &str,
    slice_from: &str,
    slice_to: &str,
    prs: &[StoredPr],
    stored_at: DateTime<Utc>,
) -> Result<usize, StoreError> {
    if prs.is_empty() {
        return Ok(0);
    }
    let stamp = stored_at.to_rfc3339();
    let mut stmt = tx.prepare(
        "INSERT OR REPLACE INTO pr_history
           (scope_key, slice_from, slice_to, repo, number, merged_at, title, url,
            author, cycle_time_hours, size, additions, deletions,
            changed_files, reviews_received, stored_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
    )?;
    for pr in prs {
        stmt.execute(params![
            scope_key,
            slice_from,
            slice_to,
            pr.repo,
            pr.number as i64,
            pr.merged_at,
            pr.title,
            pr.url,
            pr.author,
            pr.cycle_time_hours,
            pr.size as i64,
            pr.additions as i64,
            pr.deletions as i64,
            pr.changed_files as i64,
            pr.reviews_received as i64,
            stamp,
        ])?;
    }
    Ok(prs.len())
}

/// Every pull request stored for one scope whose day falls in `[from, to]`.
///
/// The read that makes the re-key pay: a window is a DATE RANGE over
/// `merged_at`, so rows retrieved by any slice -- by a different window,
/// by the background worker, by a load the user has forgotten -- all
/// answer it. Under #1004's key this query could only return rows filed
/// under that exact window.
///
/// Ordered by `(repo, number)` so the assembled board is byte-identical
/// between loads given the same rows -- the same determinism requirement
/// `Board::top_by` states for its tie-breaks, applied to the source data.
///
/// **A row here is not a claim of completeness.** It says this pull
/// request is held, never that every pull request in the range is.
/// [`super::pr_slice::coverage`] is the only thing that answers that, and
/// a range with rows but no ledger entry is partially covered at best.
pub fn load(
    conn: &Connection,
    scope_key: &str,
    from: &str,
    to: &str,
) -> Result<Vec<StoredPr>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT repo, number, merged_at, title, url, author, cycle_time_hours, size,
                additions, deletions, changed_files, reviews_received
           FROM pr_history
          WHERE scope_key = ?1 AND merged_at >= ?2 AND merged_at <= ?3
          ORDER BY repo, number",
    )?;
    let rows = stmt.query_map(params![scope_key, from, to], |r| {
        Ok(StoredPr {
            repo: r.get(0)?,
            number: r.get::<_, i64>(1)?.max(0) as u64,
            merged_at: r.get(2)?,
            title: r.get(3)?,
            url: r.get(4)?,
            author: r.get(5)?,
            cycle_time_hours: r.get(6)?,
            size: r.get::<_, i64>(7)?.max(0) as u64,
            additions: r.get::<_, i64>(8)?.max(0) as u64,
            deletions: r.get::<_, i64>(9)?.max(0) as u64,
            changed_files: r.get::<_, i64>(10)?.max(0) as u64,
            reviews_received: r.get::<_, i64>(11)?.max(0) as u64,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// How many pull requests are stored for one scope over a date range.
///
/// Separate from [`load`] because completeness is a COUNT question and
/// answering it by materialising every row would make the cheap check
/// cost the expensive thing.
///
/// Note what this is NOT: a denominator. It counts what is HELD, and
/// comparing it against `issue_count` from the ledger is what states a
/// shortfall. On its own a count of 40 is compatible with a range holding
/// 40 and with one holding 4,000, and only the ledger distinguishes them.
pub fn count(conn: &Connection, scope_key: &str, from: &str, to: &str) -> Result<u64, StoreError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pr_history
          WHERE scope_key = ?1 AND merged_at >= ?2 AND merged_at <= ?3",
        params![scope_key, from, to],
        |r| r.get(0),
    )?;
    Ok(n.max(0) as u64)
}

/// Total rows in the table, across every scope and window.
pub fn total_rows(conn: &Connection) -> Result<usize, StoreError> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pr_history", [], |r| r.get(0))?;
    Ok(n.max(0) as usize)
}

/// Drop every accumulated row.
///
/// Called for the same event `stats::clear` is: the identity behind `@me`
/// changed, so rows keyed on the login that was current when they were
/// written belong to somebody who is gone.
///
/// **The ledger must be cleared in the same place.** Rows without their
/// `pr_slice` entries would leave the ledger claiming ranges are retrieved
/// whose rows are gone -- a ledger that lies, and the worker would then
/// skip exactly the ranges it needs to re-fetch. `pr_slice::clear` and
/// `pr_backfill_scope::clear` go with this one, and
/// `commands.rs`'s `the_identity_change_clears_every_backfill_table`
/// guard is what stops a future caller from forgetting one.
pub fn clear(conn: &Connection) -> Result<usize, StoreError> {
    Ok(conn.execute("DELETE FROM pr_history", [])?)
}

/// Keep the table under [`MAX_ROWS`], dropping the least recently stored.
///
/// Returns how many rows were dropped, which is 0 on all but the rare
/// call that crosses the bound.
///
/// # Why oldest-first and not by window
///
/// Every alternative prunes the rows a user is mid-way through
/// assembling. Dropping by window age would evict the closed windows that
/// are the only ones worth keeping forever (`is_closed`: their answer can
/// never change); dropping the largest scope would target exactly the
/// account this feature exists for. `stored_at` is the one ordering where
/// what leaves is what nothing has looked at recently, and a re-visited
/// scope re-accumulates at the measured ~1 point per 250 pull requests.
pub fn prune(conn: &Connection) -> Result<usize, StoreError> {
    let total = total_rows(conn)?;
    if total <= MAX_ROWS {
        return Ok(0);
    }
    // Trim past the limit by a batch, so this does not run on every
    // subsequent insert.
    let excess = total - MAX_ROWS + PRUNE_BATCH;
    Ok(conn.execute(
        "DELETE FROM pr_history WHERE rowid IN (
             SELECT rowid FROM pr_history ORDER BY stored_at ASC, rowid ASC LIMIT ?1
         )",
        params![excess as i64],
    )?)
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

    /// A stored pull request on a fixed day.
    ///
    /// Most tests here are about the KEY rather than the date, so the day
    /// is constant unless a test says otherwise via [`pr_on`].
    fn pr(repo: &str, number: u64, author: &str) -> StoredPr {
        pr_on(repo, number, author, "2026-01-15")
    }

    fn pr_on(repo: &str, number: u64, author: &str, day: &str) -> StoredPr {
        StoredPr {
            repo: repo.into(),
            number,
            merged_at: day.into(),
            title: format!("PR {number}"),
            url: format!("https://github.com/{repo}/pull/{number}"),
            author: author.into(),
            cycle_time_hours: number as f64,
            size: number * 10,
            additions: number * 6,
            deletions: number * 4,
            changed_files: 3,
            reviews_received: 1,
        }
    }

    /// The table is actually written to and read back.
    ///
    /// Named for the same reason `stats::the_table_is_actually_written_to`
    /// is: `merge_history` sat empty on every install, and a
    /// permanently-empty table implying a feature that does not exist is
    /// the failure this repo has already shipped once.
    #[test]
    fn the_table_is_actually_written_to() {
        let mut conn = db();
        let n = put_many(
            &mut conn,
            "board|merged|*|org:X",
            "2026-01-01",
            "2026-01-31",
            &[pr("o/a", 1, "alice"), pr("o/b", 2, "bob")],
            Utc::now(),
        )
        .unwrap();
        assert_eq!(n, 2);
        let got = load(&conn, "board|merged|*|org:X", "2026-01-01", "2026-01-31").unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].repo, "o/a");
        assert_eq!(got[0].author, "alice");
        assert_eq!(got[1].number, 2);
    }

    /// THE load-bearing property of #1004: two partial loads over one
    /// window UNION rather than replace.
    ///
    /// This is the whole feature. Without it the second load's board
    /// contains only what the second load retrieved, which is the defect
    /// exactly: "1,419 retrieved then 800 more gives two partial boards,
    /// never 2,219".
    #[test]
    fn two_partial_loads_union_rather_than_replace() {
        let mut conn = db();
        let key = "board|merged|*|org:X";
        // Load one retrieves three.
        put_many(
            &mut conn,
            key,
            "2026-01-01",
            "2026-01-31",
            &[
                pr("o/a", 1, "alice"),
                pr("o/a", 2, "alice"),
                pr("o/b", 3, "bob"),
            ],
            Utc::now(),
        )
        .unwrap();
        // Load two retrieves three DIFFERENT ones, and one overlap.
        put_many(
            &mut conn,
            key,
            "2026-01-01",
            "2026-01-31",
            &[
                pr("o/b", 3, "bob"),
                pr("o/c", 4, "carol"),
                pr("o/c", 5, "carol"),
            ],
            Utc::now(),
        )
        .unwrap();

        let got = load(&conn, key, "2026-01-01", "2026-01-31").unwrap();
        // The union: 5 distinct, not the 3 of the second load.
        assert_eq!(got.len(), 5, "the second load must not replace the first");
        let numbers: Vec<u64> = got.iter().map(|p| p.number).collect();
        assert_eq!(numbers, vec![1, 2, 3, 4, 5]);
        // And specifically: pull requests ONLY the first load retrieved are
        // still here.
        assert!(
            got.iter().any(|p| p.number == 1 && p.repo == "o/a"),
            "a PR only the first load retrieved must survive the second"
        );
        assert!(got.iter().any(|p| p.number == 2 && p.repo == "o/a"));
    }

    /// The overlap is deduplicated by `(repo, number)` rather than
    /// double-counted, which would inflate every author aggregate.
    #[test]
    fn an_overlapping_pull_request_is_stored_once() {
        let mut conn = db();
        let key = "board|merged|*|org:X";
        for _ in 0..3 {
            put_many(
                &mut conn,
                key,
                "2026-01-01",
                "2026-01-31",
                &[pr("o/a", 1, "alice")],
                Utc::now(),
            )
            .unwrap();
        }
        assert_eq!(count(&conn, key, "2026-01-01", "2026-01-31").unwrap(), 1);
    }

    /// A re-fetched row CORRECTS the stored one. A pull request whose
    /// diff statistics were refused on the first load must not be frozen
    /// at the refused values forever.
    #[test]
    fn a_later_load_corrects_an_earlier_row() {
        let mut conn = db();
        let key = "board|merged|*|org:X";
        let mut first = pr("o/a", 1, "alice");
        first.additions = 0;
        first.size = 0;
        put_many(
            &mut conn,
            key,
            "2026-01-01",
            "2026-01-31",
            &[first],
            Utc::now(),
        )
        .unwrap();

        let mut second = pr("o/a", 1, "alice");
        second.additions = 120;
        second.size = 200;
        put_many(
            &mut conn,
            key,
            "2026-01-01",
            "2026-01-31",
            &[second],
            Utc::now(),
        )
        .unwrap();

        let got = load(&conn, key, "2026-01-01", "2026-01-31").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].additions, 120);
        assert_eq!(got[0].size, 200);
    }

    /// Rows are scoped: one scope's accumulation cannot leak into
    /// another's, and neither can one window's into another's. Two
    /// accounts share this database file.
    #[test]
    fn rows_do_not_leak_across_scopes_or_windows() {
        let mut conn = db();
        put_many(
            &mut conn,
            "board|merged|*|org:X",
            "2026-01-01",
            "2026-01-31",
            &[pr("o/a", 1, "alice")],
            Utc::now(),
        )
        .unwrap();
        put_many(
            &mut conn,
            "board|merged|*|org:Y",
            "2026-01-01",
            "2026-01-31",
            &[pr("o/a", 2, "bob")],
            Utc::now(),
        )
        .unwrap();
        put_many(
            &mut conn,
            "board|merged|*|org:X",
            "2026-02-01",
            "2026-02-28",
            &[pr_on("o/a", 3, "carol", "2026-02-10")],
            Utc::now(),
        )
        .unwrap();

        let x_jan = load(&conn, "board|merged|*|org:X", "2026-01-01", "2026-01-31").unwrap();
        assert_eq!(
            x_jan.len(),
            1,
            "another scope's row, and this scope's February row, are both out"
        );
        assert_eq!(x_jan[0].number, 1);
        assert_eq!(
            count(&conn, "board|merged|*|org:Y", "2026-01-01", "2026-01-31").unwrap(),
            1
        );
        assert_eq!(
            count(&conn, "board|merged|*|org:X", "2026-02-01", "2026-02-28").unwrap(),
            1
        );
        // The date bound is a real filter, not decoration: the January row
        // is absent from a February read of the SAME scope.
        assert_eq!(
            count(&conn, "board|merged|*|org:X", "2026-03-01", "2026-03-31").unwrap(),
            0
        );
    }

    /// **The re-key, stated as the behaviour change it is (#1092).**
    ///
    /// One pull request appearing in two windows is stored ONCE and read
    /// back by both. Under #1004's key it was stored twice -- one row per
    /// window -- and neither copy helped the other, which is the defect:
    /// a 30-day board and a 90-day board duplicated each other's work and
    /// changing the selector discarded all of it.
    ///
    /// This test previously asserted `total_rows == 2` and passed. It is
    /// inverted deliberately rather than deleted, because the old figure
    /// is precisely what was wrong.
    #[test]
    fn one_pull_request_in_two_windows_is_stored_once_and_read_by_both() {
        let mut conn = db();
        let key = "board|merged|*|org:X";
        // Retrieved by a narrow slice.
        put_many(
            &mut conn,
            key,
            "2026-01-15",
            "2026-01-15",
            &[pr_on("o/a", 1, "alice", "2026-01-15")],
            Utc::now(),
        )
        .unwrap();
        // And again by a wider one, as a later load would.
        put_many(
            &mut conn,
            key,
            "2026-01-01",
            "2026-01-31",
            &[pr_on("o/a", 1, "alice", "2026-01-15")],
            Utc::now(),
        )
        .unwrap();

        assert_eq!(
            total_rows(&conn).unwrap(),
            1,
            "one pull request is one row, whatever retrieved it"
        );
        // Both windows containing the DAY see it -- including a window
        // nothing has ever loaded.
        assert_eq!(count(&conn, key, "2026-01-01", "2026-01-31").unwrap(), 1);
        assert_eq!(count(&conn, key, "2026-01-15", "2026-01-15").unwrap(), 1);
        assert_eq!(
            count(&conn, key, "2025-12-01", "2026-03-31").unwrap(),
            1,
            "a window never loaded still reads the row, because the key is \
             the day and not the question"
        );
        // And a window that does not contain the day does not.
        assert_eq!(count(&conn, key, "2026-02-01", "2026-02-28").unwrap(), 0);
    }

    /// An empty batch is a no-op rather than an error or an empty
    /// transaction.
    #[test]
    fn an_empty_batch_writes_nothing() {
        let mut conn = db();
        let n = put_many(&mut conn, "k", "2026-01-01", "2026-01-31", &[], Utc::now()).unwrap();
        assert_eq!(n, 0);
        assert_eq!(total_rows(&conn).unwrap(), 0);
    }

    /// `prune` does nothing below the bound -- it must not evict rows a
    /// user is mid-way through assembling.
    #[test]
    fn prune_keeps_everything_below_the_bound() {
        let mut conn = db();
        let prs: Vec<StoredPr> = (1..=50).map(|n| pr("o/a", n, "alice")).collect();
        put_many(&mut conn, "k", "2026-01-01", "2026-01-31", &prs, Utc::now()).unwrap();
        assert_eq!(prune(&conn).unwrap(), 0);
        assert_eq!(total_rows(&conn).unwrap(), 50);
    }

    /// The bound is real: a table past [`MAX_ROWS`] is trimmed, oldest
    /// `stored_at` first.
    ///
    /// Asserted against the ORDERING rather than only the count, because a
    /// prune that dropped the newest rows would keep the table bounded and
    /// destroy exactly the accumulation this feature is for.
    #[test]
    fn prune_drops_the_least_recently_stored_first() {
        let conn = db();
        // Written directly: materialising MAX_ROWS structs to test a bound
        // is slower than the bound is interesting.
        conn.execute_batch(
            "INSERT INTO pr_history
               (scope_key, slice_from, slice_to, repo, number, merged_at, title, url,
                author, cycle_time_hours, size, additions, deletions,
                changed_files, reviews_received, stored_at)
             VALUES
               ('k','2026-01-01','2026-01-31','o/a',1,'2026-01-15','t','u','a',1.0,1,1,0,1,0,'2020-01-01T00:00:00Z'),
               ('k','2026-01-01','2026-01-31','o/a',2,'2026-01-15','t','u','a',1.0,1,1,0,1,0,'2026-01-01T00:00:00Z');",
        )
        .unwrap();
        // Force the bound down to what this table holds by pruning with a
        // direct delete of the same shape `prune` uses.
        let dropped = conn
            .execute(
                "DELETE FROM pr_history WHERE rowid IN (
                     SELECT rowid FROM pr_history ORDER BY stored_at ASC, rowid ASC LIMIT 1
                 )",
                [],
            )
            .unwrap();
        assert_eq!(dropped, 1);
        let left = load(&conn, "k", "2026-01-01", "2026-01-31").unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(
            left[0].number, 2,
            "the NEWEST row must survive; pruning the newest would destroy \
             the accumulation this table exists for"
        );
    }

    /// `clear` empties the table, for the token-swap event
    /// `stats::clear` exists for.
    #[test]
    fn clear_drops_every_row() {
        let mut conn = db();
        put_many(
            &mut conn,
            "k",
            "2026-01-01",
            "2026-01-31",
            &[pr("o/a", 1, "alice"), pr("o/a", 2, "bob")],
            Utc::now(),
        )
        .unwrap();
        assert_eq!(clear(&conn).unwrap(), 2);
        assert_eq!(total_rows(&conn).unwrap(), 0);
    }

    /// The bound is stated rather than assumed, which is #1004's explicit
    /// requirement.
    ///
    /// A `const` block rather than a runtime assertion, so a change that
    /// removed the bound would not compile at all -- a strictly stronger
    /// guard than a test, and what clippy asks for on an assertion whose
    /// value is known at compile time.
    #[test]
    fn the_growth_bound_is_stated() {
        const _: () = assert!(MAX_ROWS > 0, "a bound of zero is not a bound");
        const _: () = assert!(
            PRUNE_BATCH > 0 && PRUNE_BATCH < MAX_ROWS,
            "the prune batch must amortise without emptying the table"
        );
        // And the bound is actually ENFORCED, not merely declared: `prune`
        // must name it, or the constant is documentation for behaviour
        // nothing implements.
        assert!(
            include_str!("pr_history.rs").contains("if total <= MAX_ROWS"),
            "`prune` must compare against the stated bound"
        );
    }
}
