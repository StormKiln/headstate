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

/// Metadata serialization temporarily borrows the five raw byte buffers and
/// dirty hashes out of the checkpoint; it never clones their payloads.
pub(super) fn encode_header(cp: &mut Checkpoint) -> Result<String, StoreError> {
    let line = std::mem::take(&mut cp.line_buffer);
    let tail = std::mem::take(&mut cp.tail);
    let last = std::mem::take(&mut cp.last_block);
    let audit = std::mem::take(&mut cp.audit_buffer);
    let proof = std::mem::take(&mut cp.provenance.buffer);
    let hashes = std::mem::take(&mut cp.hashes);
    let encoded = serde_json::to_string(cp);
    cp.line_buffer = line;
    cp.tail = tail;
    cp.last_block = last;
    cp.audit_buffer = audit;
    cp.provenance.buffer = proof;
    cp.hashes = hashes;
    Ok(encoded?)
}

pub(super) fn load(conn: &Connection, path: &str) -> Result<Option<Checkpoint>, StoreError> {
    let raw: Option<(Option<String>, usize)> = conn
        .query_row(
            "SELECT payload,hash_count FROM claude_link_checkpoint WHERE path=?1",
            [path],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)),
        )
        .optional()?;
    let Some((Some(raw), count)) = raw else {
        return Ok(None);
    };
    let mut cp: Checkpoint = serde_json::from_str(&raw)?;
    cp.hash_base = count;
    conn.query_row(
        "SELECT line,tail,last_block,audit,provenance FROM claude_link_bytes WHERE path=?1",
        [path],
        |r| {
            cp.line_buffer = r.get(0)?;
            cp.tail = r.get(1)?;
            cp.last_block = r.get(2)?;
            cp.audit_buffer = r.get(3)?;
            cp.provenance.buffer = r.get(4)?;
            Ok(())
        },
    )?;
    Ok(Some(cp))
}

pub(super) fn hash(conn: &Connection, path: &str, block: usize) -> Result<String, StoreError> {
    let bytes: Vec<u8> = conn.query_row(
        "SELECT digest FROM claude_link_hash WHERE path=?1 AND block=?2",
        rusqlite::params![path, block as i64],
        |r| r.get(0),
    )?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn binary_digest(digest: &str) -> Result<Vec<u8>, StoreError> {
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(
            rusqlite::Error::InvalidParameterName("malformed checkpoint digest".into()).into(),
        );
    }
    Ok((0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&digest[i..i + 2], 16).expect("validated ASCII hex"))
        .collect())
}

/// Must run in the source transaction with its link inserts and generation fence.
pub(super) fn write(
    conn: &Connection,
    path: &str,
    cp: &Checkpoint,
    payload: &str,
) -> Result<(), StoreError> {
    conn.execute("INSERT INTO claude_link_checkpoint(path,offset,payload,hash_count) VALUES(?1,?2,?3,?4) ON CONFLICT(path) DO UPDATE SET offset=excluded.offset,payload=excluded.payload,hash_count=excluded.hash_count", rusqlite::params![path,cp.to.to_string(),payload,(cp.hash_base+cp.hashes.len()) as i64])?;
    conn.execute("INSERT INTO claude_link_bytes(path,line,tail,last_block,audit,provenance) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(path) DO UPDATE SET line=excluded.line,tail=excluded.tail,last_block=excluded.last_block,audit=excluded.audit,provenance=excluded.provenance", rusqlite::params![path,cp.line_buffer,cp.tail,cp.last_block,cp.audit_buffer,cp.provenance.buffer])?;
    // Existing history is never rewritten on append. A replay has base zero and
    // atomically replaces all old evidence, including a shorter replacement.
    conn.execute(
        "DELETE FROM claude_link_hash WHERE path=?1 AND block>=?2",
        rusqlite::params![path, cp.hash_base as i64],
    )?;
    let mut stmt =
        conn.prepare("INSERT INTO claude_link_hash(path,block,digest) VALUES(?1,?2,?3)")?;
    for (i, digest) in cp.hashes.iter().enumerate() {
        stmt.execute(rusqlite::params![
            path,
            (cp.hash_base + i) as i64,
            binary_digest(digest)?
        ])?;
    }
    Ok(())
}

