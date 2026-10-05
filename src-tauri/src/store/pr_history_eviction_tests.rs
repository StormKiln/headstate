//! Eviction fault stages and cross-connection snapshots on the production DB.
use super::*;
use crate::store::{open_db, pr_slice, stats};
use std::path::{Path, PathBuf};

const DAY: &str = "2026-01-15";
const SCOPES: [&str; 4] = [
    "merged|*|org:fixture",
    "opened|*|org:fixture",
    "merged|*|repo:fixture/repo",
    "opened|*|repo:fixture/repo",
];
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("eviction.db");
    let mut conn = open_db(&path).unwrap();
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    for (i, key) in SCOPES.iter().enumerate() {
        // The same repository/PR is held separately in overlapping scopes and
        // measures. Only merged-org and opened-repository copies are oldest victims.
        let rows: Vec<_> = (1..=150)
            .map(|number| StoredPr {
                repo: "fixture/repo".into(),
                number,
                merged_at: DAY.into(),
                title: "fixture".into(),
                url: "https://example.test/pr".into(),
                author: "fixture".into(),
                cycle_time_hours: 1.0,
                size: 1,
                additions: 1,
                deletions: 0,
                changed_files: 1,
                reviews_received: 0,
            })
            .collect();
        put_many_in(&tx, key, DAY, DAY, &rows, Utc::now()).unwrap();
        tx.execute(
            "UPDATE pr_history SET stored_at=?2 WHERE scope_key=?1",
            params![key, if i == 0 || i == 3 { "1900" } else { "9999" }],
        )
        .unwrap();
        for (from, to, state) in [
            (DAY, DAY, pr_slice::SliceState::Complete),
            (
                "2026-01-01",
                "2026-01-31",
                pr_slice::SliceState::Irreducible,
            ),
        ] {
            pr_slice::put(
                &tx,
                key,
                &pr_slice::SliceRow {
                    from: from.into(),
                    to: to.into(),
                    state,
                    issue_count: 150,
                    retrieved: 150,
                    refused_fields: 0,
                },
                Utc::now(),
            )
            .unwrap();
        }
        tx.execute("INSERT INTO pr_backfill_page(viewer,scope_key,day,payload,attempted) VALUES ('fixture',?1,?2,'retained fixture cursor',1)",params![key,DAY]).unwrap();
        tx.execute(
            "INSERT INTO pr_scope_evidence(scope_key,revision) VALUES (?1,5)",
            [key],
        )
        .unwrap();
    }
    tx.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<199401)
        INSERT INTO pr_history(scope_key,slice_from,slice_to,repo,number,merged_at,title,url,author,cycle_time_hours,size,additions,deletions,changed_files,reviews_received,stored_at)
        SELECT 'padding','2026-01-01','2026-01-31','fixture/padding',x,'2026-01-15','fixture','https://example.test/pr','fixture',1,1,1,0,1,0,'2026' FROM n;").unwrap();
    stats::put(
        &tx,
        "materialized-fixture",
        DAY,
        DAY,
        150,
        true,
        "independent-complete-answer",
        Utc::now(),
    )
    .unwrap();
    tx.commit().unwrap();
    assert_before(&conn);
    (dir, path)
}
fn state(conn: &Connection, key: &str) -> (u64, i64, i64, i64) {
    let rows = count(conn, key, DAY, DAY).unwrap();
    let proofs = conn
        .query_row(
            "SELECT COUNT(*) FROM pr_slice WHERE scope_key=?1",
            [key],
            |r| r.get(0),
        )
        .unwrap();
    let cursors = conn
        .query_row(
            "SELECT COUNT(*) FROM pr_backfill_page WHERE scope_key=?1",
            [key],
            |r| r.get(0),
        )
        .unwrap();
    let revision = conn
        .query_row(
            "SELECT revision FROM pr_scope_evidence WHERE scope_key=?1",
            [key],
            |r| r.get(0),
        )
        .unwrap();
    (rows, proofs, cursors, revision)
}
fn assert_before(conn: &Connection) {
    assert_eq!(total_rows(conn).unwrap(), 200001);
    for key in SCOPES {
        assert_eq!(state(conn, key), (150, 2, 1, 5));
    }
}
fn assert_after(conn: &Connection) {
    assert_eq!(total_rows(conn).unwrap(), 180000);
    for key in [SCOPES[0], SCOPES[3]] {
        assert_eq!(state(conn, key), (0, 0, 0, 6));
        assert_eq!(
            pr_slice::uncovered_days(conn, key, DAY, DAY).unwrap(),
            vec![DAY]
        );
    }
    for key in [SCOPES[1], SCOPES[2]] {
        assert_eq!(state(conn, key), (150, 2, 1, 5));
        assert!(pr_slice::uncovered_days(conn, key, DAY, DAY)
            .unwrap()
            .is_empty());
    }
    let payload: String = conn
        .query_row(
            "SELECT payload FROM stats_cache WHERE key='materialized-fixture'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(payload, "independent-complete-answer");
}

#[test]
fn eviction_partitions_opened_merged_and_overlapping_org_repository_copies() {
    let (_dir, path) = fixture();
    assert_eq!(prune(&open_db(&path).unwrap()).unwrap(), 20001);
    assert_after(&open_db(&path).unwrap());
}

fn fault_rollback(path: &Path, trigger: &str, expected: &str) {
    {
        let conn = open_db(path).unwrap();
        conn.execute_batch(trigger).unwrap();
        let error = prune(&conn).unwrap_err().to_string();
        assert!(error.contains(expected), "wrong fault stage: {error}");
    }
    // A different physical connection must see all original evidence after
    // the function's transaction was dropped, not a partly repaired view.
    assert_before(&open_db(path).unwrap());
}

#[test]
fn eviction_rolls_back_failures_after_invalidation_and_after_all_deletions() {
    let (_dir, path) = fixture();
    fault_rollback(&path,"CREATE TRIGGER fault_stage BEFORE DELETE ON pr_history BEGIN
        SELECT CASE WHEN (SELECT COUNT(*) FROM pr_slice WHERE scope_key IN ('merged|*|org:fixture','opened|*|repo:fixture/repo')) != 0 THEN RAISE(ABORT,'proofs not invalidated') END;
        SELECT CASE WHEN (SELECT COUNT(*) FROM pr_backfill_page WHERE scope_key IN ('merged|*|org:fixture','opened|*|repo:fixture/repo')) != 0 THEN RAISE(ABORT,'cursors not invalidated') END;
        SELECT CASE WHEN (SELECT MIN(revision) FROM pr_scope_evidence WHERE scope_key IN ('merged|*|org:fixture','opened|*|repo:fixture/repo')) != 6 THEN RAISE(ABORT,'revision not advanced') END;
        SELECT CASE WHEN (SELECT COUNT(*) FROM pr_history) != 200001 THEN RAISE(ABORT,'history already deleted') END;
        SELECT RAISE(ABORT,'after invalidation before deletion'); END;","after invalidation before deletion");
    open_db(&path)
        .unwrap()
        .execute_batch("DROP TRIGGER fault_stage;")
        .unwrap();
    fault_rollback(&path,"CREATE TEMP TABLE stats_prune_victims(rowid INTEGER PRIMARY KEY,scope_key TEXT NOT NULL,day TEXT NOT NULL);
        CREATE TEMP TRIGGER fault_stage BEFORE DELETE ON stats_prune_victims WHEN (SELECT COUNT(*) FROM pr_history)=180000 BEGIN
        SELECT CASE WHEN (SELECT COUNT(*) FROM pr_slice WHERE scope_key IN ('merged|*|org:fixture','opened|*|repo:fixture/repo')) != 0 THEN RAISE(ABORT,'proofs still present') END;
        SELECT RAISE(ABORT,'after all deletions before commit'); END;","after all deletions before commit");
    // The second fault is on final temporary-victim cleanup: the history
    // DELETE statement has finished and commit has not occurred. Its temporary
    // trigger disappears with that connection; no persistent fixture cleanup.
    assert_eq!(prune(&open_db(&path).unwrap()).unwrap(), 20001);
    assert_after(&open_db(&path).unwrap());
}

#[test]
fn eviction_readers_keep_coherent_snapshots_and_other_writers_wait() {
    let (_dir, path) = fixture();
    // Open all connections through the real WAL/5s busy-timeout policy before
    // acquiring the writer; migration itself must not be the contention test.
    let mut writer = open_db(&path).unwrap();
    let mut reader = open_db(&path).unwrap();
    let contender = open_db(&path).unwrap();
    let mode: String = reader
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let timeout: i64 = contender
        .pragma_query_value(None, "busy_timeout", |r| r.get(0))
        .unwrap();
    assert_eq!(timeout, 5000);
    let read = reader.transaction().unwrap();
    assert_before(&read);
    let tx = writer
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert_eq!(prune_in(&tx).unwrap(), 20001);
    assert_after(&tx);
    // While the writer has deleted the rows but not committed, the second
    // connection sees old proofs AND their rows together.
    assert_before(&read);
    let (started, ready) = std::sync::mpsc::channel();
    let (finished, done) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        started.send(()).unwrap();
        finished.send(prune(&contender)).unwrap();
    });
    ready
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(
        done.recv_timeout(std::time::Duration::from_millis(150))
            .is_err(),
        "a competing immediate writer must wait for commit"
    );
    tx.commit().unwrap();
    assert_eq!(
        done.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        0
    );
    worker.join().unwrap();
    // WAL keeps the old read snapshot coherent even after the writer commits.
    assert_before(&read);
    read.commit().unwrap();
    assert_after(&reader);
    assert_after(&open_db(&path).unwrap());
}
