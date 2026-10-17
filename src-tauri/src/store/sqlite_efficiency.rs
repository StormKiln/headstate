//! Synthetic bundled-SQLite measurements for release 10.7. Never opens user data.
use rusqlite::{params, Connection, StatementStatus};

fn seed_fts(conn: &mut Connection, rows: usize, mapped: bool) {
    let tx = conn.transaction().unwrap();
    for i in 0..rows {
        let id = format!("synthetic-{i}");
        tx.execute(
            "INSERT INTO claude_transcript_fts(session_id, body, body_masked) VALUES(?1, ?2, ?2)",
            params![id, "synthetic accepted evidence ".repeat(20)],
        )
        .unwrap();
        if mapped {
            tx.execute("INSERT INTO claude_fts_identity(rowid, session_id) VALUES(last_insert_rowid(), ?1)", [&id]).unwrap();
        }
    }
    tx.commit().unwrap();
}

fn identity_work(conn: &Connection, mapped: bool) -> i32 {
    let sql = if mapped {
        "SELECT rowid FROM claude_fts_identity WHERE session_id=?1"
    } else {
        "SELECT rowid FROM claude_transcript_fts WHERE session_id=?1"
    };
    let mut stmt = conn.prepare(sql).unwrap();
    let rows = stmt
        .query_map(["synthetic-500"], |r| r.get::<_, i64>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 1);
    stmt.get_status(StatementStatus::VmStep)
}

#[test]
fn fts_identity_lookup_uses_index_instead_of_corpus_scan() {
    let mut conn = Connection::open_in_memory().unwrap();
    super::migrate(&conn).unwrap();
    seed_fts(&mut conn, 1000, true);
    assert!(identity_work(&conn, true) < 30);
    assert!(identity_work(&conn, false) > 3000);
    let plans = conn.prepare("EXPLAIN QUERY PLAN SELECT rowid FROM claude_fts_identity WHERE session_id='synthetic-500'").unwrap()
        .query_map([], |r| r.get::<_, String>(3)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    assert!(
        plans
            .iter()
            .any(|p| p.contains("claude_fts_identity_session")),
        "{plans:?}"
    );
}

#[test]
#[ignore = "synthetic bundled SQLite performance measurement; run explicitly"]
fn fts_identity_writer_measurements() {
    eprintln!("sqlite_version={}", rusqlite::version());
    eprintln!("rows,mapped,lookup_vm_steps,batch100_writer_us,wal_bytes");
    for rows in [1000, 10000, 50000] {
        for mapped in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("synthetic.sqlite");
            let mut conn = Connection::open(&path).unwrap();
            super::migrate(&conn).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
            seed_fts(&mut conn, rows, mapped);
            let work = identity_work(&conn, mapped);
            if mapped {
                assert!(work < 30);
            }
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
            let started = std::time::Instant::now();
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            for i in 0..100 {
                let id = format!("synthetic-{i}");
                if mapped {
                    tx.execute("DELETE FROM claude_transcript_fts WHERE rowid IN (SELECT rowid FROM claude_fts_identity WHERE session_id=?1)", [&id]).unwrap();
                    tx.execute("DELETE FROM claude_fts_identity WHERE session_id=?1", [&id])
                        .unwrap();
                } else {
                    tx.execute(
                        "DELETE FROM claude_transcript_fts WHERE session_id=?1",
                        [&id],
                    )
                    .unwrap();
                }
                tx.execute("INSERT INTO claude_transcript_fts(session_id, body, body_masked) VALUES(?1, ?2, ?2)", params![id, "synthetic replacement evidence ".repeat(20)]).unwrap();
                if mapped {
                    tx.execute("INSERT INTO claude_fts_identity(rowid, session_id) VALUES(last_insert_rowid(), ?1)", [&id]).unwrap();
                }
            }
            tx.commit().unwrap();
            let elapsed = started.elapsed().as_micros();
            let wal = std::fs::metadata(dir.path().join("synthetic.sqlite-wal"))
                .unwrap()
                .len();
            eprintln!("{rows},{mapped},{work},{elapsed},{wal}");
        }
    }
}

#[test]
#[ignore = "synthetic bundled SQLite sort/index measurement; run explicitly"]
fn session_sort_measurements() {
    eprintln!("sessions,aligned,session_vm,session_sorts,run_vm,run_sorts,read_us,db_bytes,insert100_us,wal_bytes");
    for count in [1000, 10000, 50000] {
        for aligned in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut conn = Connection::open(dir.path().join("synthetic.sqlite")).unwrap();
            super::migrate(&conn).unwrap();
            if !aligned {
                conn.execute_batch("DROP INDEX claude_session_activity; CREATE INDEX claude_session_activity ON claude_session(last_activity_at DESC);").unwrap();
            }
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
            let insert = |conn: &Connection, start: usize, end: usize| {
                for i in start..end {
                    let id = format!("synthetic-{i}");
                    let timestamp = format!("2026-01-{:02}T00:00:00Z", i % 28 + 1);
                    let activity = (i % 4 != 0).then_some(timestamp.as_str());
                    conn.execute("INSERT INTO claude_session(session_id, first_seen_at, last_activity_at, opening_prompt) VALUES(?1, ?2, ?3, 'synthetic prompt')", params![id, timestamp, activity]).unwrap();
                    for day in [1, 2, 3] {
                        conn.execute("INSERT INTO claude_run(session_id, pid, started_at) VALUES(?1, ?2, ?3)", params![id, day, format!("2026-01-{day:02}T00:00:00Z")]).unwrap();
                    }
                }
            };
            let tx = conn.transaction().unwrap();
            insert(&tx, 0, count);
            tx.commit().unwrap();
            let session_sql = format!("SELECT session_id, name, cwd, git_branch, claude_version, transcript_path, first_seen_at, last_activity_at, opening_prompt FROM claude_session ORDER BY {}last_activity_at DESC, first_seen_at DESC", if aligned { "" } else { "last_activity_at IS NULL, " });
            let run_sql = format!("SELECT session_id, pid, pid_start_time, ended_at, end_reason FROM claude_run ORDER BY {}started_at DESC", if aligned { "session_id, " } else { "" });
            let measure = |sql: &str| {
                let mut stmt = conn.prepare(sql).unwrap();
                let mut rows = stmt.query([]).unwrap();
                while rows.next().unwrap().is_some() {}
                drop(rows);
                (
                    stmt.get_status(StatementStatus::VmStep),
                    stmt.get_status(StatementStatus::Sort),
                )
            };
            let started = std::time::Instant::now();
            let (session_vm, session_sorts) = measure(&session_sql);
            let (run_vm, run_sorts) = measure(&run_sql);
            let read_us = started.elapsed().as_micros();
            if aligned {
                assert_eq!((session_sorts, run_sorts), (0, 0));
            }
            let pages: i64 = conn
                .query_row("PRAGMA page_count", [], |r| r.get(0))
                .unwrap();
            let page_size: i64 = conn
                .query_row("PRAGMA page_size", [], |r| r.get(0))
                .unwrap();
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
            let started = std::time::Instant::now();
            let tx = conn.transaction().unwrap();
            insert(&tx, count, count + 100);
            tx.commit().unwrap();
            let write_us = started.elapsed().as_micros();
            let wal = std::fs::metadata(dir.path().join("synthetic.sqlite-wal"))
                .unwrap()
                .len();
            eprintln!("{count},{aligned},{session_vm},{session_sorts},{run_vm},{run_sorts},{read_us},{},{write_us},{wal}", pages * page_size);
        }
    }
}
