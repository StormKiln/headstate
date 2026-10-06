//! Diagnostic lineage follows accepted rows; it never grants publication authority.
use super::*;
use crate::measurement::{self as m, Acceptance, Event, Key, OpaqueId, Recorder};

#[derive(Default)]
pub(super) struct Observer {
    pub recorder: Option<Arc<Recorder>>,
    rows: Mutex<HashMap<PollKey, RowReference>>,
}
struct RowReference {
    viewer: String,
    private_owner: String,
    owner: OpaqueId,
    receipt: Option<OpaqueId>,
    revision: Option<u64>,
}
impl Observer {
    pub(super) fn recorder(&self) -> Option<&Recorder> {
        match self.recorder.as_deref() {
            Some(recorder) => Some(recorder),
            None => crate::measurement_desktop::recorder(),
        }
    }
    pub fn reference(&self, status: &Status) -> Option<OpaqueId> {
        let recorder = self.recorder()?;
        if !recorder.enabled() {
            return None;
        }
        let rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let row = rows.get(&(status.source.clone(), status.list))?;
        row.receipt
            .as_ref()
            .filter(|id| row.revision == status.receipt_revision && recorder.is_live_receipt(id))
            .cloned()
    }
}
impl SourcePolls {
    pub(super) fn observe_measurement(&self, status: &Status, outcome: Acceptance) {
        let Some(recorder) = self.7.recorder().filter(|r| r.enabled()) else {
            return;
        };
        if status.source.provider != crate::identity::Provider::Github {
            return;
        }
        let (viewer, count) = {
            let receipts = self.2.lock().unwrap_or_else(|e| e.into_inner());
            let Some((_, receipt)) = receipts.get(&(status.source.clone(), status.list)) else {
                return;
            };
            let Some(viewer) = receipt.viewer.as_deref() else {
                return;
            };
            (viewer.to_owned(), receipt.prs.len())
        };
        let key = (status.source.clone(), status.list);
        let mut rows = self.7.rows.lock().unwrap_or_else(|e| e.into_inner());
        // A -> B -> A is a new owner incarnation, not resurrection of A's retired handle.
        let private_owner = rows
            .get(&key)
            .filter(|r| r.viewer == viewer)
            .map(|r| r.private_owner.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}:{:?}:{:?}:{}:{}",
                    self.3, status.source, status.list, viewer, status.revision
                )
            });
        if let Some(old) = rows.get(&key).filter(|r| r.viewer != viewer) {
            recorder.retire(&old.owner);
        }
        let Some(owner) = recorder.intern(Key::Owner(&private_owner)) else {
            return;
        };
        let tracked = rows.entry(key).or_insert_with(|| RowReference {
            viewer: viewer.clone(),
            private_owner: private_owner.clone(),
            owner: owner.clone(),
            receipt: None,
            revision: None,
        });
        if tracked.owner != owner {
            recorder.retire(&tracked.owner);
            *tracked = RowReference {
                viewer,
                private_owner,
                owner: owner.clone(),
                receipt: None,
                revision: None,
            };
        }
        if outcome == Acceptance::Accepted && tracked.revision != status.receipt_revision {
            if let Some(old) = tracked.receipt.take() {
                recorder.retire(&old);
            }
            tracked.receipt = recorder.receipt_reference(&owner);
            tracked.revision = status.receipt_revision;
        }
        let receipt = tracked.receipt.clone();
        drop(rows);
        let phase = match status.phase {
            Phase::NotRequested => m::Phase::NotRequested,
            Phase::Fetching => m::Phase::Fetching,
            Phase::Ready => m::Phase::Ready,
            Phase::Partial => m::Phase::Partial,
            Phase::Unknown => m::Phase::Unknown,
            Phase::Retrying => m::Phase::Retrying,
            Phase::Failed => m::Phase::Failed,
            Phase::NotAsked => m::Phase::NotAsked,
        };
        let coverage = match status.coverage {
            Some(Coverage::Complete) => m::Coverage::Complete,
            Some(Coverage::Partial { .. }) => m::Coverage::Partial,
            _ => m::Coverage::Unknown,
        };
        let age = status
            .last_received_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .and_then(|at| {
                u64::try_from(
                    chrono::Utc::now()
                        .signed_duration_since(at)
                        .num_milliseconds(),
                )
                .ok()
            });
        if let Some(count) = recorder.measured_count(count as u64) {
            recorder.record(Event::QueueReceipt {
                owner,
                receipt,
                operation: crate::queue_measurement::current(),
                list: match status.list {
                    CachedList::Authored => m::List::Authored,
                    CachedList::Reviewing => m::List::Reviewing,
                },
                revision: status.revision,
                receipt_revision: status.receipt_revision,
                phase,
                coverage,
                rows: count,
                receipt_age_ms: age,
                outcome,
            });
        }
    }
}
