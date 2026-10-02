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
    #[cfg(test)]
    pub async fn run(&self, key: String, read: impl std::future::Future<Output = T>) -> T {
        self.run_checked(key, read, |_| true).await
    }
    pub async fn run_checked(
        &self,
        key: String,
        read: impl std::future::Future<Output = T>,
        keep: impl Fn(&T) -> bool,
    ) -> T {
        self.run_inner(key, read, keep, None)
            .await
            .expect("unbounded wait")
    }
    pub async fn run_until(
        &self,
        key: String,
        read: impl std::future::Future<Output = T>,
        keep: impl Fn(&T) -> bool,
        deadline: tokio::time::Instant,
    ) -> Result<T, ()> {
        self.run_inner(key, read, keep, Some(deadline)).await
    }
    async fn run_inner(
        &self,
        key: String,
        read: impl std::future::Future<Output = T>,
        keep: impl Fn(&T) -> bool,
        deadline: Option<tokio::time::Instant>,
    ) -> Result<T, ()> {
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
        let mut result = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, slot.lock())
                .await
                .map_err(|_| ())?,
            None => slot.lock().await,
        };
        if let Some(result) = result.as_ref() {
            return Ok(result.clone());
        }
        let value = read.await;
        if keep(&value) {
            *result = Some(value.clone());
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test(start_paused = true)]
    async fn joining_wait_uses_own_deadline_and_failed_leader_does_not_poison_retry() {
        let reads = Reads::<Result<usize, ()>>::default();
        let started = tokio::sync::Notify::new();
        let release = tokio::sync::Notify::new();
        let leader = reads.run_checked(
            "key".into(),
            async {
                started.notify_one();
                release.notified().await;
                Err(())
            },
            Result::is_ok,
        );
        let follower = async {
            started.notified().await;
            let result = reads
                .run_until(
                    "key".into(),
                    async { Ok(2) },
                    Result::is_ok,
                    tokio::time::Instant::now() + std::time::Duration::from_secs(1),
                )
                .await;
            assert_eq!(result, Err(()));
            release.notify_one();
        };
        let (result, _) = tokio::join!(leader, follower);
        assert_eq!(result, Err(()));
        assert_eq!(
            reads
                .run_checked("key".into(), async { Ok(3) }, Result::is_ok)
                .await,
            Ok(3)
        );
    }

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
