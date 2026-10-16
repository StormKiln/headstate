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
    #[cfg(test)]
    pub(crate) fn paired_for_test() -> Self {
        Self {
            display_name: "synthetic paired consumer".into(),
            capability: Some(PairingCapability::fresh()),
        }
    }
    #[cfg(test)]
    pub(crate) fn retire_for_test(&self) {
        if let Some(capability) = &self.capability {
            capability.retire();
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
    fn retirement_waits_for_an_admitted_operation_and_then_refuses_access() {
        let cap = PairingCapability::fresh();
        let context = DispatchContext {
            display_name: "fixture".into(),
            capability: Some(cap.clone()),
        };
        let held = context.guard().unwrap();
        let (started, waiting) = std::sync::mpsc::channel();
        let retire = std::thread::spawn(move || {
            started.send(()).unwrap();
            cap.retire();
        });
        waiting
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        drop(held);
        retire.join().unwrap();
        assert!(context.guard().is_err());
    }
}
