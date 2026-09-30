//! Share simultaneous reads across pollers and paired clients. Entries live
//! only while callers hold them; this is not a second freshness cache.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::Mutex as AsyncMutex;
type Slot<T> = AsyncMutex<Option<T>>;
pub struct Reads<T>(Mutex<HashMap<String, Weak<Slot<T>>>>);
impl<T> Default for Reads<T> {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

impl<T: Clone> Reads<T> {
    pub async fn run(&self, key: String, read: impl std::future::Future<Output = T>) -> T {
        let slot = {
            let mut slots = self.0.lock().unwrap_or_else(|e| e.into_inner());
            slots.retain(|_, slot| slot.strong_count() > 0);
            if let Some(slot) = slots.get(&key).and_then(Weak::upgrade) {
                slot
            } else {
                let slot = Arc::new(AsyncMutex::new(None));
                slots.insert(key, Arc::downgrade(&slot));
                slot
            }
        };
        let mut result = slot.lock().await;
        if let Some(result) = result.as_ref() {
            return result.clone();
        }
        let value = read.await;
        *result = Some(value.clone());
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn concurrent_consumers_share_one_read_but_later_reads_are_fresh() {
        let reads = Reads::<usize>::default();
        let count = AtomicUsize::new(0);
        let load = || async {
            let n = count.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            n
        };
        let (a, b, separate) = tokio::join!(
            reads.run("host/list".into(), load()),
            reads.run("host/list".into(), load()),
            reads.run("other/list".into(), load())
        );
        assert_eq!(a, b);
        assert_ne!(a, separate);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        reads.run("host/list".into(), load()).await;
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }
}
