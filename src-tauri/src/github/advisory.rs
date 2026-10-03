//! Bounded receipts owned by one authenticated client. No detached loaders.
use super::{
    gates::{BaseRules, LastPusher},
    model::PrStack,
};
use std::{
    collections::HashMap,
    future::Future,
    hash::Hash,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Mutex as AsyncMutex, time::Instant};

const CAPACITY: usize = 512;
pub(super) const FAILURE_TTL: Duration = Duration::from_secs(5);
pub(super) const SUCCESS_TTL: Duration = Duration::from_secs(60);
#[derive(Debug)]
struct Receipts<V> {
    latest: Option<(Instant, V)>,
    success: Option<(Instant, V)>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LastKnown<V> {
    pub value: V,
    pub age_ms: u64,
}
type Slot<V> = Arc<AsyncMutex<Receipts<V>>>;
#[derive(Debug)]
pub(super) struct Cache<K, V> {
    slots: Mutex<HashMap<K, (Instant, Slot<V>)>>,
}
impl<K, V> Default for Cache<K, V> {
    fn default() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }
}
impl<K: Eq + Hash + Clone, V: Clone> Cache<K, V> {
    fn slot(&self, key: K) -> Option<Slot<V>> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, slot)) = slots.get_mut(&key) {
            *at = Instant::now();
            return Some(slot.clone());
        }
        if slots.len() >= CAPACITY {
            // Never evict a loader/waiter's slot and split its singleflight.
            let victim = slots
                .iter()
                .filter(|(_, (_, s))| Arc::strong_count(s) == 1)
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())?;
            slots.remove(&victim);
        }
        let slot = Arc::new(AsyncMutex::new(Receipts {
            latest: None,
            success: None,
        }));
        slots.insert(key, (Instant::now(), slot.clone()));
        Some(slot)
    }
    pub fn peek(&self, key: &K) -> Option<V> {
        self.peek_receipt(key).map(|(value, _)| value)
    }
    pub fn peek_receipt(&self, key: &K) -> Option<(V, Duration)> {
        let slots = self.slots.try_lock().ok()?;
        let slot = &slots.get(key)?.1;
        let receipt = slot.try_lock().ok()?;
        receipt
            .latest
            .as_ref()
            .filter(|(until, _)| *until > Instant::now())
            .map(|(until, value)| {
                (
                    value.clone(),
                    until.saturating_duration_since(Instant::now()),
                )
            })
    }
    /// Display only: never returned by load/peek to action consumers.
    pub fn last_success(&self, key: &K) -> Option<LastKnown<V>> {
        let slots = self.slots.try_lock().ok()?;
        let slot = &slots.get(key)?.1;
        let receipt = slot.try_lock().ok()?;
        receipt.success.as_ref().map(|(at, value)| LastKnown {
            value: value.clone(),
            age_ms: at.elapsed().as_millis() as u64,
        })
    }
    pub async fn load<F: Future<Output = V>>(
        &self,
        key: K,
        deadline: Instant,
        ttl: impl Fn(&V) -> Duration,
        work: F,
    ) -> Option<V> {
        self.load_receipt(key, deadline, ttl, work)
            .await
            .map(|(value, _)| value)
    }
    /// Value and original remaining lifetime are read under the same slot lock.
    pub async fn load_receipt<F: Future<Output = V>>(
        &self,
        key: K,
        deadline: Instant,
        ttl: impl Fn(&V) -> Duration,
        work: F,
    ) -> Option<(V, Duration)> {
        let slot = self.slot(key)?;
        tokio::time::timeout_at(deadline, async {
            let mut receipt = slot.lock().await;
            if let Some((until, value)) = receipt.latest.as_ref() {
                if *until > Instant::now() {
                    return (
                        value.clone(),
                        until.saturating_duration_since(Instant::now()),
                    );
                }
            }
            let value = work.await;
            let lifetime = ttl(&value);
            // Every cache in this module uses FAILURE_TTL for inconclusive
            // attempts; longer receipts certify an actual successful read.
            if lifetime > FAILURE_TTL {
                receipt.success = Some((Instant::now(), value.clone()));
            }
            receipt.latest = Some((Instant::now() + lifetime, value.clone()));
            (value, lifetime)
        })
        .await
        .ok()
    }
}

