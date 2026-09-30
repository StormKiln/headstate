//! Short-lived GitLab receipts. Their JSON key includes provider, host, stable
//! viewer id, scope and window. History is evidence from this fetch, not an
//! invented accumulated total. Existing GitHub history/cache remains untouched.
use crate::gitlab::stats::{HistoryReceipt, Report};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use std::path::Path;

pub fn get(path: &Path, key: &str) -> Result<Option<Report>, String> {
    let conn = super::open_db(path).map_err(|e| e.to_string())?;
    let raw: Option<String> = conn.query_row(
        "SELECT payload FROM gitlab_stats_cache WHERE key = ?1 AND fetched_at > datetime('now', '-5 minutes')",
        [key], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    // Unreadable receipts are cache misses, never zero measurements.
    Ok(raw.and_then(|v| serde_json::from_str(&v).ok()))
}
pub fn put(path: &Path, key: &str, report: &Report) -> Result<(), String> {
    let conn = super::open_db(path).map_err(|e| e.to_string())?;
    conn.execute("INSERT INTO gitlab_stats_cache (key, payload, fetched_at) VALUES (?1, ?2, datetime('now')) ON CONFLICT(key) DO UPDATE SET payload = excluded.payload, fetched_at = excluded.fetched_at",
        params![key, serde_json::to_string(report).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM gitlab_stats_cache WHERE fetched_at < datetime('now', '-7 days')",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Invalidate live summaries only; retained historical evidence is immutable.
pub fn invalidate_host(path: &Path, host: &str) -> Result<(), String> {
    let conn = super::open_db(path).map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM gitlab_stats_cache WHERE json_extract(payload, '$.source.host') = ?1",
        [host],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn history_get(
    path: &Path,
    partition: &str,
    day: &str,
) -> Result<Option<HistoryReceipt>, String> {
    let conn = super::open_db(path).map_err(|e| e.to_string())?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT payload FROM gitlab_stats_history WHERE partition = ?1 AND day = ?2",
            params![partition, day],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(raw.and_then(|v| serde_json::from_str(&v).ok()))
}

pub fn history_put(
    path: &Path,
    partition: &str,
    day: &str,
    receipt: &HistoryReceipt,
) -> Result<HistoryReceipt, String> {
    let mut conn = super::open_db(path).map_err(|e| e.to_string())?;
    // Read and merge only after taking the write reservation. Another desktop
    // or phone request may have completed the day while this fetch was running.
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let raw: Option<String> = tx
        .query_row(
            "SELECT payload FROM gitlab_stats_history WHERE partition = ?1 AND day = ?2",
            params![partition, day],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let mut saved = receipt.clone();
    if let Some(prior) = raw.and_then(|value| serde_json::from_str::<HistoryReceipt>(&value).ok()) {
        saved.retain_prior(&prior);
    }
    tx.execute("INSERT INTO gitlab_stats_history (partition, day, payload) VALUES (?1, ?2, ?3) ON CONFLICT(partition, day) DO UPDATE SET payload = excluded.payload", params![partition, day, serde_json::to_string(&saved).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(saved)
}
