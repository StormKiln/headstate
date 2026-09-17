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
pub mod health;
/// Which scopes the background backfill is allowed to walk, and how far
/// (#1092). Written when a user opens a scope, so background spend
/// follows demonstrated interest rather than everything a token can see.
pub mod pr_backfill_scope;
pub mod pr_history;
/// The ledger saying which date ranges have been retrieved -- and, by the
/// ABSENCE of a row, which have never been asked for (#1092).
pub mod pr_slice;
mod schema;
pub mod settings;
pub mod stats;

pub use cache::{load_snapshot, load_snapshot_marked, save_snapshot, CachedList, CachedSnapshot};
pub use schema::{open_db, StoreError};

/// Test-only: the remote pairing tests migrate an in-memory connection.
#[cfg(test)]
pub(crate) use schema::migrate;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::model::{
        CiState, Label, MergeState, MergeStateStatus, PullRequest, ReviewState,
    };
    use chrono::Utc;

    fn db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    fn sample() -> PullRequest {
        PullRequest {
            id: "PR_test".into(),
            number: 42,
            title: "Add retry to the fetch client".into(),
            url: "https://github.com/octocat/hello-world/pull/42".into(),
            repo: "octocat/hello-world".into(),
            head_ref: "feature/x".into(),
            head_oid: "deadbeef".into(),
            head_ref_id: None,
            base_ref: "main".into(),
            author: "octocat".into(),
            is_draft: false,
            created_at: Utc::now(),
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
    fn round_trips_a_snapshot() {
        let conn = db();
        save_snapshot(&conn, CachedList::Authored, &[sample()]).unwrap();
        let loaded = load_snapshot(&conn, CachedList::Authored).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].number, 42);
        assert_eq!(loaded[0].labels[0].name, "bug");
    }

    #[test]
    fn a_snapshot_replaces_the_previous_one() {
        let conn = db();
        save_snapshot(&conn, CachedList::Authored, &[sample()]).unwrap();
        save_snapshot(&conn, CachedList::Authored, &[]).unwrap();
        assert_eq!(load_snapshot(&conn, CachedList::Authored).unwrap().len(), 0);
    }

    #[test]
    fn loading_from_an_empty_db_returns_empty_not_an_error() {
        assert_eq!(load_snapshot(&db(), CachedList::Authored).unwrap().len(), 0);
    }

    /// #742: a snapshot past the freshness window must come back MARKED,
    /// not discarded.
    ///
    /// `load_snapshot` returns an empty Vec for a stale snapshot, which
    /// the UI could not tell from "nothing awaits your review" -- so it
    /// rendered a confident empty list for as long as the live fetch
    /// took. `load_snapshot_marked` returns the rows and their age.
    #[test]
    fn a_stale_snapshot_is_returned_with_its_age() {
        let conn = db();
        save_snapshot(&conn, CachedList::Reviewing, &[sample()]).unwrap();
        // Backdate past the hour-long window.
        // Every row: this database holds only the one snapshot just
        // written, and `id()` is private to the cache module.
        conn.execute(
            "UPDATE snapshot SET fetched_at = datetime('now', '-2 hours')",
            [],
        )
        .unwrap();

        // The old path still hides it, which is what callers that only
        // want fresh data rely on.
        assert_eq!(
            load_snapshot(&conn, CachedList::Reviewing).unwrap().len(),
            0,
            "load_snapshot still ages out"
        );

        let marked = load_snapshot_marked(&conn, CachedList::Reviewing).unwrap();
        assert_eq!(marked.prs.len(), 1, "the rows survive");
        let age = marked.stale_secs.expect("a stale snapshot reports its age");
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
        save_snapshot(&conn, CachedList::Reviewing, &[sample()]).unwrap();
        let marked = load_snapshot_marked(&conn, CachedList::Reviewing).unwrap();
        assert_eq!(marked.prs.len(), 1);
        assert_eq!(
            marked.stale_secs, None,
            "just written: nothing to warn about"
        );
    }

    /// An absent snapshot is not a stale one. Marking it would put an
    /// "old data" warning over a view that has never had any.
    #[test]
    fn no_snapshot_at_all_is_not_marked() {
        let marked = load_snapshot_marked(&db(), CachedList::Reviewing).unwrap();
        assert!(marked.prs.is_empty());
        assert_eq!(marked.stale_secs, None);
    }

    #[test]
    fn migrations_are_idempotent() {
        let conn = db();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(load_snapshot(&conn, CachedList::Authored).unwrap().len(), 0);
    }
}
