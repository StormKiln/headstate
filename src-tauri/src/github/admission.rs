//! Account-owned admission. Guards hold no mutex across network awaits.
use super::client::ClientError;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Semaphore, SemaphorePermit},
    time::Instant,
};

pub const READ_LIMIT: usize = 4;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadClass {
    Foreground,
    Background,
    Advisory,
}
#[derive(Clone, Debug)]
pub struct ReadContext {
    pub class: ReadClass,
    pub deadline: Instant,
    pub(crate) attempts: Option<AttemptAllowance>,
    pub(crate) first_attempt: Option<FirstAttempt>,
    pub(super) live: Option<tokio::sync::watch::Receiver<Option<LiveRead>>>,
}
impl ReadContext {
    pub fn new(class: ReadClass, budget: Duration) -> Self {
        Self {
            class,
            deadline: Instant::now() + budget,
            attempts: None,
            first_attempt: None,
            live: None,
        }
    }
}
/// Only queued shared scans carry live demand; dispatched requests freeze it.
#[derive(Clone, Copy, Debug)]
pub(super) struct LiveRead {
    pub class: ReadClass,
    pub deadline: Instant,
}

type FirstAttemptCallback = Box<dyn FnOnce() + Send>;

/// An operation-local, once-only scheduling mark after admission, before HTTP.
/// Clones share it across retries and child reads. It carries no receipt.
#[derive(Clone)]
pub(crate) struct FirstAttempt {
    callback: Arc<Mutex<Option<FirstAttemptCallback>>>,
}
impl std::fmt::Debug for FirstAttempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirstAttempt").finish_non_exhaustive()
    }
}
impl FirstAttempt {
    pub(crate) fn new(callback: impl FnOnce() + Send + 'static) -> Self {
        Self {
            callback: Arc::new(Mutex::new(Some(Box::new(callback)))),
        }
    }
    pub(super) fn observe(&self) {
        let callback = self
            .callback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        // Release the observer lock before touching the scheduler. Admission's
        // accounting lock was already released by enter().
        if let Some(callback) = callback {
            callback();
        }
    }
}

