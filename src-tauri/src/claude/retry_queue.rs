//! Durable round-robin selection over pending identities (#1863).
//! Attempts advance separately from body transactions: a rejected body must not
//! pin every later pass to the same failed prefix. No per-file retry rows.
use rusqlite::{Connection, OptionalExtension};

fn key(lane: &str) -> &str {
    match lane {
        "usage" => "claude_usage_attempt_cursor",
        "index" => "claude_index_attempt_cursor",
        "metadata" => "claude_metadata_attempt_cursor",
        _ => unreachable!("unsupported corpus retry lane"),
    }
}

pub(crate) fn rotate_pending<T>(
    conn: &Connection,
    lane: &str,
    todo: &mut [T],
    id: impl Fn(&T) -> &str,
) -> rusqlite::Result<()> {
    todo.sort_by(|a, b| id(a).cmp(id(b)));
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            [key(lane)],
            |r| r.get(0),
        )
        .optional()?;
    let anchor = raw.and_then(|v| serde_json::from_str::<String>(&v).ok());
    if let Some(anchor) = anchor {
        let first = todo.partition_point(|item| id(item) <= anchor.as_str());
        if first < todo.len() {
            todo.rotate_left(first);
        }
    }
    Ok(())
}

pub(crate) fn checkpoint_attempt(
    conn: &Connection,
    lane: &str,
    generation: i64,
    id: &str,
) -> rusqlite::Result<bool> {
    let value = serde_json::to_string(id)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    Ok(conn.execute(
        "INSERT INTO settings(key,value) SELECT ?1,?2 WHERE EXISTS(SELECT 1 FROM claude_population_generation WHERE lane=?3 AND generation=?4) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key(lane),value,lane,generation],
    )? == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removed_anchor_wraps_and_old_generation_cannot_rewind() {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let old = super::super::transcript::population_generation(&conn, "index").unwrap();
        assert!(checkpoint_attempt(&conn, "index", old, "b").unwrap());
        let mut todo = vec!["d", "a", "c"];
        rotate_pending(&conn, "index", &mut todo, |s| *s).unwrap();
        assert_eq!(todo, vec!["c", "d", "a"]);
        let new = super::super::transcript::population_generation(&conn, "index").unwrap();
        assert!(checkpoint_attempt(&conn, "index", new, "z").unwrap());
        assert!(!checkpoint_attempt(&conn, "index", old, "a").unwrap());
        rotate_pending(&conn, "index", &mut todo, |s| *s).unwrap();
        assert_eq!(todo, vec!["a", "c", "d"]);
    }
}