pub(super) fn continuation(conn: &Connection) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT path FROM claude_link_continuation WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}
pub(super) fn advance(conn: &Connection, path: &str) -> Result<(), StoreError> {
    conn.execute("INSERT INTO claude_link_continuation VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET path=excluded.path", [path])?;
    Ok(())
}

pub(crate) fn migrate_compact(conn: &Connection) -> Result<(), StoreError> {
    let mut stmt =
        conn.prepare("SELECT path,offset,payload FROM claude_link_checkpoint ORDER BY path")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        let offset: String = row.get(1)?;
        let _: u64 = serde_json::from_str(&offset)?;
        if let Some(raw) = row.get::<_, Option<String>>(2)? {
            let mut cp: Checkpoint = serde_json::from_str(&raw)?;
            let payload = encode_header(&mut cp)?;
            write(conn, &path, &cp, &payload)?;
            // Legacy offset and checkpoint can differ: preserve both exactly.
            conn.execute(
                "UPDATE claude_link_checkpoint SET offset=?2 WHERE path=?1",
                rusqlite::params![path, offset],
            )?;
        }
    }
    Ok(())
}

pub(super) fn put(
    conn: &Connection,
    path: &str,
    offset: u64,
    payload: Option<&str>,
) -> Result<(), StoreError> {
    let compact: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='claude_link_bytes')",
        [],
        |r| r.get(0),
    )?;
    if compact {
        if let Some(payload) = payload {
            let mut cp: Checkpoint = serde_json::from_str(payload)?;
            let header = encode_header(&mut cp)?;
            write(conn, path, &cp, &header)?;
            conn.execute(
                "UPDATE claude_link_checkpoint SET offset=?2 WHERE path=?1",
                rusqlite::params![path, offset.to_string()],
            )?;
            return Ok(());
        }
    }
    conn.execute("INSERT INTO claude_link_checkpoint(path,offset,payload) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET offset=excluded.offset,payload=excluded.payload", rusqlite::params![path,offset.to_string(),payload])?;
    Ok(())
}