/// One operation's limits; child reservations share the same atomic ledger.
#[derive(Clone, Debug)]
pub(crate) struct AttemptAllowance {
    ledger: Arc<Mutex<Vec<usize>>>,
    slots: Vec<usize>,
}
impl AttemptAllowance {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            ledger: Arc::new(Mutex::new(vec![limit])),
            slots: vec![0],
        }
    }
    pub(crate) fn child(&self, limit: usize) -> Self {
        let mut ledger = self.ledger.lock().unwrap_or_else(|e| e.into_inner());
        let mut slots = self.slots.clone();
        slots.push(ledger.len());
        ledger.push(limit);
        Self {
            ledger: self.ledger.clone(),
            slots,
        }
    }
    pub(crate) fn remaining(&self) -> usize {
        let ledger = self.ledger.lock().unwrap_or_else(|e| e.into_inner());
        self.slots
            .iter()
            .map(|slot| ledger[*slot])
            .min()
            .unwrap_or(0)
    }
    fn debit(&self) -> Result<(), ClientError> {
        let mut ledger = self.ledger.lock().unwrap_or_else(|e| e.into_inner());
        if self.slots.iter().any(|slot| ledger[*slot] == 0) {
            return Err(refused("operation attempt allowance is spent"));
        }
        for slot in &self.slots {
            ledger[*slot] -= 1;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) enum Bucket {
    Graphql = 0,
    Rest = 1,
}
#[derive(Default, Debug)]
struct Quota {
    remaining: Option<u64>,
    reset: Option<u64>,
    unknown_until: Option<Instant>,
    blocked: Option<Instant>,
    reserve_until: Option<Instant>,
    probing: bool,
    revision: u64,
}
#[derive(Debug)]
struct State {
    quotas: [Quota; 2],
    secondary: Option<Instant>,
    secondary_revision: u64,
    cycle: Instant,
    spent: u8,
}
#[derive(Debug)]
pub(super) struct Admission {
    total: Semaphore,
    background: Semaphore,
    state: Mutex<State>,
}
impl Default for Admission {
    fn default() -> Self {
        Self {
            total: Semaphore::new(READ_LIMIT),
            background: Semaphore::new(2),
            state: Mutex::new(State {
                quotas: Default::default(),
                secondary: None,
                secondary_revision: 0,
                cycle: Instant::now(),
                spent: 0,
            }),
        }
    }
}
fn refused(message: &str) -> ClientError {
    ClientError::NotDispatched(message.into())
}
pub(super) struct Attempt<'a> {
    admission: &'a Admission,
    bucket: Bucket,
    probe: Option<(u64, Option<u64>, bool)>,
    done: bool,
    pub(super) deadline: Option<Instant>,
    _total: Option<SemaphorePermit<'a>>,
    _background: Option<SemaphorePermit<'a>>,
}
impl Attempt<'_> {
    pub fn complete(&mut self) {
        self.done = true;
    }
}
impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if let Some((revision, secondary_revision, primary_recovery)) = self.probe {
            let mut state = self
                .admission
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let quota = &mut state.quotas[self.bucket as usize];
            quota.probing = false;
            if primary_recovery && quota.revision == revision {
                if self.done {
                    quota.blocked = None;
                    quota.reserve_until = None;
                } else {
                    let retry = Instant::now() + Duration::from_secs(1);
                    quota.blocked = Some(quota.blocked.map_or(retry, |old| old.max(retry)));
                }
            }
            if secondary_revision == Some(state.secondary_revision) {
                if self.done {
                    if state.secondary.is_some_and(|until| until <= Instant::now()) {
                        state.secondary = None;
                    }
                } else {
                    let retry = Instant::now() + Duration::from_secs(1);
                    state.secondary = Some(state.secondary.map_or(retry, |old| old.max(retry)));
                }
            }
        }
    }
}
impl Admission {
    fn enter(
        &self,
        bucket: Bucket,
        class: Option<ReadClass>,
        allowance: Option<&AttemptAllowance>,
    ) -> Result<Attempt<'_>, ClientError> {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.secondary.is_some_and(|until| until > now) {
            return Err(refused("provider cooldown is active"));
        }
        let secondary_recovery = state.secondary.is_some();
        let quota = &mut state.quotas[bucket as usize];
        let reserve = class.is_some() && matches!(bucket, Bucket::Rest);
        let deadline = quota
            .blocked
            .into_iter()
            .chain(if reserve { quota.reserve_until } else { None })
            .max();
        if deadline.is_some_and(|until| until > now) {
            return Err(refused("provider quota retry deadline is active"));
        }
        if quota.probing {
            return Err(refused("provider recovery probe is in progress"));
        }
        // A secondary recovery is serialized across BOTH buckets.
        if secondary_recovery && state.quotas.iter().any(|q| q.probing) {
            return Err(refused("provider recovery probe is in progress"));
        }
        if class == Some(ReadClass::Advisory) {
            if now.duration_since(state.cycle) >= Duration::from_secs(30) {
                state.cycle = now;
                state.spent = 0;
            }
            if state.spent >= 8 {
                return Err(refused(
                    "advisory attempt allowance is spent for this cycle",
                ));
            }
        }
        if let Some(allowance) = allowance {
            allowance.debit()?;
        }
        let probe = if deadline.is_some() || secondary_recovery {
            let quota = &mut state.quotas[bucket as usize];
            quota.probing = true;
            Some((
                quota.revision,
                secondary_recovery.then_some(state.secondary_revision),
                deadline.is_some(),
            ))
        } else {
            None
        };
        if class == Some(ReadClass::Advisory) {
            state.spent += 1;
        }
        Ok(Attempt {
            admission: self,
            bucket,
            probe,
            done: false,
            deadline: None,
            _total: None,
            _background: None,
        })
    }
    pub async fn read(
        &self,
        bucket: Bucket,
        context: ReadContext,
    ) -> Result<Attempt<'_>, ClientError> {
        let mut live = context.live.clone();
        loop {
            let demand = match &mut live {
                Some(receiver) => (*receiver.borrow_and_update())
                    .ok_or_else(|| refused("shared read has no live consumers"))?,
                None => LiveRead {
                    class: context.class,
                    deadline: context.deadline,
                },
            };
            let deadline = demand.deadline;
            let acquire = async {
                let background = if demand.class != ReadClass::Foreground {
                    Some(
                        self.background
                            .acquire()
                            .await
                            .map_err(|_| refused("read admission closed"))?,
                    )
                } else {
                    None
                };
                let total = self
                    .total
                    .acquire()
                    .await
                    .map_err(|_| refused("read admission closed"))?;
                if Instant::now() >= deadline {
                    return Err(refused("read deadline elapsed before dispatch"));
                }
                let mut attempt = self
                    .enter(bucket, Some(demand.class), context.attempts.as_ref())
                    .map_err(|error| match error {
                        ClientError::NotDispatched(message) if message.starts_with("provider") => {
                            ClientError::RateLimited(message)
                        }
                        other => other,
                    })?;
                attempt._background = background;
                attempt._total = Some(total);
                attempt.deadline = Some(deadline);
                if let Some(observer) = &context.first_attempt {
                    observer.observe();
                }
                Ok(attempt)
            };
            let wait = tokio::time::timeout_at(deadline, acquire);
            if let Some(receiver) = &mut live {
                tokio::select! {
                    biased;
                    changed = receiver.changed() => {
                        if changed.is_err() { return Err(refused("shared read demand closed")); }
                        // Dropping only this pre-dispatch acquisition releases
                        // any partial permits; attempts have not been debited.
                        continue;
                    }
                    result = wait => return result.map_err(|_| refused("read deadline elapsed before dispatch"))?,
                }
            } else {
                return wait
                    .await
                    .map_err(|_| refused("read deadline elapsed before dispatch"))?;
            }
        }
    }

    pub fn write(&self, bucket: Bucket) -> Result<Attempt<'_>, ClientError> {
        self.enter(bucket, None, None)
    }
    pub(super) fn retry_wait(&self, bucket: Bucket) -> u64 {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.quotas[bucket as usize]
            .blocked
            .into_iter()
            .chain(state.secondary)
            .map(|until| {
                let wait = until.saturating_duration_since(Instant::now());
                wait.as_secs()
                    .saturating_add(u64::from(wait.subsec_nanos() > 0))
            })
            .max()
            .unwrap_or(0)
    }
    pub fn remaining(&self, bucket: Bucket) -> Option<u64> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let quota = &state.quotas[bucket as usize];
        if quota.reset.is_some_and(|at| seconds_until(at) == 0)
            || quota.unknown_until.is_some_and(|at| at <= Instant::now())
        {
            None
        } else {
            quota.remaining
        }
    }
    pub fn rest_available(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        !state.quotas[1]
            .reserve_until
            .is_some_and(|until| until > now)
    }
    pub fn observe(&self, bucket: Bucket, remaining: Option<u64>, reset: Option<u64>) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let quota = &mut state.quotas[bucket as usize];
        if matches!((quota.reset, reset), (Some(old), Some(new)) if new < old) {
            return false;
        }
        if reset.is_some() && reset != quota.reset {
            quota.remaining = None;
            quota.reserve_until = None;
        }
        if let Some(reset) = reset {
            quota.reset = Some(reset);
            quota.unknown_until = None;
        }
        if let Some(remaining) = remaining {
            if quota.unknown_until.is_some_and(|at| at <= Instant::now()) {
                quota.remaining = None;
            }
            quota.unknown_until = quota
                .reset
                .is_none()
                .then(|| Instant::now() + Duration::from_secs(60));
            if quota.probing && reset.is_none() && quota.reset.is_none() {
                quota.remaining = None;
                quota.reserve_until = None;
            }
            quota.remaining = Some(quota.remaining.map_or(remaining, |old| old.min(remaining)));
            if matches!(bucket, Bucket::Rest) && quota.remaining.is_some_and(|r| r <= 500) {
                let seconds = reset.map(seconds_until).unwrap_or(60).max(1);
                quota.reserve_until = Some(provider_deadline(seconds));
                quota.revision += 1;
            }
        }
        true
    }
    pub fn limit(&self, bucket: Bucket, seconds: u64, secondary: bool) -> u64 {
        let seconds = seconds.max(1);
        let until = provider_deadline(seconds);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if secondary {
            state.secondary = Some(state.secondary.map_or(until, |old| old.max(until)));
            state.secondary_revision += 1;
        } else {
            let quota = &mut state.quotas[bucket as usize];
            quota.blocked = Some(quota.blocked.map_or(until, |old| old.max(until)));
            quota.revision += 1;
        }
        seconds
    }
}
// Never shorten a provider deadline to a local retry cap. Saturate only at
// the platform clock's representable horizon for hostile/extreme headers.
fn provider_deadline(seconds: u64) -> Instant {
    let now = Instant::now();
    if let Some(until) = now.checked_add(Duration::from_secs(seconds)) {
        return until;
    }
    let (mut low, mut high) = (0, seconds);
    while high - low > 1 {
        let mid = low + (high - low) / 2;
        if now.checked_add(Duration::from_secs(mid)).is_some() {
            low = mid;
        } else {
            high = mid;
        }
    }
    now + Duration::from_secs(low)
}

