//! Durable scope CAS, independent of account generation and cursor lifetime.
use rusqlite::{Connection, OptionalExtension, Transaction};

pub fn reserve(tx: &Transaction<'_>, key: &str) -> Result<i64, String> {
    tx.execute("INSERT INTO pr_scope_evidence(scope_key,revision) VALUES (?1,1) ON CONFLICT(scope_key) DO UPDATE SET revision=revision+1", [key]).map_err(|e|e.to_string())?;
    current(tx, key)
}

pub fn current(conn: &Connection, key: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT revision FROM pr_scope_evidence WHERE scope_key=?1",
        [key],
        |r| r.get(0),
    )
    .optional()
    .map(|n| n.unwrap_or(0))
    .map_err(|e| e.to_string())
}

pub fn require(tx: &Transaction<'_>, key: &str, revision: i64) -> Result<(), String> {
    if revision > 0 && current(tx, key)? == revision {
        Ok(())
    } else {
        Err("history evidence changed during this load; retry".into())
    }
}
