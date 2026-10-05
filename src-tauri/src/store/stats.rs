//! The stats answer cache.
//!
//! One job: remember what GitHub already said about a closed time window,
//! so navigating back to a scope does not re-spend the request budget on
//! a number that cannot have changed.
//!
//! # This had to be built, not reused
//!
//! `schema.rs` migration 2 dropped `merge_history`, the table the original
//! stats design planned, because it was never written to -- and
//! `store/mod.rs` records that its SHAPE was wrong besides: it
//! accumulated merges by diffing the open PR set, where a PR leaving that
//! set may have been closed unmerged. Reusing it would have contradicted
//! the `is:merged` search that must stay authoritative.
//!
//! This module does not accumulate. It memoises an answer, keyed by the
//! question that produced it, and GitHub stays the only source of truth.
//! The test at the bottom asserts this table is actually written to, so it
//! cannot become the second permanently-empty table implying a feature
//! that does not exist.
//!
//! # Closed-window eligibility is separate from bounded retention
//!
//! The PRs merged in August 2026 are fixed once August is over. A
//! leaderboard over that window is a constant, so recomputing it per
//! navigation is pure waste -- and expensive waste, because recomputing
//! means the probe rounds and slice fetches of `github::stats::fetch`
//! against a 5,000-point hourly budget.
//!
//! A window that includes TODAY is a different thing entirely and gets a
//! short freshness bound. [`is_closed`] is where that line is drawn, and
//! it is drawn on the window's own end date rather than on the row's age,
//! because the age of the row tells you nothing about whether its answer
//! can still move.
//!
//! # Why three kinds of answer share one table (#836)
//!
//! Until #836 only `stats_count` cached, which is the CHEAPEST of the
//! three stats queries. `commands::stats_count` projects a count at
//! `days/5 + 8` -- about 26 points for 90 days -- while
//! `commands::board_projection` computes roughly 45 for the same window,
//! and `commands::stats_board`'s own doc calls a board over a busy org
//! "seconds" of node fetching. So the cache covered the query that costs
//! least to recompute and skipped the two that cost most.
//!
//! The argument above applies HARDER to a board than to a count: a
//! leaderboard over a month that has ended is as constant as a total over
//! it, and far more expensive to rebuild. The blocker was never the
//! schema -- `payload` is already an opaque blob and `complete` already
//! carries partiality -- it was that `StatsQuery::cache_key` has no
//! subject for a board (a board is about EVERYONE), so every board in a
//! scope would have collided with that scope's `*`-subject count.
//!
//! [`Kind`] is that missing discriminator, and it lives here rather than
//! in `StatsQuery::cache_key` because it is a property of the STORED
//! ANSWER's shape, not of the question's scope: a board and a count can
//! be built from the very same `StatsQuery`, and only the caller knows
//! which one it is about to write. Keeping it here also means a new
//! cached answer shape is one variant in this file, not a change to the
//! query model every stats path depends on.
//!
//! One consequence, accepted deliberately: rows written before #836 have
//! un-prefixed keys and will never be read again. They are a handful of
//! stale counts per user, served to nobody, and migrating them would mean
//! a schema migration to rewrite a cache -- a cache being exactly the
//! thing it is safe to lose. `put`'s `INSERT OR REPLACE` cannot collide
//! with them either, because no new key is un-prefixed.

use super::schema::StoreError;
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};

/// How long an answer about a window that includes today may be reused.
///
/// Five minutes. The window is still accumulating, so the answer is
/// genuinely stale the moment it is written -- this is not a correctness
/// bound but a de-duplication one: it stops a user clicking between two
/// scopes from paying for the same load twice in a row.
///
/// Short rather than long because the open window is where a user looks
/// for their own PR merged ten minutes ago, and a stale zero there reads
/// as "it did not count". Long enough that the repeated clicking that
/// motivates the cache at all is actually covered.
pub const OPEN_WINDOW_TTL_SECS: i64 = 300;

