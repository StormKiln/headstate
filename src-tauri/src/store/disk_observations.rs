//! One bounded history for disk accounting and artifact growth.
use crate::disk_inventory::model::Observation;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

pub const MIGRATION_SQL: &str = "CREATE TABLE disk_observations (id TEXT PRIMARY KEY, scope_id TEXT NOT NULL, observed_at INTEGER NOT NULL, complete INTEGER NOT NULL, payload TEXT NOT NULL); CREATE INDEX disk_observations_scope ON disk_observations(scope_id, observed_at);";
const PAYLOAD_LIMIT: usize = 8 * 1024 * 1024;
pub fn save(conn: &mut Connection, observation: &Observation) -> Result<(), String> {
    let payload = serde_json::to_string(observation).map_err(|e| e.to_string())?;
    if payload.len() > PAYLOAD_LIMIT {
        return Err("This observation exceeds the history size limit; it was not saved.".into());
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    tx.execute("INSERT OR REPLACE INTO disk_observations(id,scope_id,observed_at,complete,payload) VALUES (?1,?2,?3,?4,?5)", params![observation.id, observation.volume.id, observation.finished_at, observation.status == "complete", payload]).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM disk_observations WHERE scope_id=?1 AND id NOT IN (SELECT id FROM disk_observations WHERE scope_id=?1 ORDER BY observed_at DESC,id DESC LIMIT 31) AND id NOT IN (SELECT id FROM disk_observations WHERE scope_id=?1 AND complete=1 ORDER BY observed_at DESC,id DESC LIMIT 1)", [&observation.volume.id]).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM disk_observations WHERE id NOT IN (SELECT id FROM disk_observations ORDER BY observed_at DESC,id DESC LIMIT 256)", []).map_err(|e| e.to_string())?;
    // Bound both row count and serialized bytes, including old large observations.
    // If the global bound retires a baseline, the next comparison reports no baseline.
    tx.execute("DELETE FROM disk_observations WHERE id IN (SELECT id FROM (SELECT id, SUM(length(CAST(payload AS BLOB))) OVER (ORDER BY observed_at DESC,id DESC) AS cumulative_bytes FROM disk_observations) WHERE cumulative_bytes > 16777216)", []).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
pub fn latest(conn: &Connection, scope: &str) -> Result<Option<Observation>, String> {
    let raw: Option<String> = conn.query_row("SELECT payload FROM disk_observations WHERE scope_id=?1 AND complete=1 ORDER BY observed_at DESC,id DESC LIMIT 1", [scope], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    raw.map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
        .transpose()
}
pub fn history(conn: &Connection) -> Result<Vec<Observation>, String> {
    let mut statement = conn
        .prepare(
            "SELECT payload FROM disk_observations ORDER BY observed_at DESC,id DESC LIMIT 256",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .map(|r| {
            r.map_err(|e| e.to_string())
                .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
        })
        .collect();
    rows
}
