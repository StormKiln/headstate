//! Internal dispatch authority. Never deserialized from a caller's arguments.
use crate::store::devices::PairedDevice;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard};

#[derive(Clone, Debug)]
pub struct PairingCapability {
    id: u64,
    live: Arc<RwLock<bool>>,
}
impl PairingCapability {
    pub(super) fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .expect("pairing incarnation exhausted");
        Self {
            id,
            live: Arc::new(RwLock::new(true)),
        }
    }
    pub(super) fn retire(&self) {
        *self.live.write().unwrap_or_else(|e| e.into_inner()) = false;
    }
    pub(crate) fn guard(&self) -> Result<RwLockReadGuard<'_, bool>, String> {
        let guard = self.live.read().unwrap_or_else(|e| e.into_inner());
        if *guard {
            Ok(guard)
        } else {
            Err("pairing has been retired".into())
        }
    }
}

#[derive(Clone, Debug)]
pub struct AuthorizedDevice {
    pub device: PairedDevice,
    pub(super) capability: PairingCapability,
}
impl AuthorizedDevice {
    pub(super) fn fresh(device: PairedDevice) -> Self {
        Self {
            device,
            capability: PairingCapability::fresh(),
        }
    }
    pub(super) fn context(&self) -> DispatchContext {
        DispatchContext {
            display_name: self.device.name.clone(),
            capability: Some(self.capability.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DispatchContext {
    pub display_name: String,
    capability: Option<PairingCapability>,
}
impl DispatchContext {
    pub(crate) fn desktop() -> Self {
        Self {
            display_name: "desktop".into(),
            capability: None,
        }
    }
    pub(crate) fn principal(&self) -> u64 {
        self.capability.as_ref().map_or(0, |c| c.id)
    }
    /// Registry lock precedes this short guard. No await under either lock.
    pub(crate) fn guard(&self) -> Result<Option<RwLockReadGuard<'_, bool>>, String> {
        self.capability
            .as_ref()
            .map(PairingCapability::guard)
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retirement_and_registry_selection_finish_without_lock_inversion() {
        let cap = PairingCapability::fresh();
        let context = DispatchContext {
            display_name: "fixture".into(),
            capability: Some(cap.clone()),
        };
        let registry = Arc::new(crate::stats_demand::Registry::default());
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let owner = crate::store::stats_owner::capture_verified(&conn, "fixture").unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let lease = registry
            .acquire(
                &tx,
                &owner,
                &context,
                crate::store::pr_backfill_scope::BackfillScope {
                    scope_key: "merged|*|org:fixture".into(),
                    scope_kind: "org".into(),
                    scope_value: "fixture".into(),
                    measure: "merged".into(),
                    horizon_days: 1,
                },
                std::time::Instant::now(),
                false,
            )
            .unwrap();
        tx.commit().unwrap();
        let held = context.guard().unwrap();
        let (started, waiting) = std::sync::mpsc::channel();
        let retire = std::thread::spawn(move || {
            started.send(()).unwrap();
            cap.retire();
        });
        waiting
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        let querying = registry.clone();
        let query = std::thread::spawn(move || {
            done.send(querying.has_live_interest(std::time::Instant::now()))
                .unwrap();
        });
        drop(held);
        retire.join().unwrap();
        finished
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        query.join().unwrap();
        assert!(!registry.has_live_interest(std::time::Instant::now()));
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        assert!(registry
            .update(
                &tx,
                &context,
                &lease.handle,
                1,
                false,
                std::time::Instant::now()
            )
            .is_err());
        assert!(registry
            .pick(&tx, &owner, std::time::Instant::now(), chrono::Utc::now())
            .unwrap()
            .is_none());
    }
}
