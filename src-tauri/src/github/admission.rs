//! Account-owned admission. Guards hold no mutex across network awaits.
use super::client::ClientError;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Notify, Semaphore, SemaphorePermit},
    time::Instant,
};

pub const READ_LIMIT: usize = 4;
const RECOVERY_PENDING: &str = "provider recovery probe is in progress";
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
    pub(crate) advisory_context: Option<crate::remote::context::DispatchContext>,
    pub(super) live: Option<tokio::sync::watch::Receiver<Option<LiveRead>>>,
}
impl ReadContext {
    pub fn new(class: ReadClass, budget: Duration) -> Self {
        Self {
            class,
            deadline: Instant::now() + budget,
            attempts: None,
            first_attempt: None,
            advisory_context: None,
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
    observed_at: Option<Instant>,
}
/// Read-only account admission evidence captured under one lock. No identity or inputs.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub remaining: Option<u64>,
    pub reset_unix_secs: Option<u64>,
    pub evidence_age_ms: Option<u64>,
    pub primary_cooldown_ms: u64,
    pub reserve_cooldown_ms: u64,
    pub recovery_probe: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionSnapshot {
    pub captured_at_unix_ms: u64,
    pub graphql: QuotaSnapshot,
    pub rest: QuotaSnapshot,
    pub secondary_cooldown_ms: u64,
}
fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
const ADVISORY_PARTICIPANTS: usize = 32;
const ADVISORY_INTEREST: Duration = Duration::from_secs(100);
#[derive(Debug)]
struct Participant {
    context: crate::remote::context::DispatchContext,
    demanded_at: Instant,
    allocated: u8,
    spent: u8,
}
#[derive(Debug, Default)]
struct AdvisoryShares {
    // Vector head is the remainder-allocation cursor. Rotation preserves
    // insertion order; removal naturally skips absent/revoked participants.
    participants: Vec<Participant>,
    initialized: bool,
}
impl AdvisoryShares {
    fn demand(
        &mut self,
        context: &crate::remote::context::DispatchContext,
        now: Instant,
        new_cycle: bool,
    ) -> Result<(), ClientError> {
        let principal = context.principal();
        #[cfg(feature = "enterprise-harness")]
        crate::enterprise_harness::metrics::record(principal, "demand", "advisory-share", 0);
        if new_cycle {
            self.participants
                .retain(|p| now.duration_since(p.demanded_at) < ADVISORY_INTEREST);
        }
        if let Some(participant) = self
            .participants
            .iter_mut()
            .find(|p| p.context.principal() == principal)
        {
            participant.demanded_at = now;
        } else if self.participants.len() < ADVISORY_PARTICIPANTS {
            self.participants.push(Participant {
                context: context.clone(),
                demanded_at: now,
                allocated: 0,
                spent: 0,
            });
        }
        // Even a refused newcomer can be the first request at a boundary.
        // Allocate existing participants before returning the capacity error.
        if new_cycle || !self.initialized {
            #[cfg(feature = "enterprise-harness")]
            crate::enterprise_harness::metrics::record(0, "reset", "advisory-cycle", 0);
            let count = self.participants.len();
            let base = 8 / count;
            let remainder = 8 % count;
            for (index, participant) in self.participants.iter_mut().enumerate() {
                participant.allocated = (base + usize::from(index < remainder)) as u8;
                participant.spent = 0;
                #[cfg(feature = "enterprise-harness")]
                crate::enterprise_harness::metrics::record(
                    participant.context.principal(),
                    "allocated",
                    "advisory-share",
                    u64::from(participant.allocated),
                );
            }
            self.participants.rotate_left(remainder);
            self.initialized = true;
        }
        let participant = self
            .participants
            .iter()
            .find(|p| p.context.principal() == principal)
            .ok_or_else(|| refused("advisory consumer capacity is full"))?;
        if participant.spent >= participant.allocated {
            return Err(refused("advisory consumer share is spent for this cycle"));
        }
        Ok(())
    }
    fn debit(&mut self, principal: u64) {
        self.participants
            .iter_mut()
            .find(|p| p.context.principal() == principal)
            .expect("share checked before atomic debit")
            .spent += 1;
        #[cfg(feature = "enterprise-harness")]
        crate::enterprise_harness::metrics::record(principal, "debit", "advisory-share", 1);
    }
}

#[derive(Debug)]
struct State {
    quotas: [Quota; 2],
    secondary: Option<Instant>,
    secondary_revision: u64,
    cycle: Instant,
    spent: u8,
    shares: AdvisoryShares,
}
#[derive(Debug)]
pub(super) struct Admission {
    total: Semaphore,
    background: Semaphore,
    recovery_changed: Notify,
    state: Mutex<State>,
}
impl Default for Admission {
    fn default() -> Self {
        Self {
            total: Semaphore::new(READ_LIMIT),
            background: Semaphore::new(2),
            recovery_changed: Notify::new(),
            state: Mutex::new(State {
                quotas: Default::default(),
                secondary: None,
                secondary_revision: 0,
                cycle: Instant::now(),
                spent: 0,
                shares: AdvisoryShares::default(),
            }),
        }
    }
}
fn refused(message: &str) -> ClientError {
    ClientError::NotDispatched(message.into())
}
pub(super) struct Attempt<'a> {
    pub(super) class: Option<ReadClass>,
    admission: &'a Admission,
    bucket: Bucket,
    probe: Option<(u64, Option<u64>, bool)>,
    done: bool,
    pub(super) deadline: Option<Instant>,
    _total: Option<SemaphorePermit<'a>>,
    _background: Option<SemaphorePermit<'a>>,
    #[cfg(feature = "enterprise-harness")]
    metric: crate::enterprise_harness::metrics::Scope,
}
impl Attempt<'_> {
    pub(super) fn is_probe(&self) -> bool {
        self.probe.is_some()
    }
    pub fn complete(&mut self) {
        self.done = true;
    }
}
impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        #[cfg(feature = "enterprise-harness")]
        self.metric.finish("released");
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
            drop(state);
            self.admission.recovery_changed.notify_waiters();
        }
    }
}
impl Admission {
    fn enter(
        &self,
        bucket: Bucket,
        class: Option<ReadClass>,
        allowance: Option<&AttemptAllowance>,
        caller: Option<&crate::remote::context::DispatchContext>,
    ) -> Result<Attempt<'_>, ClientError> {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let desktop = crate::remote::context::DispatchContext::desktop();
        let caller = caller.unwrap_or(&desktop);
        // Registry-before-capability lock order, held only through accounting.
        // A revoked paired incarnation cannot renew demand or admit an attempt.
        let _capability = if class.is_some() {
            // Drop each liveness snapshot before acquiring the next guard.
            // Different immutable account instances may share capabilities;
            // holding two reader guards could deadlock behind queued revokers.
            state
                .shares
                .participants
                .retain(|p| p.context.guard().is_ok());
            caller
                .guard()
                .map_err(|_| refused("advisory consumer pairing has been retired"))?
        } else {
            None
        };
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
            return Err(refused(RECOVERY_PENDING));
        }
        // A secondary recovery is serialized across BOTH buckets.
        if secondary_recovery && state.quotas.iter().any(|q| q.probing) {
            return Err(refused(RECOVERY_PENDING));
        }
        if class == Some(ReadClass::Advisory) {
            let new_cycle = now.duration_since(state.cycle) >= Duration::from_secs(30);
            if new_cycle {
                state.cycle = now;
                state.spent = 0;
            }
            // Refused late demand is remembered before checking spent credit.
            // New callers cannot mint shares inside an already allocated cycle.
            state.shares.demand(caller, now, new_cycle)?;
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
            state.shares.debit(caller.principal());
        }
        Ok(Attempt {
            class,
            admission: self,
            bucket,
            probe,
            done: false,
            deadline: None,
            _total: None,
            _background: None,
            #[cfg(feature = "enterprise-harness")]
            metric: crate::enterprise_harness::metrics::Scope::new(
                "admitted",
                match class {
                    Some(ReadClass::Foreground) => 0,
                    Some(ReadClass::Background) => 1,
                    Some(ReadClass::Advisory) => 2,
                    None => 3,
                },
            ),
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
                loop {
                    // Register before inspecting admission so a finishing probe
                    // cannot be missed between refusal and awaiting notification.
                    let changed = self.recovery_changed.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
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
                    let entered = self.enter(
                        bucket,
                        Some(demand.class),
                        context.attempts.as_ref(),
                        context.advisory_context.as_ref(),
                    );
                    let mut attempt = match entered {
                        Err(ClientError::NotDispatched(ref message))
                            if demand.class == ReadClass::Foreground
                                && message == RECOVERY_PENDING =>
                        {
                            // Keep only demand, not scarce read capacity. The outer
                            // deadline/live-demand select still bounds this wait.
                            drop(total);
                            drop(background);
                            changed.await;
                            continue;
                        }
                        result => result,
                    }
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
                    return Ok(attempt);
                }
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
        self.enter(bucket, None, None, None)
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
    pub fn snapshot(&self) -> AdmissionSnapshot {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let cooldown = |until: Option<Instant>| {
            until.map_or(0, |until| millis(until.saturating_duration_since(now)))
        };
        let quota = |q: &Quota| QuotaSnapshot {
            remaining: if q.reset.is_some_and(|at| seconds_until(at) == 0)
                || q.unknown_until.is_some_and(|at| at <= now)
            {
                None
            } else {
                q.remaining
            },
            reset_unix_secs: q.reset,
            evidence_age_ms: q
                .observed_at
                .map(|at| millis(now.saturating_duration_since(at))),
            primary_cooldown_ms: cooldown(q.blocked),
            reserve_cooldown_ms: cooldown(q.reserve_until),
            recovery_probe: q.probing,
        };
        AdmissionSnapshot {
            captured_at_unix_ms: chrono::Utc::now().timestamp_millis().max(0) as u64,
            graphql: quota(&state.quotas[0]),
            rest: quota(&state.quotas[1]),
            secondary_cooldown_ms: cooldown(state.secondary),
        }
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
            quota.observed_at = None;
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
            // A higher out-of-order same-window value does not refresh the
            // age of the lower, retained constraint.
            if quota.remaining.is_none_or(|old| remaining <= old) {
                quota.observed_at = Some(Instant::now());
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
    fn principal_read(caller: &crate::remote::context::DispatchContext) -> ReadContext {
        let mut read = context(ReadClass::Advisory);
        read.advisory_context = Some(caller.clone());
        read
    }
    async fn spend_share(
        admission: &Admission,
        caller: &crate::remote::context::DispatchContext,
    ) -> usize {
        let mut admitted = 0;
        for _ in 0..9 {
            if admission
                .read(Bucket::Rest, principal_read(caller))
                .await
                .is_ok()
            {
                admitted += 1;
            }
        }
        admitted
    }
    #[test]
    fn two_account_ledgers_release_capability_snapshots_before_retirement_fences() {
        use crate::remote::context::DispatchContext;
        let a = DispatchContext::paired_for_test();
        let b = DispatchContext::paired_for_test();
        let ledgers = [Admission::default(), Admission::default()];
        for ledger in &ledgers {
            for caller in [&a, &b] {
                drop(ledger.enter(Bucket::Rest, Some(ReadClass::Advisory), None, Some(caller)));
            }
        }
        let held_a = a.guard().unwrap();
        let held_b = b.guard().unwrap();
        let (started, waiting) = std::sync::mpsc::channel();
        let (done, finished) = std::sync::mpsc::channel();
        for caller in [a.clone(), b.clone()] {
            let started = started.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                started.send(()).unwrap();
                caller.retire_for_test();
                done.send(()).unwrap();
            });
        }
        for _ in 0..2 {
            waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        for (ledger, caller) in ledgers.into_iter().zip([a.clone(), b.clone()]) {
            let done = done.clone();
            std::thread::spawn(move || {
                // A race may enter before retirement; after the writer completes,
                // the same incarnation must always be refused.
                drop(ledger.enter(Bucket::Rest, Some(ReadClass::Advisory), None, Some(&caller)));
                done.send(()).unwrap();
            });
        }
        drop(held_a);
        drop(held_b);
        for _ in 0..4 {
            finished
                .recv_timeout(Duration::from_secs(2))
                .expect("capability pruning and concurrent retirement must finish");
        }
        assert!(a.guard().is_err());
        assert!(b.guard().is_err());
        let fresh = Admission::default();
        for caller in [&a, &b] {
            assert!(fresh
                .enter(Bucket::Rest, Some(ReadClass::Advisory), None, Some(caller))
                .is_err());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn principal_share_capacity_refusal_cannot_poison_a_new_cycle() {
        use crate::remote::context::DispatchContext;
        let admission = Admission::default();
        let callers: Vec<_> = (0..33)
            .map(|_| DispatchContext::paired_for_test())
            .collect();
        for caller in &callers[..32] {
            let _ = admission.read(Bucket::Rest, principal_read(caller)).await;
        }
        assert_eq!(
            admission.state.lock().unwrap().shares.participants.len(),
            32
        );
        tokio::time::advance(Duration::from_secs(31)).await;
        assert!(admission
            .read(Bucket::Rest, principal_read(&callers[32]))
            .await
            .is_err());
        let mut total = 0;
        for caller in &callers[..32] {
            total += spend_share(&admission, caller).await;
        }
        assert_eq!(
            total, 8,
            "33rd caller refusal must not prevent existing participants' boundary allocation"
        );
        assert_eq!(
            admission.state.lock().unwrap().shares.participants.len(),
            32
        );
    }
    #[tokio::test(start_paused = true)]
    async fn principal_share_allocations_rotate_for_one_through_thirty_two_consumers() {
        use crate::remote::context::DispatchContext;
        for n in [1, 2, 3, 8, 9, 32] {
            let admission = Admission::default();
            let callers: Vec<_> = (0..n).map(|_| DispatchContext::paired_for_test()).collect();
            for caller in &callers {
                let _ = admission.read(Bucket::Rest, principal_read(caller)).await;
            }
            let mut totals = vec![0; n];
            for _ in 0..n {
                tokio::time::advance(Duration::from_secs(31)).await;
                let mut cycle = 0;
                for (i, caller) in callers.iter().enumerate() {
                    let count = spend_share(&admission, caller).await;
                    assert!(count >= 8 / n && count <= (8 / n) + usize::from(8 % n != 0));
                    cycle += count;
                    totals[i] += count;
                }
                assert_eq!(cycle, 8);
                assert_eq!(admission.state.lock().unwrap().spent, 8);
            }
            assert!(
                totals.iter().all(|count| *count == 8),
                "stable remainder rotation for {n} participants: {totals:?}"
            );
        }
    }
    #[tokio::test(start_paused = true)]
    async fn principal_interest_covers_cached_sixty_plus_thirty_but_abandoned_credit_expires() {
        use crate::remote::context::DispatchContext;
        let admission = Admission::default();
        let a = DispatchContext::desktop();
        let b = DispatchContext::paired_for_test();
        assert_eq!(spend_share(&admission, &a).await, 8);
        assert_eq!(spend_share(&admission, &b).await, 0);
        for _ in 0..3 {
            tokio::time::advance(Duration::from_secs(30)).await;
            assert_eq!(spend_share(&admission, &a).await, 4);
        }
        assert_eq!(
            admission.state.lock().unwrap().shares.participants.len(),
            2,
            "cached peer survives ordinary60s plus next30s window"
        );
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(
            spend_share(&admission, &a).await,
            8,
            "last absent demand at0 expires at100, allocation catches up at120 <130"
        );
        assert_eq!(admission.state.lock().unwrap().shares.participants.len(), 1);
        assert_eq!(
            spend_share(&admission, &b).await,
            0,
            "return cannot mint credit mid-cycle"
        );
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(spend_share(&admission, &a).await, 4);
        assert_eq!(spend_share(&admission, &b).await, 4);
    }
    #[tokio::test(start_paused = true)]
    async fn revoked_principal_cannot_renew_or_admit_and_unused_credit_is_not_borrowed() {
        use crate::remote::context::DispatchContext;
        let admission = Admission::default();
        let a = DispatchContext::desktop();
        let b = DispatchContext::paired_for_test();
        let _ = admission.read(Bucket::Rest, principal_read(&a)).await;
        let _ = admission.read(Bucket::Rest, principal_read(&b)).await;
        tokio::time::advance(Duration::from_secs(31)).await;
        assert!(admission
            .read(Bucket::Rest, principal_read(&a))
            .await
            .is_ok());
        b.retire_for_test();
        assert_eq!(spend_share(&admission, &b).await, 0);
        assert_eq!(
            spend_share(&admission, &a).await,
            3,
            "revoked peer's current frozen credit remains unused"
        );
        assert_eq!(admission.state.lock().unwrap().shares.participants.len(), 1);
        for _ in 0..8 {
            assert!(admission
                .read(Bucket::Rest, context(ReadClass::Foreground))
                .await
                .is_ok());
        }
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(spend_share(&admission, &a).await, 8);
        let repaired = DispatchContext::paired_for_test();
        assert_ne!(repaired.principal(), b.principal());
        assert_eq!(spend_share(&admission, &repaired).await, 0);
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(spend_share(&admission, &a).await, 4);
        assert_eq!(spend_share(&admission, &repaired).await, 4);
        assert_eq!(
            spend_share(&Admission::default(), &repaired).await,
            8,
            "new immutable account has no prior sharing state"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn remembered_late_principal_receives_four_attempts_despite_early_competitor() {
        use crate::remote::context::DispatchContext;
        let admission = Admission::default();
        let desktop = DispatchContext::desktop();
        let paired = DispatchContext::paired_for_test();
        let read = |caller: &DispatchContext| {
            let mut context = context(ReadClass::Advisory);
            context.advisory_context = Some(caller.clone());
            context
        };
        for _ in 0..8 {
            assert!(admission.read(Bucket::Rest, read(&desktop)).await.is_ok());
        }
        assert!(admission
            .read(Bucket::Graphql, read(&paired))
            .await
            .is_err());
        tokio::time::advance(Duration::from_secs(31)).await;
        for _ in 0..4 {
            assert!(admission.read(Bucket::Rest, read(&desktop)).await.is_ok());
        }
        assert!(
            admission.read(Bucket::Rest, read(&desktop)).await.is_err(),
            "early caller cannot consume a remembered late caller's four credits"
        );
        for _ in 0..4 {
            assert!(admission.read(Bucket::Graphql, read(&paired)).await.is_ok());
        }
        assert!(admission
            .read(Bucket::Graphql, read(&paired))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn selected_foreground_cannot_dispatch_for_a_retired_pairing() {
        let admission = Admission::default();
        let caller = crate::remote::context::DispatchContext::paired_for_test();
        let mut selected = context(ReadClass::Foreground);
        selected.advisory_context = Some(caller.clone());
        caller.retire_for_test();
        assert!(admission.read(Bucket::Graphql, selected).await.is_err());
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
    async fn foreground_waits_for_recovery_without_holding_capacity() {
        let admission = Admission::default();
        admission.limit(Bucket::Graphql, 5, true);
        tokio::time::advance(Duration::from_secs(5)).await;
        let mut probe = admission
            .read(Bucket::Graphql, context(ReadClass::Background))
            .await
            .unwrap();
        let waiting = admission.read(Bucket::Rest, context(ReadClass::Foreground));
        tokio::pin!(waiting);
        tokio::select! {
            biased;
            _ = &mut waiting => panic!("foreground must wait for the existing recovery probe"),
            _ = tokio::task::yield_now() => {},
        }
        assert_eq!(admission.total.available_permits(), READ_LIMIT - 1);
        assert_eq!(admission.background.available_permits(), 1);
        assert!(admission
            .read(Bucket::Rest, context(ReadClass::Advisory))
            .await
            .is_err());
        probe.complete();
        drop(probe);
        let resumed = waiting
            .await
            .expect("successful probe resumes foreground without a UI retry");
        drop(resumed);
        assert_eq!(admission.total.available_permits(), READ_LIMIT);
    }

    #[tokio::test(start_paused = true)]
    async fn foreground_probe_wait_keeps_newer_cooldowns_and_failure_backoff() {
        for newer_limit in [false, true] {
            let admission = Admission::default();
            admission.limit(Bucket::Graphql, 1, true);
            tokio::time::advance(Duration::from_secs(1)).await;
            let mut probe = admission
                .read(Bucket::Graphql, context(ReadClass::Background))
                .await
                .unwrap();
            let waiting = admission.read(Bucket::Rest, context(ReadClass::Foreground));
            tokio::pin!(waiting);
            tokio::select! {
                biased;
                _ = &mut waiting => panic!("must remain queued during recovery"),
                _ = tokio::task::yield_now() => {},
            }
            if newer_limit {
                admission.limit(Bucket::Rest, 30, true);
                probe.complete();
            }
            drop(probe);
            assert!(matches!(waiting.await, Err(ClientError::RateLimited(_))));
            assert_eq!(admission.total.available_permits(), READ_LIMIT);
            assert_eq!(
                admission.retry_wait(Bucket::Rest),
                if newer_limit { 30 } else { 1 }
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn foreground_probe_wait_deadline_and_cancellation_release_all_capacity() {
        let admission = Admission::default();
        admission.limit(Bucket::Graphql, 1, true);
        tokio::time::advance(Duration::from_secs(1)).await;
        let mut probe = admission
            .read(Bucket::Graphql, context(ReadClass::Background))
            .await
            .unwrap();
        let mut waiting = Box::pin(admission.read(
            Bucket::Rest,
            ReadContext::new(ReadClass::Foreground, Duration::from_secs(2)),
        ));
        tokio::select! {
            biased;
            _ = &mut waiting => panic!("must remain queued during recovery"),
            _ = tokio::task::yield_now() => {},
        }
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(matches!(waiting.await, Err(ClientError::NotDispatched(_))));
        let mut cancelled = Box::pin(admission.read(Bucket::Rest, context(ReadClass::Foreground)));
        tokio::select! {
            biased;
            _ = &mut cancelled => panic!("must remain queued during recovery"),
            _ = tokio::task::yield_now() => {},
        }
        drop(cancelled);
        assert_eq!(admission.total.available_permits(), READ_LIMIT - 1);
        probe.complete();
        drop(probe);
        assert!(admission
            .read(Bucket::Rest, context(ReadClass::Foreground))
            .await
            .is_ok());
        assert_eq!(admission.total.available_permits(), READ_LIMIT);
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
