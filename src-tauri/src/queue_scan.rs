//! Durable traversal evidence, separate from rich inventory observations.
use crate::{
    identity::{PrIdentity, Source},
    store::{CachedList, StoreError},
};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const VERSION: u32 = 1;
pub const MEMBERS: usize = 10000;
pub const CANDIDATES: usize = 1000;
pub const CONFIRM_DELAY: i64 = 60;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub identity: PrIdentity,
    pub id: String,
    pub head: String,
    pub created_at: DateTime<Utc>,
    pub negative_at: Option<i64>,
    pub eligible_at: i64,
    pub failures: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub coverage_valid: bool,
    #[serde(default = "default_pass_delay")]
    pub pass_delay: i64,
    /// Last validated traversal, not an atomic membership snapshot.
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub completed_total: Option<u64>,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub finished_at: Option<i64>,
    #[serde(default)]
    pub no_work: bool,
    #[serde(default)]
    pub step_failure: Option<ScanFailure>,
    #[serde(default)]
    pub received: bool,
    pub version: u32,
    #[serde(default)]
    pub receipt_id: Option<String>,
    pub after: Option<String>,
    pub cursors: Vec<String>,
    pub seen: Vec<PrIdentity>,
    pub total: Option<u64>,
    pub count_seen: bool,
    pub tainted: bool,
    pub done: bool,
    pub ceiling: bool,
    pub pages: u32,
    pub eligible_at: i64,
    pub failures: u32,
    pub candidates: VecDeque<Candidate>,
    pub isolate: bool,
}
fn default_pass_delay() -> i64 {
    CONFIRM_DELAY
}

