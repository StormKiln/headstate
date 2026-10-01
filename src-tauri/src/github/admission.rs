//! Account-owned admission. Guards hold no mutex across network awaits.
use super::client::ClientError;
use std::{sync::Mutex, time::Duration};
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
#[derive(Clone, Copy, Debug)]
pub struct ReadContext {
    pub class: ReadClass,
    pub deadline: Instant,
}
impl ReadContext {
    pub fn new(class: ReadClass, budget: Duration) -> Self {
        Self {
            class,
            deadline: Instant::now() + budget,
        }
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
    probe: Option<u64>,
    done: bool,
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
        if let Some(revision) = self.probe {
            let mut state = self
                .admission
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let quota = &mut state.quotas[self.bucket as usize];
            quota.probing = false;
            if quota.revision == revision {
                if self.done {
                    quota.blocked = None;
                    quota.reserve_until = None;
                    if state.secondary.is_some_and(|until| until <= Instant::now()) {
                        state.secondary = None;
                    }
                } else {
                    quota.blocked = Some(Instant::now() + Duration::from_secs(1));
                    if state.secondary.is_some() {
                        state.secondary = Some(Instant::now() + Duration::from_secs(1));
                    }
                }
            }
        }
    }
}
impl Admission {
    fn enter(&self, bucket: Bucket, class: Option<ReadClass>) -> Result<Attempt<'_>, ClientError> {
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
        let probe = if deadline.is_some() || secondary_recovery {
            let quota = &mut state.quotas[bucket as usize];
            quota.probing = true;
            Some(quota.revision)
        } else {
            None
        };
        if class == Some(ReadClass::Advisory) {
            if now.duration_since(state.cycle) >= Duration::from_secs(30) {
                state.cycle = now;
                state.spent = 0;
            }
            if state.spent >= 8 {
                if probe.is_some() {
                    state.quotas[bucket as usize].probing = false;
                }
                return Err(refused(
                    "advisory attempt allowance is spent for this cycle",
                ));
            }
            state.spent += 1;
        }
        Ok(Attempt {
            admission: self,
            bucket,
            probe,
            done: false,
            _total: None,
            _background: None,
        })
    }
    pub async fn read(
        &self,
        bucket: Bucket,
        context: ReadContext,
    ) -> Result<Attempt<'_>, ClientError> {
        let acquire = async {
            let background = if context.class != ReadClass::Foreground {
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
            if Instant::now() >= context.deadline {
                return Err(refused("read deadline elapsed before dispatch"));
            }
            let mut attempt =
                self.enter(bucket, Some(context.class))
                    .map_err(|error| match error {
                        ClientError::NotDispatched(message) if message.starts_with("provider") => {
                            ClientError::RateLimited(message)
                        }
                        other => other,
                    })?;
            attempt._background = background;
            attempt._total = Some(total);
            Ok(attempt)
        };
        tokio::time::timeout_at(context.deadline, acquire)
            .await
            .map_err(|_| refused("read deadline elapsed before dispatch"))?
    }
    pub fn write(&self, bucket: Bucket) -> Result<Attempt<'_>, ClientError> {
        self.enter(bucket, None)
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
        let quota = &mut state.quotas[bucket as usize];
        quota.blocked = Some(quota.blocked.map_or(until, |old| old.max(until)));
        quota.revision += 1;
        if secondary {
            state.secondary = Some(state.secondary.map_or(until, |old| old.max(until)));
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
    fn context(class: ReadClass) -> ReadContext {
        ReadContext::new(class, Duration::from_secs(30))
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