/// Which of the three stats answers a row holds.
///
/// The discriminator #836 needed. `StatsQuery::cache_key` is
/// `measure|subject|scope`, and for a board the subject is absent -- a
/// board is about everyone -- so a board's key would be byte-identical to
/// the whole-scope count's. Two different answers under one key is the
/// bug this type makes unrepresentable: an enum rather than a `&str`
/// parameter precisely so a caller cannot pass `"boards"` in one place and
/// `"board"` in another and get a silent permanent miss.
///
/// Ordered cheapest to most expensive, which is the order #836 argues they
/// should have been cached in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// One total: `commands::stats_count`.
    Count,
    /// A row per author: `commands::stats_board`.
    Board,
    /// A point per day: `commands::stats_series`.
    Series,
}

impl Kind {
    /// The key prefix. Short, lowercase, and stable: it is persisted, so
    /// renaming a variant's string silently orphans every row written
    /// under the old one.
    fn tag(self) -> &'static str {
        match self {
            Kind::Count => "count",
            Kind::Board => "board",
            Kind::Series => "series",
        }
    }
}

/// The stored key for one answer: a [`Kind`] and a `StatsQuery::cache_key`.
///
/// Built here rather than by each caller so the separator and the order
/// cannot drift between the writer and the reader -- a mismatch there is
/// not an error, it is a cache that silently never hits, which is the
/// hardest kind of failure to notice because everything still works and
/// only the request bill goes up.
///
/// `|` matches the separator `StatsQuery::cache_key` already uses, so a
/// full key reads `count|merged|octocat|org:Stohic`.
pub fn key(kind: Kind, query_key: &str) -> String {
    format!("{}|{query_key}", kind.tag())
}

/// A cached answer and what is known about its trustworthiness.
#[derive(Debug, Clone, PartialEq)]
pub struct Cached {
    pub total: u64,
    /// False when the stored answer was capped, refused, or assembled
    /// from a plan that could not be fully retrieved. Carried through
    /// storage so a partial answer cannot be read back as a confident
    /// one -- #824 item 8 does not stop applying because a number went
    /// through SQLite.
    pub complete: bool,
    /// The `Outcome` JSON as it was written. Opaque here on purpose: #826
    /// defines what a leaderboard needs and this module should not have
    /// an opinion about it.
    pub payload: String,
    pub fetched_at: DateTime<Utc>,
}

/// Whether a window has ended, and its answer can therefore never change.
///
/// `today` is passed rather than read from the clock so this is testable
/// without freezing time -- the same reason `query::period_ranges` takes
/// a `now`.
///
/// A window ending YESTERDAY or earlier is closed. Today's own date is
/// NOT closed even at 23:59: a PR can still be merged into it, and
/// `query::period_ranges` already makes exactly this distinction for the
/// same reason ("today is still accumulating, so including it compares a
/// partial period against complete ones", `query.rs:247-252`).
pub fn is_closed(window_end: &str, today: NaiveDate) -> bool {
    match NaiveDate::parse_from_str(window_end, "%Y-%m-%d") {
        Ok(end) => end < today,
        // An unparseable end date is treated as OPEN, which is the safe
        // direction: it means the answer is re-fetched rather than
        // trusted forever on the strength of a date nobody could read.
        Err(_) => false,
    }
}

