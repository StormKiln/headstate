//! Per-source persistence. Only migration and test snapshots assemble a Cursor.
use super::{Checkpoint, Cursor};
use crate::store::{settings, StoreError};
use rusqlite::{Connection, OptionalExtension};
use std::collections::BTreeMap;

pub(crate) fn migrate_legacy(conn: &Connection) -> Result<(), StoreError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM settings WHERE key=?1)",
        [settings::keys::CLAUDE_PR_LINK_CURSOR],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    if conn.is_autocommit() {
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        migrate_legacy(&tx)?;
        tx.commit()?;
        return Ok(());
    }
    let Some(cursor) = settings::get_strict::<Cursor>(conn, settings::keys::CLAUDE_PR_LINK_CURSOR)?
    else {
        return Ok(());
    };
    // Parse the entire legacy value before writing anything. Retain checkpoint-
    // only entries too: historical parser evidence must not be silently lost.
    conn.execute("INSERT INTO claude_link_baseline VALUES(1, ?1) ON CONFLICT(singleton) DO UPDATE SET since_ms=excluded.since_ms", [cursor.since_ms])?;
    for (path, offset) in &cursor.offsets {
        let payload = cursor
            .checkpoints
            .get(path)
            .map(serde_json::to_string)
            .transpose()?;
        put(conn, path, *offset, payload.as_deref())?;
    }
    for (path, checkpoint) in &cursor.checkpoints {
        if !cursor.offsets.contains_key(path) {
            put(
                conn,
                path,
                checkpoint.to,
                Some(&serde_json::to_string(checkpoint)?),
            )?;
        }
    }
    conn.execute(
        "DELETE FROM settings WHERE key=?1",
        [settings::keys::CLAUDE_PR_LINK_CURSOR],
    )?;
    Ok(())
}

pub(super) fn baseline(conn: &Connection) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT since_ms FROM claude_link_baseline WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

pub(super) fn offsets(conn: &Connection) -> Result<BTreeMap<String, u64>, StoreError> {
    let mut stmt = conn.prepare("SELECT path, offset FROM claude_link_checkpoint")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    rows.map(|row| {
        let (path, offset) = row?;
        Ok((path, serde_json::from_str(&offset)?))
    })
    .collect()
}

pub(super) fn checkpoint(conn: &Connection, path: &str) -> Result<Option<Checkpoint>, StoreError> {
    let raw: Option<Option<String>> = conn
        .query_row(
            "SELECT payload FROM claude_link_checkpoint WHERE path=?1",
            [path],
            |r| r.get(0),
        )
        .optional()?;
    raw.flatten()
        .map(|raw| serde_json::from_str(&raw).map_err(StoreError::from))
        .transpose()
}

