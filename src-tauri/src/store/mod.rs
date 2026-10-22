//! SQLite persistence.
//!
//! One role: [`cache`], the snapshot cache -- the last poll's PR list, so
//! launch paints real content instead of a spinner and the app is readable
//! offline.
//!
//! There was also a `history` module writing a row per merged PR. It was
//! never called, so `merge_history` sat empty on every install while two
//! module docs described week/month counters "surviving offline" that in
//! fact came from live network calls and errored without a client.
//!
//! It is gone rather than wired up, because the shape was wrong: a PR
//! leaving the open set is NOT necessarily a merge -- it may be closed
//! unmerged -- so a disappearance diff would have recorded abandoned PRs
//! as merges, contradicting the `is:merged` search that must stay
//! authoritative. `MERGED_DETAIL_QUERY` already returns the real merged
//! set inside a search the app already pays for. Local accumulation past
//! the 90-day live window needs a per-DAY counts table and a way to mark
//! unobserved days, which is tracked separately.

mod cache;
pub mod devices;
pub mod disk_observations;
pub mod github_facts;
pub mod health;
/// Filesystem scan results, so a cold start is not a blank page (#1152).
pub mod scans;
mod schema;
pub mod settings;
pub mod source_cache;
#[cfg(test)]
mod sqlite_efficiency;

pub use cache::{CachedList, CachedSnapshot};
pub use schema::{open_db, StoreError};