/// Store one answer.
///
/// `INSERT OR REPLACE` rather than an ignore-if-present: a fresh answer
/// for the same window is strictly better than a stale one, and in
/// particular an answer that is now COMPLETE must be able to overwrite a
/// partial one from a load that was capped or refused.
#[allow(clippy::too_many_arguments)]
pub fn put(
    conn: &Connection,
    key: &str,
    window_start: &str,
    window_end: &str,
    total: u64,
    complete: bool,
    payload: &str,
    fetched_at: DateTime<Utc>,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO stats_cache
           (key, window_start, window_end, total, complete, payload, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            key,
            window_start,
            window_end,
            total as i64,
            i64::from(complete),
            payload,
            fetched_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Read an answer back, if one is still usable.
///
/// Returns `None` when there is no row, or when the row is about a window
/// that is still open and older than [`OPEN_WINDOW_TTL_SECS`]. A closed
/// window's answer is returned however old it is, which is the point.
///
/// `now` is a parameter for the same reason `is_closed` takes `today`.
pub fn get(
    conn: &Connection,
    key: &str,
    window_start: &str,
    window_end: &str,
    now: DateTime<Utc>,
) -> Result<Option<Cached>, StoreError> {
    let row = conn
        .query_row(
            "SELECT total, complete, payload, fetched_at FROM stats_cache
              WHERE key = ?1 AND window_start = ?2 AND window_end = ?3",
            params![key, window_start, window_end],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((total, complete, payload, fetched_at)) = row else {
        return Ok(None);
    };
    // An unparseable timestamp makes the row unusable rather than
    // eternally fresh. Same direction as `is_closed`'s parse failure:
    // re-fetch rather than trust something nobody can read.
    let Ok(fetched) = DateTime::parse_from_rfc3339(&fetched_at) else {
        return Ok(None);
    };
    let fetched = fetched.with_timezone(&Utc);
    if !is_closed(window_end, now.date_naive())
        && (now - fetched).num_seconds() > OPEN_WINDOW_TTL_SECS
    {
        return Ok(None);
    }
    Ok(Some(Cached {
        total: total.max(0) as u64,
        complete: complete != 0,
        payload,
        fetched_at: fetched,
    }))
}

/// Bound only derived answers: three recent UTC end dates and 2,048 rows.
/// Called in the owner's write transaction, never on the WAL read fast path.
pub fn prune_answers(
    conn: &Connection,
    now: DateTime<Utc>,
    active: (&str, &str, &str),
) -> Result<usize, StoreError> {
    let earliest = (now.date_naive() - chrono::Duration::days(3)).to_string();
    let mut removed = conn.execute("DELETE FROM stats_cache WHERE window_end < ?1", [earliest])?;
    // Protect this write only during cardinality eviction. Acquisition age
    // stays unchanged, and the UTC-end horizon applies even to this row.
    removed += conn.execute(
        "DELETE FROM stats_cache WHERE rowid IN (
            SELECT rowid FROM stats_cache
            ORDER BY (key=?1 AND window_start=?2 AND window_end=?3) DESC,
                     fetched_at DESC, rowid DESC LIMIT -1 OFFSET 2048
        )",
        params![active.0, active.1, active.2],
    )?;
    Ok(removed)
}

/// Drop every cached answer.
///
/// For the moment when the identity behind `@me` changes: a new token is
/// a different person, and `StatsQuery::cache_key` resolves `@me` with
/// the login that was current when the row was written. Keys are
/// namespaced by login so a stale row cannot be *served* to the wrong
/// user, but it can still sit there forever taking space for a user who
/// is gone.
///
/// Owner transitions clear all derived answers. Ordinary writes separately
/// reclaim unreachable daily keys and bound cardinality with `prune_answers`.
pub fn clear(conn: &Connection) -> Result<usize, StoreError> {
    Ok(conn.execute("DELETE FROM stats_cache", [])?)
}

/// Record who `@me` currently is, clearing the cache if it changed.
///
/// Returns how many rows were dropped, which is 0 on every call but the
/// one after a token swap.
///
/// # Why this is the right trigger
///
/// `clear`'s doc names the event it exists for -- "a new token is a
/// different person" -- and #840 found nothing ever raised it. The only
/// place the app learns the current identity is `fetch_viewer`, whose
/// result `stats_count` and `stats_board` already need for their keys, so
/// the check is free: compare the login against the last one seen and
/// store it.
///
/// # Why the cache is cleared WHOLESALE rather than per login
///
/// Deleting only the departed user's rows would need the key's subject
/// field parsed back out, and the subject is not the only place a login
/// appears in an answer -- a board's `payload` is a row per person and its
/// viewer split was made against the OLD login. A board cached under
/// `board|merged|*|org:X` is keyed on no login at all, so a per-login
/// delete would leave exactly the rows whose contents are most tied to the
/// identity that went away. The table is small by construction (see
/// `clear`), so dropping all of it costs one re-fetch of whatever the new
/// user looks at first.
///
/// # Why a row counts as "no previous identity" rather than as a change
///
/// A fresh install has no stored login, and treating that as a change
/// would mean the first stats load of every install clears an empty
/// table -- harmless, but it would also make the test below unable to
/// distinguish "first run" from "swapped", which is the distinction that
/// matters.
///
/// Failure to READ the stored login is treated as absent rather than
/// propagated: a settings row that will not parse must not be able to
/// break the stats page, and the cost of getting this wrong is one
/// unnecessary re-fetch.
#[cfg(test)]
fn note_viewer(conn: &Connection, login: &str) -> Result<usize, StoreError> {
    let previous: Option<String> = super::settings::get(conn, super::settings::keys::STATS_VIEWER)
        .ok()
        .flatten();
    if previous.as_deref() == Some(login) {
        return Ok(0);
    }
    let dropped = match previous {
        // A DIFFERENT login: the rows belong to whoever was here before.
        Some(_) => clear(conn)?,
        // First run. Nothing to drop, and nothing stale to worry about.
        None => 0,
    };
    super::settings::set(conn, super::settings::keys::STATS_VIEWER, &login)?;
    Ok(dropped)
}

/// How many answers are cached.
///
/// For diagnostics, and for the tests below that prove this table is
/// written to. #840 noted it had no production caller beyond that test;
/// it stays `pub` rather than `pub(crate)` because `commands`' own
/// command-level guard (#840: the module-level one would have passed with
/// every caller deleted) reads it too.
pub fn count(conn: &Connection) -> Result<u64, StoreError> {
    let n: i64 = conn.query_row("SELECT count(*) FROM stats_cache", [], |r| r.get(0))?;
    Ok(n.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// The whole point: an answer about a window that has ENDED cannot
    /// change, so it is reusable however old the row is. Recomputing it
    /// would re-spend the probe rounds and slice fetches for a constant.
    #[test]
    fn a_closed_windows_answer_never_goes_stale() {
        let conn = db();
        put(
            &conn,
            "merged|octocat|org:FNX-Labs",
            "2026-08-01",
            "2026-08-31",
            706,
            true,
            r#"{"total":706}"#,
            at("2026-09-01T00:00:00Z"),
        )
        .unwrap();

        // A year later. August 2026 is still over.
        let got = get(
            &conn,
            "merged|octocat|org:FNX-Labs",
            "2026-08-01",
            "2026-08-31",
            at("2027-09-01T00:00:00Z"),
        )
        .unwrap()
        .expect("a closed window's answer is still good");
        assert_eq!(got.total, 706);
        assert!(got.complete);
    }

    /// A window that includes today is still accumulating, so its answer
    /// gets a short bound. The failure this prevents: a user's PR merged
    /// ten minutes ago missing from "this week", which reads as "it did
    /// not count".
    #[test]
    fn an_open_windows_answer_expires_quickly() {
        let conn = db();
        put(
            &conn,
            "merged|octocat|org:FNX-Labs",
            "2026-09-01",
            "2026-09-30",
            93,
            true,
            "{}",
            at("2026-09-11T12:00:00Z"),
        )
        .unwrap();
        let key = ("merged|octocat|org:FNX-Labs", "2026-09-01", "2026-09-30");

        // Inside the TTL: reused, which is the de-duplication the cache
        // is for.
        assert!(get(&conn, key.0, key.1, key.2, at("2026-09-11T12:04:00Z"))
            .unwrap()
            .is_some());
        // Past it: re-fetched.
        assert!(
            get(&conn, key.0, key.1, key.2, at("2026-09-11T12:06:00Z"))
                .unwrap()
                .is_none(),
            "an accumulating window must not be served stale for long"
        );
    }

    /// The line is TODAY, matching `query::period_ranges`'s own rule that
    /// today is still accumulating (`query.rs:247-252`).
    #[test]
    fn today_is_not_a_closed_window() {
        assert!(is_closed("2026-09-10", day("2026-09-11")));
        assert!(
            !is_closed("2026-09-11", day("2026-09-11")),
            "a PR can still be merged into today"
        );
        assert!(!is_closed("2026-09-12", day("2026-09-11")));
    }

    /// An unparseable end date is treated as OPEN, so the answer is
    /// re-fetched rather than trusted forever on a date nobody can read.
    #[test]
    fn an_unreadable_window_is_never_treated_as_closed() {
        assert!(!is_closed("not-a-date", day("2026-09-11")));
        assert!(!is_closed("", day("2026-09-11")));
    }

    /// Partiality survives the round trip. A cached total whose
    /// partiality was forgotten launders a sample into a fact, which is
    /// worse than having no cache at all.
    #[test]
    fn a_partial_answer_is_read_back_as_partial() {
        let conn = db();
        put(
            &conn,
            "merged|*|org:FNX-Labs",
            "2026-08-01",
            "2026-08-01",
            1_200,
            false,
            r#"{"retrievable":false,"unretrievable":200}"#,
            at("2026-09-01T00:00:00Z"),
        )
        .unwrap();
        let got = get(
            &conn,
            "merged|*|org:FNX-Labs",
            "2026-08-01",
            "2026-08-01",
            at("2026-09-02T00:00:00Z"),
        )
        .unwrap()
        .unwrap();
        assert!(!got.complete, "a capped answer must not read back clean");
        assert!(got.payload.contains("unretrievable"));
    }

    /// A complete answer must be able to replace a partial one -- a load
    /// that was capped or refused should not poison the row forever.
    #[test]
    fn a_complete_answer_overwrites_a_partial_one() {
        let conn = db();
        let k = ("merged|octocat|org:Stohic", "2026-08-01", "2026-08-31");
        put(
            &conn,
            k.0,
            k.1,
            k.2,
            100,
            false,
            "{}",
            at("2026-09-01T00:00:00Z"),
        )
        .unwrap();
        put(
            &conn,
            k.0,
            k.1,
            k.2,
            319,
            true,
            "{}",
            at("2026-09-02T00:00:00Z"),
        )
        .unwrap();
        let got = get(&conn, k.0, k.1, k.2, at("2026-09-03T00:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(got.total, 319);
        assert!(got.complete);
        assert_eq!(count(&conn).unwrap(), 1, "replaced, not duplicated");
    }

    /// Different windows for one key are different rows. A month's answer
    /// must not be served for a quarter's.
    #[test]
    fn the_window_is_part_of_the_identity() {
        let conn = db();
        let k = "merged|octocat|org:FNX-Labs";
        put(
            &conn,
            k,
            "2026-07-01",
            "2026-07-31",
            461,
            true,
            "{}",
            at("2026-09-01T00:00:00Z"),
        )
        .unwrap();
        put(
            &conn,
            k,
            "2026-08-01",
            "2026-08-31",
            706,
            true,
            "{}",
            at("2026-09-01T00:00:00Z"),
        )
        .unwrap();
        assert_eq!(count(&conn).unwrap(), 2);
        let july = get(
            &conn,
            k,
            "2026-07-01",
            "2026-07-31",
            at("2026-09-02T00:00:00Z"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(july.total, 461);
        // And a window nobody asked about is absent rather than
        // approximated from a neighbour.
        assert!(get(
            &conn,
            k,
            "2026-07-01",
            "2026-08-31",
            at("2026-09-02T00:00:00Z")
        )
        .unwrap()
        .is_none());
    }

    /// Two subjects are two rows. The key carries a RESOLVED login
    /// precisely so this holds -- see `StatsQuery::cache_key`.
    #[test]
    fn two_subjects_do_not_share_a_row() {
        let conn = db();
        let w = ("2026-08-01", "2026-08-31");
        put(
            &conn,
            "merged|octocat|org:FNX-Labs",
            w.0,
            w.1,
            10,
            true,
            "{}",
            at("2026-09-01T00:00:00Z"),
        )
        .unwrap();
        put(
            &conn,
            "merged|pktstorm|org:FNX-Labs",
            w.0,
            w.1,
            99,
            true,
            "{}",
            at("2026-09-01T00:00:00Z"),
        )
        .unwrap();
        let a = get(
            &conn,
            "merged|octocat|org:FNX-Labs",
            w.0,
            w.1,
            at("2026-09-02T00:00:00Z"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(a.total, 10, "one user's row must not serve another's");
        assert_eq!(count(&conn).unwrap(), 2);
    }

    #[test]
    fn a_missing_row_is_none_not_zero() {
        let conn = db();
        assert!(
            get(&conn, "nothing", "2026-01-01", "2026-01-31", Utc::now())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn clearing_drops_every_row() {
        let conn = db();
        put(
            &conn,
            "k",
            "2026-01-01",
            "2026-01-31",
            1,
            true,
            "{}",
            Utc::now(),
        )
        .unwrap();
        assert_eq!(clear(&conn).unwrap(), 1);
        assert_eq!(count(&conn).unwrap(), 0);
    }

    /// A board and a count over the SAME scope and window are two rows.
    ///
    /// The collision #836 had to fix before a board could be cached at
    /// all. A board has no subject -- it is about everyone -- so
    /// `StatsQuery::cache_key` gives it the `*` subject, which is exactly
    /// what a whole-scope count gets. Without [`Kind`] the second of the
    /// two written would have REPLACED the first (`put` is `INSERT OR
    /// REPLACE`), and the reader would have deserialised a board's payload
    /// into an `Outcome` or the reverse -- which fails softly, as a cache
    /// row that is discarded and re-fetched forever.
    #[test]
    fn a_board_and_a_count_over_one_scope_do_not_collide() {
        let conn = db();
        // What `StatsQuery::cache_key` produces for a subject-less query.
        let q = "merged|*|org:FNX-Labs";
        let w = ("2026-08-01", "2026-08-31");

        let count_key = key(Kind::Count, q);
        let board_key = key(Kind::Board, q);
        let series_key = key(Kind::Series, q);
        assert_ne!(count_key, board_key, "a board must not share a count's row");
        assert_ne!(count_key, series_key);
        assert_ne!(board_key, series_key);

        for (k, total) in [(&count_key, 706), (&board_key, 41), (&series_key, 706)] {
            put(
                &conn,
                k,
                w.0,
                w.1,
                total,
                true,
                "{}",
                at("2026-09-01T00:00:00Z"),
            )
            .unwrap();
        }
        assert_eq!(count(&conn).unwrap(), 3, "three answers, three rows");
        let got = get(&conn, &board_key, w.0, w.1, at("2026-09-02T00:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(got.total, 41, "the board's row, not the count's");
    }

    /// The key's prefix is the kind's tag, and it is STABLE.
    ///
    /// Pinned on the literal strings because they are persisted: renaming
    /// a tag does not fail to compile, it silently orphans every row
    /// written under the old one, which presents as a cache that stopped
    /// working for no visible reason.
    #[test]
    fn the_kind_tags_are_the_persisted_strings() {
        assert_eq!(
            key(Kind::Count, "merged|octocat|org:X"),
            "count|merged|octocat|org:X"
        );
        assert_eq!(key(Kind::Board, "merged|*|org:X"), "board|merged|*|org:X");
        assert_eq!(key(Kind::Series, "merged|*|all"), "series|merged|*|all");
    }

    /// A partial board must not read back as a complete one.
    ///
    /// #836's acceptance item: "a partial result is never cached as
    /// complete". Asserted for a BOARD payload specifically rather than
    /// relying on `a_partial_answer_is_read_back_as_partial` above, which
    /// covers a count -- the board is the answer whose partiality is a
    /// short ROSTER, and a leaderboard that forgot it was short is a
    /// ranking presented as the whole population.
    #[test]
    fn a_partial_board_is_never_cached_as_complete() {
        let conn = db();
        let k = key(Kind::Board, "merged|*|org:FNX-Labs");
        let w = ("2026-08-01", "2026-08-31");
        put(
            &conn,
            &k,
            w.0,
            w.1,
            1_200,
            // The board's own `complete` flag: false because a slice came
            // back short of the 1,000-result cap.
            false,
            r#"{"rows":[],"total":1200,"retrieved":1000,"complete":false}"#,
            at("2026-09-01T00:00:00Z"),
        )
        .unwrap();
        let got = get(&conn, &k, w.0, w.1, at("2027-01-01T00:00:00Z"))
            .unwrap()
            .expect("a closed window is still served");
        assert!(
            !got.complete,
            "a board assembled from a short roster must not read back whole"
        );
        assert!(got.payload.contains(r#""complete":false"#));
    }

    /// An identity change drops the previous user's rows; a repeat login
    /// does not.
    ///
    /// #840: `clear`'s doc described this scenario and nothing ever raised
    /// it. The three cases asserted here are the three that exist -- first
    /// run, same person again, and a swap -- and the middle one is the one
    /// that would be easy to get wrong in the expensive direction: a
    /// `note_viewer` that cleared whenever the stored value was not
    /// *compared* correctly would throw the cache away on every load,
    /// turning the fix into a permanent cache miss.
    #[test]
    fn a_changed_identity_drops_the_previous_users_rows() {
        let conn = db();
        let k = key(Kind::Count, "merged|octocat|org:FNX-Labs");
        let w = ("2026-08-01", "2026-08-31");
        let put_one = |c: &Connection| {
            put(c, &k, w.0, w.1, 706, true, "{}", at("2026-09-01T00:00:00Z")).unwrap();
        };

        // First run: nothing stored, nothing to drop.
        assert_eq!(
            note_viewer(&conn, "octocat").unwrap(),
            0,
            "a fresh install must not report a clear it did not need"
        );
        put_one(&conn);

        // The same person again: the cache survives, which is the whole
        // point of having one.
        assert_eq!(note_viewer(&conn, "octocat").unwrap(), 0);
        assert_eq!(count(&conn).unwrap(), 1, "a repeat login keeps the cache");

        // A different token: the rows belong to whoever was here before.
        assert_eq!(
            note_viewer(&conn, "pktstorm").unwrap(),
            1,
            "a new identity must reclaim the previous one's rows"
        );
        assert_eq!(count(&conn).unwrap(), 0);

        // And the new login is now the remembered one, so going back is
        // itself a change rather than a free hit on stale rows.
        put_one(&conn);
        assert_eq!(note_viewer(&conn, "octocat").unwrap(), 1);
    }

    /// THE guard against this becoming the second `merge_history`.
    ///
    /// Migration 2 dropped that table because nothing ever wrote to it,
    /// and `store/mod.rs` calls a permanently-empty table "a feature that
    /// does not exist". This asserts the write path exists and works end
    /// to end -- schema, insert, read back -- so a table with no writer
    /// cannot ship again.
    ///
    /// #840 noted the weakness this does NOT cover: it exercises the
    /// MODULE's write path, so it would still pass with every `put` call
    /// in `commands.rs` deleted -- which is precisely the `merge_history`
    /// failure it is named for. `commands`' own
    /// `every_stats_command_writes_its_answer_to_the_cache` is the half
    /// that asserts a COMMAND writes a row.
    #[test]
    fn the_table_is_actually_written_to() {
        let conn = db();
        assert_eq!(count(&conn).unwrap(), 0, "starts empty");
        put(
            &conn,
            "merged|pktstorm|repo:pktstorm/headstate",
            "2026-01-01",
            "2026-08-31",
            337,
            true,
            r#"{"total":337,"viaConnection":true}"#,
            at("2026-09-01T00:00:00Z"),
        )
        .unwrap();
        assert_eq!(count(&conn).unwrap(), 1, "and is written to");
        let got = get(
            &conn,
            "merged|pktstorm|repo:pktstorm/headstate",
            "2026-01-01",
            "2026-08-31",
            at("2026-09-02T00:00:00Z"),
        )
        .unwrap()
        .unwrap();
        // The live figure for this repository through the connection,
        // which is the path that has no 1,000-result cap.
        assert_eq!(got.total, 337);
    }
}