impl Default for State {
    fn default() -> Self {
        Self {
            coverage_valid: false,
            pass_delay: default_pass_delay(),
            completed_at: None,
            completed_total: None,
            started_at: None,
            finished_at: None,
            no_work: false,
            step_failure: None,
            received: false,
            version: VERSION,
            receipt_id: None,
            after: None,
            cursors: vec![],
            seen: vec![],
            total: None,
            count_seen: false,
            tainted: false,
            done: false,
            ceiling: false,
            pages: 0,
            eligible_at: 0,
            failures: 0,
            candidates: VecDeque::new(),
            isolate: false,
        }
    }
}
impl State {
    pub fn fresh_pass(&mut self) {
        let candidates = std::mem::take(&mut self.candidates);
        let isolate = self.isolate;
        *self = Self {
            candidates,
            isolate,
            coverage_valid: self.coverage_valid,
            pass_delay: self.pass_delay,
            completed_at: self.completed_at,
            completed_total: self.completed_total,
            ..Self::default()
        };
    }
    pub fn taint_effect(&mut self) {
        self.receipt_id = None;
        self.tainted = true;
        self.coverage_valid = false;
        for c in &mut self.candidates {
            c.negative_at = None;
        }
    }
    pub fn failure(&mut self, now: i64) {
        self.tainted = true;
        self.coverage_valid = false;
        self.failures = self.failures.saturating_add(1);
        self.eligible_at = now + backoff(self.failures);
    }
    pub fn observe_count(&mut self, total: Option<u64>) {
        if self.completed_at.is_some() && self.completed_total != total {
            self.coverage_valid = false;
        }
        if !self.count_seen {
            self.total = total;
            self.count_seen = true;
        } else if self.total != total {
            self.total = None;
            self.tainted = true;
        }
        if total.is_none() {
            self.tainted = true;
        }
    }
}
pub fn backoff(failures: u32) -> i64 {
    (30i64 * (1i64 << failures.min(5))).min(900)
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanFailure {
    pub message: String,
    pub transient: bool,
    pub not_asked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub expected_revision: i64,
    pub state: State,
    pub removals: Vec<PrIdentity>,
}
#[derive(Debug)]
pub struct Loaded {
    pub revision: i64,
    pub state: State,
}

pub fn load(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    owner: &str,
) -> Result<Loaded, StoreError> {
    let provider = serde_json::to_value(source.provider)?;
    let row: Option<(i64, String, String)> = conn.query_row(
        "SELECT revision, owner, payload FROM queue_scan WHERE provider=?1 AND host=?2 AND list=?3",
        params![provider.as_str(), source.host, list.id()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
    let Some((revision, stored_owner, payload)) = row else {
        return Ok(Loaded {
            revision: 0,
            state: State::default(),
        });
    };
    let state = if stored_owner == owner {
        serde_json::from_str::<State>(&payload)
            .ok()
            .filter(|state| {
                state.version == VERSION
                    && state.seen.len() <= MEMBERS
                    && state.candidates.len() <= CANDIDATES
                    && state.cursors.len() <= MEMBERS
            })
            .unwrap_or_default()
    } else {
        State::default()
    };
    Ok(Loaded { revision, state })
}

pub fn new_receipt_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}
/// An overlapping caller may reuse only this exact accepted operation. Any
/// newer scan, owner change or confirmed action advances the revision.
pub fn accepted(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    owner: &str,
    next: &Commit,
) -> Result<bool, StoreError> {
    let loaded = load(conn, source, list, owner)?;
    Ok(next.state.receipt_id.is_some()
        && loaded.revision == next.expected_revision + 1
        && loaded.state == next.state)
}

/// CAS and rows are one transaction. The caller still holds its publication gate.
pub fn commit(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    owner: &str,
    next: &Commit,
    save_rows: impl FnOnce(&Connection) -> Result<(), StoreError>,
) -> Result<bool, StoreError> {
    let tx = conn.unchecked_transaction()?;
    let provider = serde_json::to_value(source.provider)?;
    let revision: i64 = tx
        .query_row(
            "SELECT revision FROM queue_scan WHERE provider=?1 AND host=?2 AND list=?3",
            params![provider.as_str(), source.host, list.id()],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    if revision != next.expected_revision {
        return Ok(false);
    }
    tx.execute("INSERT INTO queue_scan(provider,host,list,owner,revision,payload) VALUES(?1,?2,?3,?4,?5,?6)
        ON CONFLICT(provider,host,list) DO UPDATE SET owner=excluded.owner,revision=excluded.revision,payload=excluded.payload",
        params![provider.as_str(),source.host,list.id(),owner,revision+1,serde_json::to_string(&next.state)?])?;
    save_rows(&tx)?;
    tx.commit()?;
    Ok(true)
}
/// Called inside the confirmed-effect snapshot transaction; preserve the tail.
pub fn taint(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    owner: &str,
) -> Result<(), StoreError> {
    let loaded = load(conn, source, list, owner)?;
    if loaded.revision == 0 {
        return Ok(());
    }
    let mut state = loaded.state;
    state.taint_effect();
    let provider = serde_json::to_value(source.provider)?;
    conn.execute("UPDATE queue_scan SET payload=?1,revision=revision+1 WHERE provider=?2 AND host=?3 AND list=?4 AND owner=?5",
        params![serde_json::to_string(&state)?,provider.as_str(),source.host,list.id(),owner])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::source_cache::{save_owned_source_snapshot, snapshot_owner, Coverage};
    #[test]
    fn new_pass_keeps_historical_coverage_but_changed_count_invalidates_it() {
        for count in [74, 76] {
            let mut state = State {
                done: true,
                completed_at: Some(1000),
                completed_total: Some(75),
                coverage_valid: true,
                ..State::default()
            };
            state.fresh_pass();
            assert!(state.coverage_valid);
            state.observe_count(Some(count));
            assert!(!state.coverage_valid);
            assert_eq!(state.completed_at, Some(1000));
            assert_eq!(state.total, Some(count));
        }
    }

    #[test]
    fn rows_checkpoint_and_owner_commit_or_rollback_together_and_cas_rejects_old_steps() {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let source = Source::default();
        let list = CachedList::Reviewing;
        let state = State {
            after: Some("opaque-tail".into()),
            ..State::default()
        };
        let next = Commit {
            expected_revision: 0,
            state,
            removals: vec![],
        };
        conn.execute_batch("CREATE TRIGGER reject_snapshot BEFORE INSERT ON snapshot BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
        assert!(commit(&conn, &source, list, "fixture", &next, |tx| {
            save_owned_source_snapshot(tx, &source, list, &[], &Coverage::Unknown, Some("fixture"))
        })
        .is_err());
        assert_eq!(load(&conn, &source, list, "fixture").unwrap().revision, 0);
        assert_eq!(snapshot_owner(&conn, &source, list).unwrap(), None);
        conn.execute_batch("DROP TRIGGER reject_snapshot;").unwrap();
        assert!(commit(&conn, &source, list, "fixture", &next, |tx| {
            save_owned_source_snapshot(tx, &source, list, &[], &Coverage::Unknown, Some("fixture"))
        })
        .unwrap());
        assert_eq!(
            snapshot_owner(&conn, &source, list).unwrap().as_deref(),
            Some("fixture")
        );
        assert!(!commit(&conn, &source, list, "fixture", &next, |_| panic!(
            "stale rows must not save"
        ))
        .unwrap());
        let tx = conn.unchecked_transaction().unwrap();
        taint(&tx, &source, list, "fixture").unwrap();
        tx.commit().unwrap();
        let loaded = load(&conn, &source, list, "fixture").unwrap();
        assert_eq!(loaded.revision, 2);
        assert_eq!(loaded.state.after.as_deref(), Some("opaque-tail"));
        assert!(loaded.state.tainted);
        let other = load(&conn, &source, list, "different").unwrap();
        assert_eq!(other.revision, 2);
        assert!(other.state.after.is_none());
    }
    #[test]
    fn restart_preserves_only_matching_owned_query_progress_and_recovers_bad_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.sqlite");
        let source = Source::default();
        let list = CachedList::Authored;
        {
            let conn = crate::store::open_db(&path).unwrap();
            let next = Commit {
                expected_revision: 0,
                state: State {
                    receipt_id: Some(new_receipt_id()),
                    after: Some("opaque-restart".into()),
                    ..State::default()
                },
                removals: vec![],
            };
            assert!(commit(&conn, &source, list, "fixture", &next, |tx| {
                save_owned_source_snapshot(
                    tx,
                    &source,
                    list,
                    &[],
                    &Coverage::Unknown,
                    Some("fixture"),
                )
            })
            .unwrap());
        }
        let conn = crate::store::open_db(&path).unwrap();
        assert_eq!(
            load(&conn, &source, list, "fixture")
                .unwrap()
                .state
                .after
                .as_deref(),
            Some("opaque-restart")
        );
        assert!(load(&conn, &source, list, "other")
            .unwrap()
            .state
            .after
            .is_none());
        assert_eq!(
            load(&conn, &source, CachedList::Reviewing, "fixture")
                .unwrap()
                .revision,
            0
        );
        let mut different = source.clone();
        different.host = "other.example".into();
        assert_eq!(
            load(&conn, &different, list, "fixture").unwrap().revision,
            0
        );
        let mut state = load(&conn, &source, list, "fixture").unwrap().state;
        state.version += 1;
        conn.execute(
            "UPDATE queue_scan SET payload=?1",
            [serde_json::to_string(&state).unwrap()],
        )
        .unwrap();
        assert!(load(&conn, &source, list, "fixture")
            .unwrap()
            .state
            .after
            .is_none());
        conn.execute("UPDATE queue_scan SET payload='broken'", [])
            .unwrap();
        let recovered = load(&conn, &source, list, "fixture").unwrap();
        assert_eq!(recovered.revision, 1);
        assert!(recovered.state.after.is_none());
    }
}