#[cfg(test)]
pub(super) fn checkpoint(conn: &Connection, path: &str) -> Result<Option<Checkpoint>, StoreError> {
    let Some(mut cp) = load(conn, path)? else {
        return Ok(None);
    };
    for i in 0..cp.hash_base {
        cp.hashes.push(hash(conn, path, i)?);
    }
    cp.hash_base = 0;
    Ok(Some(cp))
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

    #[test]
    fn compact_migration_preserves_all_fields_and_offset_membership_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("migration.db");
        let conn = Connection::open(&path).unwrap();
        crate::store::historical_schema(&conn, 48);
        let mut cp = Checkpoint {
            to: 7,
            line_buffer: vec![0, 255, 13, 10, 42],
            tail: vec![255, 0],
            last_block: vec![7; 4096],
            audit_buffer: vec![8; 17],
            hashes: vec![super::super::digest(b"old"); 2048],
            ..Default::default()
        };
        cp.parser.line_links(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"pending","name":"Bash","input":{"command":"gh pr create"}}]}}"#, "s");
        cp.pending_origins.insert("pending".into(), (2, 5));
        cp.provenance.buffer = vec![9; 23];
        cp.provenance.ranges = vec![(3, 10)];
        cp.provenance.block = 3;
        cp.audit_next = 9;
        cp.audit_end = 20;
        cp.anchor_checked = 2;
        conn.execute("INSERT INTO claude_link_baseline VALUES(1,44)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO claude_link_checkpoint VALUES('evidence','99',?1)",
            [serde_json::to_string(&cp).unwrap()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO claude_link_checkpoint VALUES('offset-only','18446744073709551615',NULL)",
            [],
        )
        .unwrap();
        crate::store::migrate(&conn).unwrap();
        drop(conn);
        let conn = crate::store::open_db(&path).unwrap();
        assert_eq!(checkpoint(&conn, "evidence").unwrap(), Some(cp));
        assert_eq!(offsets(&conn).unwrap()["evidence"], 99);
        assert_eq!(offsets(&conn).unwrap()["offset-only"], u64::MAX);
        assert!(checkpoint(&conn, "offset-only").unwrap().is_none());
        assert_eq!(
            conn.query_row(
                "SELECT length(line) FROM claude_link_bytes WHERE path='evidence'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            5
        );
        let lazy = load(&conn, "evidence").unwrap().unwrap();
        assert!(lazy.hashes.is_empty());
        assert_eq!(lazy.hash_base, 2048);
    }

    #[test]
    fn compact_migration_malformed_or_failed_commit_rolls_back_and_reopens() {
        for failure in ["payload", "digest", "offset", "row", "commit"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("rollback.db");
            let conn = Connection::open(&path).unwrap();
            crate::store::historical_schema(&conn, 48);
            let cp = Checkpoint {
                hashes: vec![if failure == "digest" {
                    "malformed".into()
                } else {
                    super::super::digest(b"ok")
                }],
                ..Default::default()
            };
            let raw = if failure == "payload" {
                "malformed".into()
            } else {
                serde_json::to_string(&cp).unwrap()
            };
            conn.execute(
                "INSERT INTO claude_link_checkpoint VALUES('old','12',?1)",
                [&raw],
            )
            .unwrap();
            if failure == "offset" {
                conn.execute("UPDATE claude_link_checkpoint SET offset='invalid'", [])
                    .unwrap();
            }
            if failure == "row" {
                conn.execute_batch("CREATE TRIGGER fail_conversion BEFORE INSERT ON claude_link_checkpoint BEGIN SELECT RAISE(ABORT,'injected row failure'); END;").unwrap();
            }
            if failure == "commit" {
                conn.commit_hook(Some(|| true)).unwrap();
            }
            assert!(crate::store::migrate(&conn).is_err(), "{failure}");
            drop(conn);
            let conn = Connection::open(&path).unwrap();
            assert_eq!(
                conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                48
            );
            assert_eq!(
                conn.query_row("SELECT payload FROM claude_link_checkpoint", [], |r| r
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
                raw
            );
            assert!(conn.prepare("SELECT * FROM claude_link_bytes").is_err());
        }
    }

    #[test]
    fn oversized_legacy_metadata_is_preserved_without_new_evidence_cap() {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::historical_schema(&conn, 48);
        // Historical JSON was not a bounded schema. This deliberately exceeds
        // today's parser ID limit; migration is lossless, not a new admission gate.
        let id = "x".repeat(1024 * 1024);
        let mut value = serde_json::to_value(Checkpoint::default()).unwrap();
        value["parser"]["pending"] = serde_json::json!([id]);
        value["pending_origins"] = serde_json::json!({id: [0,1]});
        let cp: Checkpoint = serde_json::from_value(value).unwrap();
        conn.execute(
            "INSERT INTO claude_link_checkpoint VALUES('historical','0',?1)",
            [serde_json::to_string(&cp).unwrap()],
        )
        .unwrap();
        let before = std::time::Instant::now();
        crate::store::migrate(&conn).unwrap();
        let migrate_us = before.elapsed().as_micros();
        let before = std::time::Instant::now();
        let loaded = load(&conn, "historical").unwrap().unwrap();
        println!("oversized_legacy_metadata migrated_us={migrate_us} load_us={} encoded_header={} decoded_parser_id_bytes={}",before.elapsed().as_micros(),conn.query_row("SELECT length(payload) FROM claude_link_checkpoint",[],|r|r.get::<_,i64>(0)).unwrap(),loaded.parser.pending_ids().map(str::len).sum::<usize>());
        assert_eq!(loaded, cp);
    }

    fn legacy_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::historical_schema(&conn, 46);
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
            // Historical v47 comparison, not the compact production codec.
            let conn = Connection::open(&db).unwrap();
            crate::store::historical_schema(&conn, 47);
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0")
                .unwrap();
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