pub(super) fn put(
    conn: &Connection,
    path: &str,
    offset: u64,
    payload: Option<&str>,
) -> Result<(), StoreError> {
    conn.execute("INSERT INTO claude_link_checkpoint(path, offset, payload) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET offset=excluded.offset,payload=excluded.payload", rusqlite::params![path, offset.to_string(), payload])?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn checkpoint_cursor(conn: &Connection) -> Result<Option<Cursor>, StoreError> {
    let Some(since_ms) = baseline(conn)? else {
        return Ok(None);
    };
    let offsets = offsets(conn)?;
    let mut checkpoints = BTreeMap::new();
    for path in offsets.keys() {
        if let Some(checkpoint) = checkpoint(conn, path)? {
            checkpoints.insert(path.clone(), checkpoint);
        }
    }
    Ok(Some(Cursor {
        since_ms,
        offsets,
        checkpoints,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn.execute_batch("DROP TABLE claude_link_checkpoint; DROP TABLE claude_link_baseline; PRAGMA user_version=46;").unwrap();
        conn
    }

    #[test]
    fn migration_preserves_offset_only_and_parser_evidence() {
        let conn = legacy_db();
        let checkpoint = Checkpoint {
            to: 7,
            line_buffer: b"partial".to_vec(),
            hashes: vec![super::super::digest(b"evidence")],
            ..Default::default()
        };
        let cursor = Cursor {
            since_ms: 123,
            offsets: BTreeMap::from([("offset-only".into(), u64::MAX), ("pending".into(), 7)]),
            checkpoints: BTreeMap::from([
                ("pending".into(), checkpoint.clone()),
                ("checkpoint-only".into(), checkpoint),
            ]),
        };
        settings::set(&conn, settings::keys::CLAUDE_PR_LINK_CURSOR, &cursor).unwrap();
        crate::store::migrate(&conn).unwrap();
        let mut expected = cursor;
        expected.offsets.insert("checkpoint-only".into(), 7);
        assert_eq!(checkpoint_cursor(&conn).unwrap(), Some(expected));
        assert!(
            settings::get_strict::<Cursor>(&conn, settings::keys::CLAUDE_PR_LINK_CURSOR)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn malformed_legacy_rolls_back_schema_version_and_retains_original() {
        let conn = legacy_db();
        settings::set(&conn, settings::keys::CLAUDE_PR_LINK_CURSOR, &"malformed").unwrap();
        assert!(crate::store::migrate(&conn).is_err());
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            46
        );
        assert_eq!(
            settings::get_strict::<String>(&conn, settings::keys::CLAUDE_PR_LINK_CURSOR)
                .unwrap()
                .as_deref(),
            Some("malformed")
        );
        assert!(conn.prepare("SELECT * FROM claude_link_baseline").is_err());
    }

    #[test]
    fn failed_conversion_and_standalone_reset_are_atomic() {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let cursor = Cursor {
            since_ms: 1,
            offsets: BTreeMap::from([("old".into(), 8)]),
            ..Default::default()
        };
        settings::set(&conn, settings::keys::CLAUDE_PR_LINK_CURSOR, &cursor).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_source BEFORE INSERT ON claude_link_checkpoint BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(migrate_legacy(&conn).is_err());
        assert_eq!(baseline(&conn).unwrap(), None);
        assert_eq!(
            settings::get_strict::<Cursor>(&conn, settings::keys::CLAUDE_PR_LINK_CURSOR).unwrap(),
            Some(cursor.clone())
        );
        conn.execute_batch("DROP TRIGGER fail_source").unwrap();
        migrate_legacy(&conn).unwrap();
        let generation =
            super::super::super::transcript::population_generation(&conn, "links").unwrap();
        conn.execute_batch("CREATE TRIGGER fail_reset BEFORE INSERT ON claude_link_baseline BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(super::super::reset(&conn, 99).is_err());
        assert_eq!(checkpoint_cursor(&conn).unwrap(), Some(cursor));
        assert!(
            super::super::super::transcript::generation_current(&conn, "links", generation)
                .unwrap()
        );
    }

    #[test]
    fn migrated_empty_baseline_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.db");
        let conn = crate::store::open_db(&path).unwrap();
        settings::set(
            &conn,
            settings::keys::CLAUDE_PR_LINK_CURSOR,
            &Cursor {
                since_ms: 91,
                ..Default::default()
            },
        )
        .unwrap();
        migrate_legacy(&conn).unwrap();
        drop(conn);
        let conn = crate::store::open_db(&path).unwrap();
        assert_eq!(checkpoint_cursor(&conn).unwrap().unwrap().since_ms, 91);
        assert!(offsets(&conn).unwrap().is_empty());
    }

    #[test]
    fn absent_legacy_does_not_require_writer_admission() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer.db");
        let reader = crate::store::open_db(&path).unwrap();
        reader.busy_timeout(std::time::Duration::ZERO).unwrap();
        let writer = crate::store::open_db(&path).unwrap();
        let tx =
            rusqlite::Transaction::new_unchecked(&writer, rusqlite::TransactionBehavior::Immediate)
                .unwrap();
        migrate_legacy(&reader).unwrap();
        assert!(reader.is_autocommit());
        tx.rollback().unwrap();
    }

    /// Synthetic evidence only. Run explicitly with --ignored --nocapture.
    #[test]
    #[ignore]
    fn checkpoint_storage_measurement() {
        use std::time::Instant;
        const SOURCES: usize = 32;
        let dir = tempfile::tempdir().unwrap();
        let mut cursor = Cursor::default();
        for i in 0..SOURCES {
            let path = format!("synthetic/source-{i:03}.jsonl");
            let checkpoint = Checkpoint {
                to: 4 * 1024 * 1024 + 65536,
                hashes: vec![super::super::digest(b"synthetic block"); 1024],
                line_buffer: vec![b'x'; 65536],
                ..Default::default()
            };
            cursor.offsets.insert(path.clone(), checkpoint.to);
            cursor.checkpoints.insert(path, checkpoint);
        }
        let started = Instant::now();
        let legacy = serde_json::to_string(&cursor).unwrap();
        let legacy_serialize = started.elapsed();
        let path = cursor.checkpoints.keys().next().unwrap().clone();
        let mut changed = cursor.checkpoints[&path].clone();
        changed.line_buffer.push(b'y');
        changed.to += 1;
        let started = Instant::now();
        let payload = serde_json::to_string(&changed).unwrap();
        let source_serialize = started.elapsed();
        let heap = |c: &Checkpoint| {
            c.line_buffer.capacity()
                + c.hashes.capacity() * std::mem::size_of::<String>()
                + c.hashes.iter().map(String::capacity).sum::<usize>()
        };
        println!("sources={SOURCES} legacy_json={} source_json={} legacy_serialize_us={} source_serialize_us={} legacy_payload_heap={} one_source_payload_heap={} digest_hex_bytes={} binary_digest_bytes={}", legacy.len(), payload.len(), legacy_serialize.as_micros(), source_serialize.as_micros(), cursor.checkpoints.values().map(heap).sum::<usize>(), heap(&changed), 1024*64, 1024*32);

        for per_source in [false, true] {
            let db = dir
                .path()
                .join(if per_source { "rows.db" } else { "legacy.db" });
            let conn = crate::store::open_db(&db).unwrap();
            conn.execute_batch("PRAGMA wal_autocheckpoint=0").unwrap();
            settings::set(&conn, settings::keys::CLAUDE_PR_LINK_CURSOR, &cursor).unwrap();
            if per_source {
                migrate_legacy(&conn).unwrap();
            }
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
            let updated_legacy;
            let (data, key) = if per_source {
                (payload.as_str(), path.as_str())
            } else {
                let mut updated = cursor.clone();
                updated.checkpoints.insert(path.clone(), changed.clone());
                updated.offsets.insert(path.clone(), changed.to);
                updated_legacy = serde_json::to_string(&updated).unwrap();
                (
                    updated_legacy.as_str(),
                    settings::keys::CLAUDE_PR_LINK_CURSOR,
                )
            };
            let changes = conn.total_changes();
            let started = Instant::now();
            let tx = conn.unchecked_transaction().unwrap();
            let source_sql = format!("INSERT INTO claude_link_checkpoint(path,offset,payload) VALUES(?2,'{}',?1) ON CONFLICT(path) DO UPDATE SET offset=excluded.offset,payload=excluded.payload", changed.to);
            let mut stmt = tx.prepare(if per_source { &source_sql } else {
                "INSERT INTO settings(key,value) VALUES(?2,?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value"
            }).unwrap();
            stmt.execute([data, key]).unwrap();
            let vm = stmt.get_status(rusqlite::StatementStatus::VmStep);
            drop(stmt);
            tx.commit().unwrap();
            let elapsed = started.elapsed();
            let wal = std::fs::metadata(db.with_extension("db-wal"))
                .unwrap()
                .len();
            let pages: i64 = conn
                .query_row("PRAGMA page_count", [], |r| r.get(0))
                .unwrap();
            let page_size: i64 = conn
                .query_row("PRAGMA page_size", [], |r| r.get(0))
                .unwrap();
            let free_pages: i64 = conn
                .query_row("PRAGMA freelist_count", [], |r| r.get(0))
                .unwrap();
            println!("per_source={per_source} vm_steps={vm} mutations={} transaction_us={} wal_bytes={wal} db_allocated_bytes={} db_live_page_bytes={}",conn.total_changes()-changes, elapsed.as_micros(), pages*page_size, (pages-free_pages)*page_size);
        }
        let long = Checkpoint {
            line_buffer: vec![b'x'; 1024 * 1024],
            ..Default::default()
        };
        println!(
            "long_partial_raw={} long_partial_json={} long_partial_payload_heap={}",
            long.line_buffer.len(),
            serde_json::to_string(&long).unwrap().len(),
            heap(&long)
        );
    }
}
