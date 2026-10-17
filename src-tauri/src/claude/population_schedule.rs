//! Give each processing lane an early turn without extending the enclosing pass.
use super::transcript::PopulationControl;
use crate::store::{settings, StoreError};
use rusqlite::Connection;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lane {
    Links,
    Metadata,
    Usage,
    Index,
}
impl Lane {
    pub(crate) fn control(self, root: &PopulationControl) -> PopulationControl {
        root.with_slice_budget(Duration::from_secs(match self {
            Self::Links | Self::Metadata => 10,
            Self::Usage => 15,
            Self::Index => 25,
        }))
    }
}

pub(crate) fn processing_order(conn: &mut Connection) -> Result<[Lane; 4], StoreError> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let first = settings::get_strict::<usize>(&tx, settings::keys::CLAUDE_POPULATION_NEXT_LANE)?
        .unwrap_or(0)
        % 4;
    let mut lanes = [Lane::Links, Lane::Metadata, Lane::Usage, Lane::Index];
    lanes.rotate_left(first);
    settings::set(
        &tx,
        settings::keys::CLAUDE_POPULATION_NEXT_LANE,
        &((first + 1) % 4),
    )?;
    tx.commit()?;
    Ok(lanes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_passes_rotate_first_lane_across_database_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schedule.db");
        let expected = [
            Lane::Links,
            Lane::Metadata,
            Lane::Usage,
            Lane::Index,
            Lane::Links,
        ];
        for first in expected {
            let mut conn = crate::store::open_db(&path).unwrap();
            let order = processing_order(&mut conn).unwrap();
            assert_eq!(order[0], first);
            // Simulate a pass stopping after its first lane. The next admission
            // must still rotate even if no lane produced a successful result.
            assert_eq!(order.iter().filter(|&&lane| lane == Lane::Index).count(), 1);
        }
    }
    #[test]
    fn lane_budget_cannot_revive_an_expired_parent_and_cancel_is_shared() {
        let root = PopulationControl::discovery();
        let expired = root.with_budget(Duration::ZERO);
        assert!(Lane::Index.control(&expired).stopped());
        let usage = Lane::Usage.control(&root);
        let index = Lane::Index.control(&root);
        usage.cancel();
        assert!(index.stopped());
        assert!(root.stopped());
    }
}