pub(crate) type StackKey = (String, u64, String, String);
#[derive(Default, Debug)]
pub(super) struct Advisory {
    pub rules: Cache<(String, String), BaseRules>,
    pub pushers: Cache<(String, String, String), LastPusher>,
    pub stacks: Cache<StackKey, PrStack>,
    order: Mutex<(u64, HashMap<String, u64>)>,
    rotations: Mutex<HashMap<String, (Instant, usize)>>,
}
impl Advisory {
    pub fn rotation(&self, key: String, len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        let mut rotations = self.rotations.lock().unwrap_or_else(|e| e.into_inner());
        if rotations.len() >= CAPACITY && !rotations.contains_key(&key) {
            if let Some(oldest) = rotations
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            {
                rotations.remove(&oldest);
            }
        }
        let entry = rotations.entry(key).or_insert((Instant::now(), 0));
        let start = entry.1 % len;
        *entry = (Instant::now(), (start + 1) % len);
        start
    }
    pub fn order(&self, keys: &[String]) -> Vec<usize> {
        let order = self.order.lock().unwrap_or_else(|e| e.into_inner());
        let mut indices: Vec<_> = (0..keys.len()).collect();
        indices.sort_by_key(|i| order.1.get(&keys[*i]).copied().unwrap_or(0));
        indices
    }
    // Record first admitted work, before its provider await. Admission is an
    // opportunity, not a successful observation; cancellation keeps this mark.
    pub fn offered(&self, key: String) {
        let mut order = self.order.lock().unwrap_or_else(|e| e.into_inner());
        if order.1.len() >= CAPACITY && !order.1.contains_key(&key) {
            if let Some(oldest) = order
                .1
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(k, _)| k.clone())
            {
                order.1.remove(&oldest);
            }
        }
        order.0 = order.0.wrapping_add(1);
        let serial = order.0;
        order.1.insert(key, serial);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn failed_attempt_keeps_original_success_without_fresh_peek() {
        let cache = Cache::<usize, usize>::default();
        cache
            .load(1, Instant::now() + SUCCESS_TTL, |_| SUCCESS_TTL, async {
                42
            })
            .await;
        tokio::time::advance(SUCCESS_TTL).await;
        cache
            .load(1, Instant::now() + SUCCESS_TTL, |_| FAILURE_TTL, async {
                0
            })
            .await;
        assert_eq!(
            cache.peek(&1),
            Some(0),
            "action peek returns the failure, never the retained success"
        );
        assert_eq!(
            cache.last_success(&1).map(|receipt| receipt.value),
            Some(42)
        );
        tokio::time::advance(FAILURE_TTL).await;
        assert_eq!(cache.peek(&1), None);
        assert_eq!(
            cache.last_success(&1).map(|receipt| receipt.value),
            Some(42)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn incremental_eviction_keeps_recent_receipts_and_cancelled_leader_is_retryable() {
        let cache = Arc::new(Cache::<usize, usize>::default());
        for i in 0..CAPACITY {
            cache
                .load(i, Instant::now() + SUCCESS_TTL, |_| SUCCESS_TTL, async {
                    i
                })
                .await
                .unwrap();
            tokio::time::advance(Duration::from_millis(1)).await;
        }
        cache
            .load(
                CAPACITY,
                Instant::now() + SUCCESS_TTL,
                |_| SUCCESS_TTL,
                async { CAPACITY },
            )
            .await
            .unwrap();
        assert_eq!(cache.peek(&0), None);
        assert_eq!(cache.peek(&(CAPACITY - 1)), Some(CAPACITY - 1));
        assert_eq!(cache.slots.lock().unwrap().len(), CAPACITY);
        let worker_cache = cache.clone();
        let worker = tokio::spawn(async move {
            worker_cache
                .load(
                    9999,
                    Instant::now() + SUCCESS_TTL,
                    |_| SUCCESS_TTL,
                    std::future::pending(),
                )
                .await
        });
        tokio::task::yield_now().await;
        assert_eq!(
            cache.peek(&9999),
            None,
            "peek does not wait for an active loader"
        );
        worker.abort();
        let _ = worker.await;
        assert_eq!(
            cache
                .load(9999, Instant::now() + SUCCESS_TTL, |_| SUCCESS_TTL, async {
                    42
                })
                .await,
            Some(42)
        );
    }
}
