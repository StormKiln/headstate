//! One bounded history for disk accounting and artifact growth.
use crate::disk_inventory::model::Observation;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

pub const MIGRATION_SQL: &str = "CREATE TABLE disk_observations (id TEXT PRIMARY KEY, scope_id TEXT NOT NULL, observed_at INTEGER NOT NULL, complete INTEGER NOT NULL, payload TEXT NOT NULL); CREATE INDEX disk_observations_scope ON disk_observations(scope_id, observed_at);";
const PAYLOAD_LIMIT: usize = 8 * 1024 * 1024;

/// The stored payload's format version (#1963).
///
/// Rows written before this existed carry no version and read as 0,
/// which is the same shape as 1. A row from a newer build (a higher
/// version) is counted as unreadable rather than half-decoded.
const PAYLOAD_VERSION: u32 = 1;

#[derive(serde::Serialize)]
struct StoredRef<'a> {
    schema_version: u32,
    #[serde(flatten)]
    observation: &'a Observation,
}

#[derive(serde::Deserialize)]
struct Stored {
    #[serde(default)]
    schema_version: u32,
    #[serde(flatten)]
    observation: Observation,
}

fn decode(payload: &str) -> Result<Observation, String> {
    let stored: Stored = serde_json::from_str(payload).map_err(|e| e.to_string())?;
    if stored.schema_version > PAYLOAD_VERSION {
        return Err(format!(
            "saved by a newer version (format {}, this build reads {PAYLOAD_VERSION})",
            stored.schema_version
        ));
    }
    Ok(stored.observation)
}

/// The saved history, with how many rows could not be read (#1963).
///
/// One undecodable row used to fail the whole history. Now it is left
/// out and COUNTED, so the reader can say the history is incomplete
/// instead of presenting what remains as all there is.
#[derive(Debug, Clone, serde::Serialize)]
pub struct History {
    pub observations: Vec<Observation>,
    pub unreadable: u64,
}

pub fn save(conn: &mut Connection, observation: &Observation) -> Result<(), String> {
    let payload = serde_json::to_string(&StoredRef {
        schema_version: PAYLOAD_VERSION,
        observation,
    })
    .map_err(|e| e.to_string())?;
    if payload.len() > PAYLOAD_LIMIT {
        return Err("This observation exceeds the history size limit; it was not saved.".into());
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    tx.execute("INSERT OR REPLACE INTO disk_observations(id,scope_id,observed_at,complete,payload) VALUES (?1,?2,?3,?4,?5)", params![observation.id, observation.volume.id, observation.finished_at, observation.status == "complete", payload]).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM disk_observations WHERE scope_id=?1 AND id NOT IN (SELECT id FROM disk_observations WHERE scope_id=?1 ORDER BY observed_at DESC,id DESC LIMIT 31) AND id NOT IN (SELECT id FROM disk_observations WHERE scope_id=?1 AND complete=1 ORDER BY observed_at DESC,id DESC LIMIT 1)", [&observation.volume.id]).map_err(|e| e.to_string())?;
    // Reserve each scope's newest complete baseline before spending global
    // capacity on partial or older observations. Only competing complete
    // baselines can retire an old baseline when their own budget is exceeded.
    let candidates = {
        let mut statement = tx.prepare("SELECT id, length(CAST(payload AS BLOB)) FROM (SELECT id,payload,observed_at,complete,ROW_NUMBER() OVER (PARTITION BY scope_id ORDER BY complete DESC,observed_at DESC,id DESC) AS scope_rank FROM disk_observations) ORDER BY CASE WHEN complete=1 AND scope_rank=1 THEN 0 ELSE 1 END,observed_at DESC,id DESC").map_err(|e|e.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    let (mut count, mut bytes, mut current_saved) = (0usize, 0i64, false);
    for (id, size) in candidates {
        if count < 256
            && bytes
                .checked_add(size)
                .is_some_and(|total| total <= 16 * 1024 * 1024)
        {
            count += 1;
            bytes += size;
            current_saved |= id == observation.id;
        } else {
            tx.execute("DELETE FROM disk_observations WHERE id=?1", [id])
                .map_err(|e| e.to_string())?;
        }
    }
    if !current_saved {
        // Roll back, leaving prior history intact; the controller still publishes
        // the current useful measurement with this explicit persistence refusal.
        return Err("History capacity is reserved for complete baselines; the current measurement was not saved.".into());
    }
    tx.commit().map_err(|e| e.to_string())
}
pub fn latest(conn: &Connection, scope: &str) -> Result<Option<Observation>, String> {
    let raw: Option<String> = conn.query_row("SELECT payload FROM disk_observations WHERE scope_id=?1 AND complete=1 ORDER BY observed_at DESC,id DESC LIMIT 1", [scope], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    raw.map(|s| decode(&s)).transpose()
}
/// A failed query is still an error: only rows that were read and could
/// not be decoded are skipped and counted.
pub fn history(conn: &Connection) -> Result<History, String> {
    let mut statement = conn
        .prepare(
            "SELECT payload FROM disk_observations ORDER BY observed_at DESC,id DESC LIMIT 256",
        )
        .map_err(|e| e.to_string())?;
    let payloads = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut history = History {
        observations: Vec::with_capacity(payloads.len()),
        unreadable: 0,
    };
    for payload in payloads {
        match decode(&payload) {
            Ok(observation) => history.observations.push(observation),
            Err(e) => {
                log::warn!("skipping a saved disk observation that could not be read: {e}");
                history.unreadable += 1;
            }
        }
    }
    Ok(history)
}