pub(super) fn seconds_until(epoch: u64) -> u64 {
    epoch.saturating_sub(chrono::Utc::now().timestamp().max(0) as u64)
}
pub(super) fn retry_seconds(value: &str, now: chrono::DateTime<chrono::Utc>) -> Option<u64> {
    value.trim().parse().ok().or_else(|| {
        chrono::DateTime::parse_from_rfc2822(value.trim())
            .ok()
            .map(|at| at.timestamp().saturating_sub(now.timestamp()).max(0) as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn newer_cross_protocol_cooldown_survives_old_probe_cleanup() {
        for (first, other) in [
            (Bucket::Graphql, Bucket::Rest),
            (Bucket::Rest, Bucket::Graphql),
        ] {
            for completed in [false, true] {
                let admission = Admission::default();
                admission.limit(first, 1, true);
                tokio::time::advance(Duration::from_secs(1)).await;
                let mut probe = admission.write(first).unwrap();
                admission.limit(other, 60, true);
                if completed {
                    probe.complete();
                }
                drop(probe);
                tokio::time::advance(Duration::from_secs(2)).await;
                assert!(
                    admission.write(first).is_err(),
                    "old probe shortened newer shared refusal"
                );
                tokio::time::advance(Duration::from_secs(58)).await;
                let mut recovery = admission.write(first).unwrap();
                assert!(admission.write(other).is_err());
                recovery.complete();
                drop(recovery);
                assert!(admission.write(other).is_ok());
            }
        }
    }
    #[tokio::test(start_paused = true)]
    async fn secondary_recovery_in_other_bucket_cannot_clear_primary_exhaustion() {
        let admission = Admission::default();
        admission.limit(Bucket::Graphql, 1800, false);
        admission.limit(Bucket::Rest, 1, true);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(admission.write(Bucket::Graphql).is_err());
        let mut recovery = admission.write(Bucket::Rest).unwrap();
        recovery.complete();
        drop(recovery);
        assert!(admission.write(Bucket::Rest).is_ok());
        assert!(admission.write(Bucket::Graphql).is_err());
        tokio::time::advance(Duration::from_secs(1799)).await;
        let mut recovery = admission.write(Bucket::Graphql).unwrap();
        recovery.complete();
        drop(recovery);
        assert!(admission.write(Bucket::Graphql).is_ok());
    }
    fn context(class: ReadClass) -> ReadContext {
        ReadContext::new(class, Duration::from_secs(30))
    }
    #[tokio::test(start_paused = true)]
    async fn first_attempt_observer_ignores_refusals_and_runs_once_outside_accounting_lock() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let admission = Arc::new(Admission::default());
        let observed = Arc::new(AtomicUsize::new(0));
        let count = observed.clone();
        let owner = admission.clone();
        let mut read = context(ReadClass::Advisory);
        read.first_attempt = Some(FirstAttempt::new(move || {
            assert!(
                owner.state.try_lock().is_ok(),
                "observer cannot run under accounting lock"
            );
            count.fetch_add(1, Ordering::SeqCst);
        }));
        let mut expired = read.clone();
        expired.deadline = Instant::now();
        assert!(admission.read(Bucket::Rest, expired).await.is_err());
        let mut exhausted = read.clone();
        exhausted.attempts = Some(AttemptAllowance::new(0));
        assert!(admission.read(Bucket::Rest, exhausted).await.is_err());
        admission.limit(Bucket::Rest, 5, true);
        assert!(admission.read(Bucket::Rest, read.clone()).await.is_err());
        assert_eq!(observed.load(Ordering::SeqCst), 0);
        tokio::time::advance(Duration::from_secs(5)).await;
        let mut attempt = admission.read(Bucket::Rest, read.clone()).await.unwrap();
        assert_eq!(
            observed.load(Ordering::SeqCst),
            1,
            "mark precedes provider completion"
        );
        attempt.complete();
        drop(attempt);
        // Retry/stage clones share the same once-only callback.
        admission.read(Bucket::Graphql, read).await.unwrap();
        assert_eq!(observed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn canceled_capacity_waiter_does_not_consume_first_attempt_observer() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let admission = Admission::default();
        let a = admission
            .read(Bucket::Rest, context(ReadClass::Background))
            .await
            .unwrap();
        let b = admission
            .read(Bucket::Rest, context(ReadClass::Background))
            .await
            .unwrap();
        let observed = Arc::new(AtomicUsize::new(0));
        let count = observed.clone();
        let mut read = ReadContext::new(ReadClass::Advisory, Duration::from_secs(1));
        read.first_attempt = Some(FirstAttempt::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(admission.read(Bucket::Rest, read.clone()).await.is_err());
        assert_eq!(observed.load(Ordering::SeqCst), 0);
        drop((a, b));
        read.deadline = Instant::now() + Duration::from_secs(1);
        admission.read(Bucket::Rest, read).await.unwrap();
        assert_eq!(observed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn shared_advisory_exhaustion_preserves_unoffered_turn_for_next_cycle() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let admission = Admission::default();
        for _ in 0..8 {
            admission
                .read(Bucket::Rest, context(ReadClass::Advisory))
                .await
                .unwrap();
        }
        let observed = Arc::new(AtomicUsize::new(0));
        let count = observed.clone();
        let mut read = context(ReadClass::Advisory);
        read.first_attempt = Some(FirstAttempt::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(admission.read(Bucket::Rest, read.clone()).await.is_err());
        assert_eq!(observed.load(Ordering::SeqCst), 0);
        tokio::time::advance(Duration::from_secs(30)).await;
        read.deadline = Instant::now() + Duration::from_secs(1);
        admission.read(Bucket::Rest, read).await.unwrap();
        assert_eq!(observed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn background_waiters_do_not_take_reserved_foreground_capacity_and_cancel_releases() {
        let admission = std::sync::Arc::new(Admission::default());
        let a = admission
            .read(Bucket::Graphql, context(ReadClass::Background))
            .await
            .unwrap();
        let b = admission
            .read(Bucket::Graphql, context(ReadClass::Advisory))
            .await
            .unwrap();
        let cloned = admission.clone();
        let waiting = tokio::spawn(async move {
            let _held = cloned
                .read(Bucket::Rest, context(ReadClass::Background))
                .await;
        });
        tokio::task::yield_now().await;
        let c = admission
            .read(Bucket::Graphql, context(ReadClass::Foreground))
            .await
            .unwrap();
        let d = admission
            .read(Bucket::Rest, context(ReadClass::Foreground))
            .await
            .unwrap();
        assert_eq!(admission.total.available_permits(), 0);
        assert!(!waiting.is_finished());
        waiting.abort();
        let _ = waiting.await;
        drop((a, b, c, d));
        assert_eq!(admission.total.available_permits(), 4);
        assert_eq!(admission.background.available_permits(), 2);
    }
    #[tokio::test(start_paused = true)]
    async fn advisory_cycle_is_shared_but_does_not_limit_foreground() {
        let admission = Admission::default();
        for _ in 0..8 {
            admission
                .read(Bucket::Graphql, context(ReadClass::Advisory))
                .await
                .unwrap();
        }
        assert!(admission
            .read(Bucket::Rest, context(ReadClass::Advisory))
            .await
            .is_err());
        admission
            .read(Bucket::Rest, context(ReadClass::Foreground))
            .await
            .unwrap();
        tokio::time::advance(Duration::from_secs(30)).await;
        admission
            .read(Bucket::Rest, context(ReadClass::Advisory))
            .await
            .unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn cooldown_recovery_has_one_probe_and_failed_probe_backs_off() {
        let admission = Admission::default();
        admission.limit(Bucket::Graphql, 5, true);
        assert!(admission.write(Bucket::Rest).is_err());
        tokio::time::advance(Duration::from_secs(5)).await;
        let probe = admission
            .read(Bucket::Graphql, context(ReadClass::Foreground))
            .await
            .unwrap();
        assert!(admission
            .read(Bucket::Rest, context(ReadClass::Foreground))
            .await
            .is_err());
        drop(probe);
        assert!(admission
            .read(Bucket::Graphql, context(ReadClass::Foreground))
            .await
            .is_err());
        // A failed shared probe must also defer the other protocol.
        assert!(admission
            .read(Bucket::Rest, context(ReadClass::Foreground))
            .await
            .is_err());
        tokio::time::advance(Duration::from_secs(1)).await;
        let mut probe = admission
            .read(Bucket::Graphql, context(ReadClass::Foreground))
            .await
            .unwrap();
        probe.complete();
        drop(probe);
        let a = admission
            .read(Bucket::Graphql, context(ReadClass::Foreground))
            .await
            .unwrap();
        let b = admission
            .read(Bucket::Rest, context(ReadClass::Foreground))
            .await
            .unwrap();
        drop((a, b));
    }
    #[tokio::test(start_paused = true)]
    async fn rest_unknown_reset_recovers_without_a_global_counter_or_restart() {
        let admission = Admission::default();
        admission.observe(Bucket::Rest, Some(500), None);
        assert!(!admission.rest_available());
        tokio::time::advance(Duration::from_secs(60)).await;
        let mut probe = admission
            .read(Bucket::Rest, context(ReadClass::Foreground))
            .await
            .unwrap();
        admission.observe(Bucket::Rest, Some(5000), None);
        probe.complete();
        drop(probe);
        assert!(admission.rest_available());
        assert_eq!(admission.remaining(Bucket::Rest), Some(5000));
    }
    #[tokio::test(start_paused = true)]
    async fn older_unknown_window_success_cannot_clear_a_newer_cooldown() {
        let admission = Admission::default();
        admission.limit(Bucket::Graphql, 1, false);
        tokio::time::advance(Duration::from_secs(1)).await;
        let mut old_probe = admission
            .read(Bucket::Graphql, context(ReadClass::Foreground))
            .await
            .unwrap();
        admission.limit(Bucket::Graphql, 300, true);
        admission.observe(Bucket::Graphql, Some(5000), None);
        old_probe.complete();
        drop(old_probe);
        assert!(admission.write(Bucket::Graphql).is_err());
        assert!(admission.write(Bucket::Rest).is_err());
        tokio::time::advance(Duration::from_secs(299)).await;
        assert!(admission.write(Bucket::Graphql).is_err());
    }
    #[tokio::test(start_paused = true)]
    async fn queued_deadline_does_not_spend_attempts_or_leak_capacity() {
        let admission = Admission::default();
        let a = admission
            .read(Bucket::Graphql, context(ReadClass::Background))
            .await
            .unwrap();
        let b = admission
            .read(Bucket::Graphql, context(ReadClass::Background))
            .await
            .unwrap();
        let short = ReadContext::new(ReadClass::Advisory, Duration::from_secs(1));
        assert!(admission.read(Bucket::Graphql, short).await.is_err());
        assert_eq!(admission.state.lock().unwrap().spent, 0);
        drop((a, b));
        assert_eq!(admission.total.available_permits(), 4);
        assert_eq!(admission.background.available_permits(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_graphql_reset_expires_without_latching_the_account_budget() {
        let admission = Admission::default();
        admission.observe(Bucket::Graphql, Some(100), None);
        assert_eq!(admission.remaining(Bucket::Graphql), Some(100));
        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(admission.remaining(Bucket::Graphql), None);
        admission.observe(Bucket::Graphql, Some(4000), None);
        assert_eq!(admission.remaining(Bucket::Graphql), Some(4000));
    }

    #[tokio::test(start_paused = true)]
    async fn provider_deadlines_are_not_shortened_to_a_local_cap() {
        let admission = Admission::default();
        admission.limit(Bucket::Rest, 172800, true);
        tokio::time::advance(Duration::from_secs(86400)).await;
        assert!(admission.write(Bucket::Rest).is_err());
        assert!(admission.write(Bucket::Graphql).is_err());
    }

    #[test]
    fn quota_observations_are_window_ordered_and_client_local() {
        let admission = Admission::default();
        let reset = chrono::Utc::now().timestamp() as u64 + 3600;
        admission.observe(Bucket::Rest, Some(1000), Some(reset));
        admission.observe(Bucket::Rest, Some(10), Some(reset - 3600));
        assert_eq!(admission.remaining(Bucket::Rest), Some(1000));
        admission.observe(Bucket::Rest, Some(800), Some(reset));
        admission.observe(Bucket::Rest, Some(900), Some(reset));
        assert_eq!(admission.remaining(Bucket::Rest), Some(800));
        assert_eq!(Admission::default().remaining(Bucket::Rest), None);
    }
    #[test]
    fn numeric_and_http_date_retry_headers_do_not_invent_missing_evidence() {
        let now = chrono::DateTime::parse_from_rfc2822("Wed, 01 Oct 2025 10:00:00 GMT")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(retry_seconds("300", now), Some(300));
        assert_eq!(
            retry_seconds("Wed, 01 Oct 2025 10:05:00 GMT", now),
            Some(300)
        );
        assert_eq!(retry_seconds("invalid", now), None);
    }
}
