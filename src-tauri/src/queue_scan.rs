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
pub const PARTITION_WINDOWS: usize = 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub lo: i64,
    pub hi: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leaf {
    pub window: Window,
    pub after: Option<String>,
    pub cursors: Vec<String>,
    pub seen: Vec<PrIdentity>,
    pub total: Option<u64>,
    pub pages: u32,
}
impl Leaf {
    pub fn new(window: Window) -> Self {
        Self {
            window,
            after: None,
            cursors: vec![],
            seen: vec![],
            total: None,
            pages: 0,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartitionPhase {
    LowerBound,
    UpperBound,
    Leaves,
    FinalCheck,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockReason {
    TimestampResolution,
    WindowLimit,
    MembershipLimit,
    InvalidMetadata,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubPartition {
    pub version: u32,
    pub phase: PartitionPhase,
    pub lower: Option<i64>,
    pub upper: Option<i64>,
    pub pending: VecDeque<Window>,
    pub active: Option<Leaf>,
    /// Includes retired parents, so splitting cannot create unbounded work.
    pub windows_started: usize,
    pub blocked: Vec<(Window, BlockReason)>,
}
impl Default for GithubPartition {
    fn default() -> Self {
        Self {
            version: 1,
            phase: PartitionPhase::LowerBound,
            lower: None,
            upper: None,
            pending: VecDeque::new(),
            active: None,
            windows_started: 0,
            blocked: vec![],
        }
    }
}
impl GithubPartition {
    fn valid(&self) -> bool {
        let time = |t| DateTime::from_timestamp(t, 0).is_some();
        let window = |w: &Window| {
            w.lo <= w.hi
                && time(w.lo)
                && time(w.hi)
                && self.lower.is_some_and(|lo| w.lo >= lo)
                && self.upper.is_some_and(|hi| w.hi <= hi)
        };
        let cursor = |c: &String| !c.is_empty() && c.len() <= 4096;
        self.version == 1
            && self.windows_started <= PARTITION_WINDOWS
            && self.pending.len() + self.blocked.len() + usize::from(self.active.is_some())
                <= self.windows_started
            && self.lower.is_none_or(time)
            && self.upper.is_none_or(time)
            && self.lower.zip(self.upper).is_none_or(|(lo, hi)| lo <= hi)
            && self.pending.iter().all(window)
            && self.blocked.iter().all(|(w, _)| window(w))
            && self.active.as_ref().is_none_or(|l| {
                window(&l.window)
                    && l.pages < 40
                    && l.seen.len() <= 1000
                    && l.cursors.len() == l.pages as usize
                    && l.after.as_ref().is_none_or(cursor)
                    && l.cursors.iter().all(cursor)
                    && l.cursors
                        .iter()
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        == l.cursors.len()
                    && l.seen
                        .iter()
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        == l.seen.len()
                    && l.after.as_ref() == l.cursors.last()
            })
            && match self.phase {
                PartitionPhase::LowerBound => {
                    self.lower.is_none() && self.upper.is_none() && self.windows_started == 0
                }
                PartitionPhase::UpperBound => {
                    self.lower.is_some() && self.upper.is_none() && self.windows_started == 0
                }
                PartitionPhase::Leaves => {
                    self.lower.is_some()
                        && self.upper.is_some()
                        && (self.active.is_some() || !self.pending.is_empty())
                }
                PartitionPhase::FinalCheck => {
                    self.lower.is_some()
                        && self.upper.is_some()
                        && self.active.is_none()
                        && self.pending.is_empty()
                        && self.windows_started > 0
                }
            }
    }
}
fn deserialize_partition<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<GithubPartition>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(d)?;
    Ok(value.map(|v| {
        serde_json::from_value(v).unwrap_or_else(|_| GithubPartition {
            version: 0,
            ..Default::default()
        })
    }))
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// An inconclusive direct read is retried alone until structurally healthy.
    #[serde(default)]
    pub isolated: bool,
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
    #[serde(default, deserialize_with = "deserialize_partition")]
    pub github_partition: Option<GithubPartition>,
    /// Next inventory position to consider for bounded proof admission.
    #[serde(default)]
    pub candidate_position: usize,
    #[serde(default)]
    pub coverage_valid: bool,
    /// One bounded repair traversal after a local authoritative effect.
    #[serde(default)]
    pub local_repair_pending: bool,
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
            github_partition: None,
            candidate_position: 0,
            coverage_valid: false,
            local_repair_pending: false,
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
    /// A local traversal bound is a partial answer, not a failed provider read.
    pub fn partition_reason(&self) -> Option<String> {
        let partition = self.github_partition.as_ref()?;
        let reason = match partition.blocked.first().map(|(_, reason)| reason) {
            Some(BlockReason::TimestampResolution) => "More than 1,000 matching pull requests share a timestamp window; saved rows are retained.",
            Some(BlockReason::WindowLimit) => "The queue reached the bounded creation-window scan limit; saved rows are retained.",
            Some(BlockReason::MembershipLimit) => "A creation window reached the bounded search-page limit; saved rows are retained.",
            Some(BlockReason::InvalidMetadata) => "A creation window could not be validated; saved rows are retained and the next pass will retry.",
            None if self.ceiling => "The queue reached the 10,000-identity traversal proof limit; saved rows are retained.",
            None if self.failures > 0 => "The queue creation bounds could not be validated; saved rows are retained and traversal will retry.",
            None => return None,
        };
        Some(reason.into())
    }
    pub fn fresh_pass(&mut self) {
        let candidates = std::mem::take(&mut self.candidates);
        *self = Self {
            github_partition: self
                .github_partition
                .as_ref()
                .map(|_| GithubPartition::default()),
            candidate_position: self.candidate_position,
            candidates,
            coverage_valid: self.coverage_valid,
            pass_delay: self.pass_delay,
            completed_at: self.completed_at,
            completed_total: self.completed_total,
            ..Self::default()
        };
    }
    pub fn taint_effect(&mut self) {
        self.invalidate_proof();
        self.local_repair_pending = true;
        if self.local_repair_ready() {
            if let Some(finished) = self.finished_at {
                self.eligible_at = finished + self.completed_pass_delay();
            }
        }
    }
    fn invalidate_proof(&mut self) {
        self.receipt_id = None;
        self.tainted = true;
        self.coverage_valid = false;
        for c in &mut self.candidates {
            c.negative_at = None;
        }
    }
    pub(crate) fn local_repair_ready(&self) -> bool {
        self.local_repair_pending
            && self.done
            && !self.coverage_valid
            && !self.ceiling
            && self.failures == 0
            && self.step_failure.is_none()
            && self
                .github_partition
                .as_ref()
                .is_none_or(|p| p.blocked.is_empty())
    }
    pub fn local_repair_due(&self, now: i64) -> bool {
        self.local_repair_ready() && now >= self.eligible_at
    }
    pub fn completed_pass_delay(&self) -> i64 {
        if self.local_repair_ready() {
            15
        } else {
            self.pass_delay.max(CONFIRM_DELAY)
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
    let mut state = if stored_owner == owner {
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
    // The legacy global latch records a batch failure, not per-identity evidence.
    // Preserve retry/proof state and let one bounded batch establish health.
    state.isolate = false;
    if state.github_partition.as_ref().is_some_and(|p| !p.valid()) {
        state.fresh_pass();
        // Corrupt metadata is not a local action and earns no expedited repair.
        state.invalidate_proof();
    }
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
    #[cfg(feature = "enterprise-harness")]
    let mut metric = crate::enterprise_harness::metrics::Scope::new("queue-transaction", 0);
    let tx = conn.unchecked_transaction()?;
    #[cfg(feature = "enterprise-harness")]
    metric.mark("acquired", 0);
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
        #[cfg(feature = "enterprise-harness")]
        metric.finish("cas-rejected");
        return Ok(false);
    }
    tx.execute("INSERT INTO queue_scan(provider,host,list,owner,revision,payload) VALUES(?1,?2,?3,?4,?5,?6)
        ON CONFLICT(provider,host,list) DO UPDATE SET owner=excluded.owner,revision=excluded.revision,payload=excluded.payload",
        params![provider.as_str(),source.host,list.id(),owner,revision+1,serde_json::to_string(&next.state)?])?;
    save_rows(&tx)?;
    #[cfg(feature = "enterprise-harness")]
    crate::enterprise_harness::metrics::before_queue_commit();
    tx.commit()?;
    #[cfg(feature = "enterprise-harness")]
    metric.finish("committed");
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
    fn local_repair_is_defaulted_durable_and_consumed_once() {
        let mut state = State {
            done: true,
            finished_at: Some(1000),
            eligible_at: 1300,
            pass_delay: 300,
            ..State::default()
        };
        let mut legacy = serde_json::to_value(&state).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("local_repair_pending");
        assert!(
            !serde_json::from_value::<State>(legacy)
                .unwrap()
                .local_repair_pending
        );
        for _ in 0..10 {
            state.taint_effect();
        }
        assert_eq!(state.eligible_at, 1015);
        let mut restored: State =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert!(restored.local_repair_pending && restored.local_repair_due(1015));
        restored.fresh_pass();
        assert!(!restored.local_repair_pending && !restored.done && !restored.coverage_valid);
        assert_eq!(restored.pass_delay, 300);
    }

    #[test]
    fn malformed_partition_resets_only_traversal_not_candidate_or_inventory_ownership() {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let source = Source::default();
        let mut state = State {
            github_partition: Some(GithubPartition {
                phase: PartitionPhase::Leaves,
                lower: Some(0),
                upper: Some(10),
                ..Default::default()
            }),
            coverage_valid: true,
            completed_at: Some(1),
            completed_total: Some(1),
            ..Default::default()
        };
        state.candidates.push_back(Candidate {
            isolated: false,
            identity: PrIdentity {
                source: source.clone(),
                repo: "fixture/repo".into(),
                number: 1,
            },
            id: "PR1".into(),
            head: "head".into(),
            created_at: Utc::now(),
            negative_at: Some(1),
            eligible_at: 2000,
            failures: 2,
        });
        for partition in [
            serde_json::to_value(state.github_partition.clone()).unwrap(),
            serde_json::json!({"version":99}),
            serde_json::json!({"phase":"FuturePhase"}),
        ] {
            let mut payload = serde_json::to_value(&state).unwrap();
            payload["github_partition"] = partition;
            conn.execute("INSERT OR REPLACE INTO queue_scan(provider,host,list,owner,revision,payload) VALUES('github',?1,?2,'fixture',1,?3)", params![source.host, CachedList::Reviewing.id(), payload.to_string()]).unwrap();
            let recovered = load(&conn, &source, CachedList::Reviewing, "fixture")
                .unwrap()
                .state;
            assert_eq!(
                recovered.github_partition.unwrap().phase,
                PartitionPhase::LowerBound
            );
            assert!(!recovered.coverage_valid);
            assert!(
                !recovered.local_repair_pending,
                "malformed provider metadata is not a local effect"
            );
            assert_eq!(recovered.completed_at, Some(1));
            assert_eq!(recovered.candidates.len(), 1);
            assert_eq!(recovered.candidates[0].negative_at, None);
            assert_eq!(recovered.candidates[0].eligible_at, 2000);
        }
    }
    #[test]
    fn persisted_exhausted_leaf_cannot_admit_a_forty_first_page() {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let source = Source::default();
        let window = Window { lo: 0, hi: 100 };
        let cursors = (1..=40).map(|n| format!("cursor-{n}")).collect::<Vec<_>>();
        let state = State {
            github_partition: Some(GithubPartition {
                phase: PartitionPhase::Leaves,
                lower: Some(0),
                upper: Some(100),
                windows_started: 1,
                active: Some(Leaf {
                    after: cursors.last().cloned(),
                    cursors,
                    pages: 40,
                    ..Leaf::new(window)
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let next = Commit {
            expected_revision: 0,
            state,
            removals: vec![],
        };
        assert!(commit(
            &conn,
            &source,
            CachedList::Reviewing,
            "fixture",
            &next,
            |_| Ok(())
        )
        .unwrap());
        let loaded = load(&conn, &source, CachedList::Reviewing, "fixture").unwrap();
        assert_eq!(
            loaded.state.github_partition.unwrap().phase,
            PartitionPhase::LowerBound
        );
        assert!(loaded.state.tainted);
    }
    #[test]
    fn old_checkpoint_keeps_small_queue_coverage_and_has_no_partition_probes() {
        let state = State {
            coverage_valid: true,
            completed_at: Some(1),
            completed_total: Some(42),
            ..Default::default()
        };
        let mut payload = serde_json::to_value(state).unwrap();
        payload.as_object_mut().unwrap().remove("github_partition");
        payload
            .as_object_mut()
            .unwrap()
            .remove("candidate_position");
        let state: State = serde_json::from_value(payload).unwrap();
        assert!(state.coverage_valid && state.github_partition.is_none());
        assert_eq!(state.completed_total, Some(42));
    }
    #[test]
    fn partition_receipts_reject_overtaken_action_without_losing_pending_windows() {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let source = Source::default();
        let list = CachedList::Reviewing;
        let partition = GithubPartition {
            phase: PartitionPhase::Leaves,
            lower: Some(0),
            upper: Some(2),
            pending: [Window { lo: 0, hi: 1 }, Window { lo: 1, hi: 2 }].into(),
            windows_started: 3,
            ..Default::default()
        };
        let receipt = Commit {
            expected_revision: 0,
            state: State {
                github_partition: Some(partition.clone()),
                receipt_id: Some(new_receipt_id()),
                coverage_valid: true,
                ..Default::default()
            },
            removals: vec![],
        };
        assert!(commit(&conn, &source, list, "fixture", &receipt, |tx| {
            save_owned_source_snapshot(tx, &source, list, &[], &Coverage::Complete, Some("fixture"))
        })
        .unwrap());
        assert!(accepted(&conn, &source, list, "fixture", &receipt).unwrap());
        let tx = conn.unchecked_transaction().unwrap();
        taint(&tx, &source, list, "fixture").unwrap();
        tx.commit().unwrap();
        assert!(!accepted(&conn, &source, list, "fixture", &receipt).unwrap());
        assert!(
            !commit(&conn, &source, list, "fixture", &receipt, |_| panic!(
                "overtaken rows cannot publish"
            ))
            .unwrap()
        );
        let loaded = load(&conn, &source, list, "fixture").unwrap();
        assert_eq!(loaded.state.github_partition, Some(partition));
        assert!(loaded.state.tainted && !loaded.state.coverage_valid);
        assert_eq!(
            snapshot_owner(&conn, &source, list).unwrap().as_deref(),
            Some("fixture")
        );
    }
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
