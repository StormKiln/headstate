//! One durable interval of Stats ownership. Authentication comes from the
//! immutable GitHub client; this token only fences shared profile storage.
use super::{settings, StoreError};
use rusqlite::{Connection, Transaction, TransactionBehavior};

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsOwner {
    viewer: String,
    generation: i64,
}
impl StatsOwner {
    pub fn viewer(&self) -> &str {
        &self.viewer
    }
    pub fn generation(&self) -> i64 {
        self.generation
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OwnedError {
    #[error("the stats account changed during this load; retry to use the current account")]
    Superseded,
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("stats ownership generation exhausted")]
    Exhausted,
    #[error("stats storage task failed: {0}")]
    Task(String),
}
impl From<rusqlite::Error> for OwnedError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

fn read(conn: &Connection) -> Result<(Option<String>, Option<i64>), OwnedError> {
    // One statement: viewer and generation cannot come from different commits.
    let (viewer, generation): (Option<String>, Option<String>) = conn.query_row(
        "SELECT (SELECT value FROM settings WHERE key=?1), (SELECT value FROM settings WHERE key=?2)",
        [settings::keys::STATS_VIEWER, settings::keys::STATS_GENERATION],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok((
        viewer.and_then(|s| serde_json::from_str(&s).ok()),
        generation.and_then(|s| serde_json::from_str(&s).ok()),
    ))
}

/// Read-only, including while another WAL connection owns the writer lock.
/// Background work must use this, never activate ownership.
pub fn current_for_verified(
    conn: &Connection,
    viewer: &str,
) -> Result<Option<StatsOwner>, OwnedError> {
    let (current, generation) = read(conn)?;
    Ok(match (current, generation) {
        (Some(current), Some(generation)) if current == viewer && generation > 0 => {
            Some(StatsOwner {
                viewer: current,
                generation,
            })
        }
        _ => None,
    })
}

/// Called once at foreground entry. Same-owner legacy history survives its
/// one-time metadata initialization. All account retirement is atomic.
pub fn capture_verified(conn: &Connection, viewer: &str) -> Result<StatsOwner, OwnedError> {
    if let Some(owner) = current_for_verified(conn, viewer)? {
        return Ok(owner);
    }
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    if let Some(owner) = current_for_verified(&tx, viewer)? {
        return Ok(owner);
    }
    let (previous, generation) = read(&tx)?;
    let generation = generation
        .unwrap_or(0)
        .checked_add(1)
        .filter(|n| *n > 0)
        .ok_or(OwnedError::Exhausted)?;
    if previous.as_deref() != Some(viewer) {
        super::stats::clear(&tx)?;
        super::pr_history::clear(&tx)?;
        super::pr_slice::clear(&tx)?;
        super::pr_backfill_scope::clear(&tx)?;
        super::pr_backfill_page::clear(&tx)?;
    }
    settings::set(&tx, settings::keys::STATS_VIEWER, &viewer)?;
    settings::set(&tx, settings::keys::STATS_GENERATION, &generation)?;
    tx.commit()?;
    Ok(StatsOwner {
        viewer: viewer.to_owned(),
        generation,
    })
}

/// Check inside the same transaction as the actual read or mutation.
pub fn require_current(tx: &Transaction<'_>, expected: &StatsOwner) -> Result<(), OwnedError> {
    if current_for_verified(tx, expected.viewer())?.as_ref() == Some(expected) {
        Ok(())
    } else {
        Err(OwnedError::Superseded)
    }
}