/// Test-only: the remote pairing tests migrate an in-memory connection.
#[cfg(test)]
pub(crate) use schema::{historical_schema, migrate};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::model::{
        CiState, Label, MergeState, MergeStateStatus, PullRequest, ReviewState,
    };
    use chrono::Utc;
    use source_cache::SnapshotData;

    fn db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    /// Write the default source's list as the removed `save_snapshot` did
    /// (#2003), with coverage left unknown, through the writer production
    /// uses and with an owner, as production records one (#2009).
    fn save(conn: &rusqlite::Connection, which: CachedList, prs: &[PullRequest]) {
        source_cache::save_owned_source_snapshot(
            conn,
            &crate::identity::Source::default(),
            which,
            prs,
            &source_cache::Coverage::Unknown,
            Some("alice"),
        )
        .unwrap();
    }

    /// The default source's list, read the way production reads it.
    fn read(conn: &rusqlite::Connection, which: CachedList) -> SnapshotData {
        source_cache::load_source_snapshot(conn, &crate::identity::Source::default(), which)
            .unwrap()
            .data
    }

    /// The rows of a FRESH available snapshot; panics on any other state.
    fn fresh_rows(conn: &rusqlite::Connection, which: CachedList) -> Vec<PullRequest> {
        match read(conn, which) {
            SnapshotData::Available {
                prs,
                stale_secs: None,
                ..
            } => prs,
            other => panic!("expected a fresh snapshot, got {other:?}"),
        }
    }

    fn sample() -> PullRequest {
        PullRequest {
            observation: None,
            source: Default::default(),
            id: "PR_test".into(),
            number: 42,
            title: "Add retry to the fetch client".into(),
            url: "https://github.com/octocat/hello-world/pull/42".into(),
            repo: "octocat/hello-world".into(),
            head_ref: "feature/x".into(),
            head_oid: "deadbeef".into(),
            head_ref_id: None,
            head_repo: None,
            base_ref: "main".into(),
            author: "octocat".into(),
            is_draft: false,
            created_at: Utc::now(),
            ready_at: None,
            updated_at: Utc::now(),
            ci: CiState::Success,
            merge: MergeState::Mergeable,
            merge_status: MergeStateStatus::Clean,
            review: ReviewState::Approved,
            in_merge_queue: false,
            labels: vec![Label {
                name: "bug".into(),
                color: "d73a4a".into(),
            }],
            comment_count: 2,
            unresolved_threads: 0,
            unresolved_threads_floor: false,
            requested_reviewers: Vec::new(),
            assignees: Vec::new(),
            latest_reviews: Vec::new(),
            // Zero totals against empty lists: nothing was cut.
            requested_reviewers_total: 0,
            assignees_total: 0,
            latest_reviews_total: 0,
            labels_total: 0,
        }
    }

    #[test]
    fn legacy_snapshot_loads_as_github_and_emits_explicit_source() {
        let conn = db();
        let original = sample();
        let mut value = serde_json::to_value(&original).unwrap();
        value.as_object_mut().unwrap().remove("source");
        let legacy = serde_json::to_string(&vec![value]).unwrap();
        conn.execute(
            "INSERT INTO snapshot (id, payload, fetched_at) VALUES (1, ?1, datetime('now'))",
            rusqlite::params![legacy],
        )
        .unwrap();
        let loaded = fresh_rows(&conn, CachedList::Authored);
        assert_eq!(loaded, vec![original.clone()]);
        assert_eq!(
            loaded[0].identity().source,
            crate::identity::Source::default()
        );
        let emitted = serde_json::to_value(&loaded[0]).unwrap();
        assert_eq!(
            emitted["source"],
            serde_json::json!({"provider": "github", "host": "github.com"})
        );
    }

    #[test]
    fn round_trips_a_snapshot() {
        let conn = db();
        save(&conn, CachedList::Authored, &[sample()]);
        let loaded = fresh_rows(&conn, CachedList::Authored);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].number, 42);
        assert_eq!(loaded[0].labels[0].name, "bug");
    }

    #[test]
    fn a_snapshot_replaces_the_previous_one() {
        let conn = db();
        save(&conn, CachedList::Authored, &[sample()]);
        save(&conn, CachedList::Authored, &[]);
        assert_eq!(fresh_rows(&conn, CachedList::Authored).len(), 0);
    }

    #[test]
    fn loading_from_an_empty_db_returns_empty_not_an_error() {
        // `read` unwraps, so an error fails here; and "nothing saved" is
        // its own state, not a measured empty list.
        assert_eq!(read(&db(), CachedList::Authored), SnapshotData::Missing);
    }

    /// The saved reviewing list, read the way production reads it
    /// (`source_cache::load_source_snapshot`), as its rows and age.
    /// `None` when nothing usable is saved.
    fn rows_and_age(conn: &rusqlite::Connection) -> Option<(Vec<PullRequest>, Option<i64>)> {
        match read(conn, CachedList::Reviewing) {
            SnapshotData::Available {
                prs, stale_secs, ..
            } => Some((prs, stale_secs)),
            SnapshotData::Missing => None,
            other => panic!("unexpected snapshot state: {other:?}"),
        }
    }

    /// #742: a snapshot past the freshness window must come back MARKED,
    /// not discarded.
    ///
    /// The old `load_snapshot` returned an empty Vec for a stale snapshot,
    /// which the UI could not tell from "nothing awaits your review" -- so
    /// it rendered a confident empty list for as long as the live fetch
    /// took. The production reader returns the rows and their age.
    /// (These drove `load_snapshot_marked`, which only tests reached;
    /// #1998 moved them onto `load_source_snapshot`. `load_snapshot`
    /// itself was test-only too and went in #2003.)
    #[test]
    fn a_stale_snapshot_is_returned_with_its_age() {
        let conn = db();
        save(&conn, CachedList::Reviewing, &[sample()]);
        // Backdate past the hour-long window.
        // Every row: this database holds only the one snapshot just
        // written, and `id()` is private to the cache module.
        conn.execute(
            "UPDATE snapshot SET fetched_at = datetime('now', '-2 hours')",
            [],
        )
        .unwrap();

        let (prs, stale_secs) = rows_and_age(&conn).expect("the rows survive");
        assert_eq!(prs.len(), 1, "the rows survive");
        let age = stale_secs.expect("a stale snapshot reports its age");
        assert!(
            (7100..7300).contains(&age),
            "two hours, give or take clock granularity: {age}"
        );
    }

    /// The other direction: a snapshot inside the window must NOT be
    /// marked, or every ordinary paint would carry a warning and the
    /// marker would stop meaning anything.
    #[test]
    fn a_fresh_snapshot_is_not_marked() {
        let conn = db();
        save(&conn, CachedList::Reviewing, &[sample()]);
        let (prs, stale_secs) = rows_and_age(&conn).expect("just written");
        assert_eq!(prs.len(), 1);
        assert_eq!(stale_secs, None, "just written: nothing to warn about");
    }

    /// An absent snapshot is not a stale one. Marking it would put an
    /// "old data" warning over a view that has never had any -- and it
    /// is not an empty list either: it is its own state.
    #[test]
    fn no_snapshot_at_all_is_not_marked() {
        assert!(rows_and_age(&db()).is_none(), "missing, not marked");
    }

    #[test]
    fn migrations_are_idempotent() {
        let conn = db();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(read(&conn, CachedList::Authored), SnapshotData::Missing);
    }
}
