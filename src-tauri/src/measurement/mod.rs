//! Bounded opt-in measurement core, shared by platform adapters. No provider or
//! database dependency. Admission never performs IO or waits for the writer.
mod export;
pub mod model;
mod writer;
pub use model::*;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub const MAX_RECORD: usize = 1024;
pub trait Clock: Send + Sync {
    fn now(&self) -> (u64, u64);
}
struct SystemClock(Instant);
impl Clock for SystemClock {
    fn now(&self) -> (u64, u64) {
        (
            self.0.elapsed().as_millis().min(u64::MAX as u128) as u64,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u64::MAX as u128) as u64,
        )
    }
}
#[derive(Clone)]
pub struct Config {
    pub directory: PathBuf,
    pub epoch: [u8; 16],
    pub role: Role,
    pub platform: Platform,
    pub build: String,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Caps {
    pub keys: usize,
    pub key_bytes: usize,
    pub data: usize,
    pub control: usize,
    pub operations: usize,
    pub aggregates: usize,
    pub routine_per_minute: usize,
    pub transitions_per_minute: usize,
    pub failures_per_minute: usize,
    pub segments: usize,
    pub segment_bytes: u64,
}
impl Default for Caps {
    fn default() -> Self {
        Self {
            keys: 4096,
            key_bytes: 1024 * 1024,
            data: 1024,
            control: 8,
            operations: 1024,
            aggregates: 128,
            routine_per_minute: 12,
            transitions_per_minute: 120,
            failures_per_minute: 60,
            segments: 8,
            segment_bytes: 16 * 1024 * 1024,
        }
    }
}
pub enum Key<'a> {
    Owner(&'a str),
    StatsScope {
        owner: &'a str,
        scope: &'a str,
        from_day: i32,
        to_day: i32,
    },
    Session(&'a str),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AggregateDelta {
    pub domain: Domain,
    pub metric: AggregateKind,
    pub work: WorkClass,
    pub count: u64,
}
type AggregateKey = (Domain, AggregateKind, WorkClass);
#[derive(Clone, Copy, PartialEq, Eq)]
enum HandleKind {
    Owner,
    Scope,
    Session,
    Operation,
    Receipt,
}
struct State {
    capture: u64,
    active: bool,
    next_id: u64,
    keys: HashMap<Vec<u8>, u64>,
    key_bytes: usize,
    live: HashMap<u64, HandleKind>,
    operations: HashMap<u64, Option<u64>>,
    seq: u64,
    dedup: VecDeque<Vec<u8>>,
    window: u64,
    transitions: usize,
    failures: usize,
    routine: usize,
    aggregates: HashMap<AggregateKey, (u64, u64)>,
    last_clock: (u64, u64),
}
impl State {
    fn new() -> Self {
        Self {
            capture: 0,
            active: false,
            next_id: 0,
            keys: HashMap::new(),
            key_bytes: 0,
            live: HashMap::new(),
            operations: HashMap::new(),
            seq: 0,
            dedup: VecDeque::new(),
            window: 0,
            transitions: 0,
            failures: 0,
            routine: 0,
            aggregates: HashMap::new(),
            last_clock: (0, 0),
        }
    }
}
struct Shared {
    enabled: AtomicBool,
    unavailable: AtomicBool,
    stopping: AtomicBool,
    exporting: AtomicBool,
    status: Mutex<JournalStatus>,
    loss: Mutex<Loss>,
    lifecycle: Mutex<Lifecycle>,
}
pub struct Recorder {
    config: Config,
    epoch: String,
    caps: Caps,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    shared: Arc<Shared>,
    data: SyncSender<Envelope>,
    control: SyncSender<writer::Control>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl Recorder {
    pub fn new(config: Config) -> Result<Self, ExportError> {
        Self::with_clock(
            config,
            Caps::default(),
            Arc::new(SystemClock(Instant::now())),
        )
    }
    fn with_clock(config: Config, caps: Caps, clock: Arc<dyn Clock>) -> Result<Self, ExportError> {
        if config.build.len() > 64
            || !config
                .build
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_+".contains(&b))
        {
            return Err(ExportError::Unavailable);
        }
        let epoch = config.epoch.iter().map(|b| format!("{b:02x}")).collect();
        let shared = Arc::new(Shared {
            enabled: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            exporting: AtomicBool::new(false),
            status: Mutex::new(JournalStatus::default()),
            loss: Mutex::new(Loss::default()),
            lifecycle: Mutex::new(Lifecycle::default()),
        });
        let (data, receiver) = mpsc::sync_channel(caps.data);
        let (control, controls) = mpsc::sync_channel(caps.control);
        let writer = writer::Writer::open(config.clone(), caps.clone(), shared.clone())?;
        let worker = std::thread::Builder::new()
            .name("measurement-writer".into())
            .spawn(move || writer.run(receiver, controls))
            .map_err(|_| ExportError::Unavailable)?;
        Ok(Self {
            config,
            epoch,
            caps,
            clock,
            state: Mutex::new(State::new()),
            shared,
            data,
            control,
            worker: Mutex::new(Some(worker)),
        })
    }
    pub fn enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Acquire)
            && !self.shared.unavailable.load(Ordering::Acquire)
    }
    pub fn set_enabled(&self, on: bool) {
        if !on {
            self.shared.enabled.store(false, Ordering::Release)
        }
        let mut s = self.state.lock().unwrap();
        if on && !s.active {
            s.capture = s.capture.saturating_add(1);
            s.keys.clear();
            s.key_bytes = 0;
            s.live.clear();
            s.operations.clear();
            s.dedup.clear();
            // Unflushed aggregates cannot leak into another capture.
            self.shared.loss.lock().unwrap().dropped += s.aggregates.len() as u64;
            s.aggregates.clear();
            s.transitions = 0;
            s.failures = 0;
            s.routine = 0;
            s.window = self.clock.now().0 / 60000;
        }
        if on != s.active {
            let mut lifecycle = self.shared.lifecycle.lock().unwrap();
            if on {
                lifecycle.captures_opened = lifecycle.captures_opened.saturating_add(1);
                lifecycle.active_capture = Some(s.capture);
            } else {
                lifecycle.captures_closed = lifecycle.captures_closed.saturating_add(1);
                lifecycle.active_capture = None;
            }
        }
        s.active = on;
        if !on {
            s.keys.clear();
            s.key_bytes = 0;
            s.live.clear();
            s.operations.clear();
            s.dedup.clear();
        }
        self.shared.enabled.store(on, Ordering::Release);
        // Wakeups are best effort; admission closure never depends on capacity.
        let _ = self
            .control
            .try_send(writer::Control::Flush { cutoff: s.seq });
    }
    fn id(&self, s: &mut State, kind: HandleKind) -> OpaqueId {
        s.next_id = s
            .next_id
            .checked_add(1)
            .expect("measurement identity exhausted");
        let id = OpaqueId {
            epoch: self.epoch.clone(),
            capture: s.capture,
            id: s.next_id,
        };
        s.live.insert(id.id, kind);
        id
    }
    fn valid(&self, s: &State, id: &OpaqueId) -> bool {
        id.epoch == self.epoch && id.capture == s.capture && s.live.contains_key(&id.id)
    }
    pub fn intern(&self, key: Key<'_>) -> Option<OpaqueId> {
        if !self.enabled() {
            return None;
        }
        let bytes = match &key {
            Key::Owner(v) | Key::Session(v) => v.len(),
            Key::StatsScope { owner, scope, .. } => owner.len().saturating_add(scope.len()),
        };
        if bytes > self.caps.key_bytes {
            self.shared.loss.lock().unwrap().cardinality += 1;
            return None;
        }
        let (kind, parent) = match &key {
            Key::Owner(_) => (HandleKind::Owner, None),
            Key::Session(_) => (HandleKind::Session, None),
            Key::StatsScope { owner, .. } => {
                let key = serde_json::to_vec(&(0, owner)).ok()?;
                let s = self.state.lock().unwrap();
                let parent = *s.keys.get(&key)?;
                if !s.live.contains_key(&parent) {
                    return None;
                }
                (HandleKind::Scope, Some(parent))
            }
        };
        let key = match key {
            Key::Owner(v) => serde_json::to_vec(&(0, v)),
            Key::Session(v) => serde_json::to_vec(&(1, v)),
            Key::StatsScope {
                owner,
                scope,
                from_day,
                to_day,
            } => serde_json::to_vec(&(2, owner, scope, from_day, to_day)),
        }
        .ok()?;
        let mut s = self.state.lock().unwrap();
        if !self.enabled() {
            return None;
        }
        if let Some(&id) = s.keys.get(&key) {
            if s.live.contains_key(&id) {
                return Some(OpaqueId {
                    epoch: self.epoch.clone(),
                    capture: s.capture,
                    id,
                });
            }
            return None;
        }
        if s.keys.len() >= self.caps.keys
            || s.key_bytes.saturating_add(key.len()) > self.caps.key_bytes
        {
            self.shared.loss.lock().unwrap().cardinality += 1;
            return None;
        }
        if parent.is_some_and(|id| !s.live.contains_key(&id)) {
            self.shared.loss.lock().unwrap().stale_handle += 1;
            return None;
        }
        if parent.is_some() && s.operations.len() >= self.caps.operations {
            self.shared.loss.lock().unwrap().cardinality += 1;
            return None;
        }
        let id = self.id(&mut s, kind);
        if let Some(parent) = parent {
            s.operations.insert(id.id, Some(parent));
        }
        s.key_bytes += key.len();
        s.keys.insert(key, id.id);
        Some(id)
    }
    /// Parent must remain live. Retiring an owner invalidates its operation tree.
    pub fn next_operation(&self, parent: Option<&OpaqueId>) -> Option<OpaqueId> {
        if !self.enabled() {
            return None;
        }
        let mut s = self.state.lock().unwrap();
        if !self.enabled() {
            return None;
        }
        if parent.is_some_and(|p| !self.valid(&s, p)) {
            self.shared.loss.lock().unwrap().stale_handle += 1;
            return None;
        }
        if s.operations.len() >= self.caps.operations {
            self.shared.loss.lock().unwrap().cardinality += 1;
            return None;
        }
        let id = self.id(&mut s, HandleKind::Operation);
        s.operations.insert(id.id, parent.map(|p| p.id));
        Some(id)
    }
    /// Capture-qualified receipt reference for one actual accepted publication.
    /// Producer retires the previous reference when those rows are replaced.
    pub fn receipt_reference(&self, owner: &OpaqueId) -> Option<OpaqueId> {
        let id = self.next_operation(Some(owner))?;
        let mut s = self.state.lock().unwrap();
        if !self.valid(&s, owner) || s.live.get(&owner.id) != Some(&HandleKind::Owner) {
            s.live.remove(&id.id);
            s.operations.remove(&id.id);
            return None;
        }
        s.live.insert(id.id, HandleKind::Receipt);
        Some(id)
    }
    fn typed_handles(&self, s: &State, event: &Event) -> bool {
        let is = |id: &OpaqueId, kind| self.valid(s, id) && s.live.get(&id.id) == Some(&kind);
        match event {
            Event::QueueReceipt {
                owner, operation, ..
            } => {
                is(owner, HandleKind::Owner)
                    && operation
                        .as_ref()
                        .is_none_or(|v| is(v, HandleKind::Operation))
            }
            Event::StatsProgress { scope, .. } => is(scope, HandleKind::Scope),
            Event::StopFailureMatch { session, .. } => is(session, HandleKind::Session),
            Event::Operation { operation, .. } | Event::Transcript { operation, .. } => {
                is(operation, HandleKind::Operation)
            }
            Event::Client { observation } => match observation {
                ClientMeasurement::MountedReview { receipt, scope, .. } => {
                    receipt.as_ref().is_none_or(|v| is(v, HandleKind::Receipt))
                        && scope.as_ref().is_none_or(|v| is(v, HandleKind::Scope))
                }
                ClientMeasurement::StatsView { scope, .. } => {
                    scope.as_ref().is_none_or(|v| is(v, HandleKind::Scope))
                }
                ClientMeasurement::TranscriptView { operation, .. } => operation
                    .as_ref()
                    .is_none_or(|v| is(v, HandleKind::Operation)),
            },
            Event::Aggregate { .. } => true,
        }
    }
    pub fn retire(&self, id: &OpaqueId) {
        let mut s = self.state.lock().unwrap();
        if !self.valid(&s, id) {
            return;
        }
        let mut pending = vec![id.id];
        while let Some(id) = pending.pop() {
            s.live.remove(&id);
            s.operations.remove(&id);
            pending.extend(
                s.operations
                    .iter()
                    .filter_map(|(&child, &parent)| (parent == Some(id)).then_some(child)),
            );
        }
    }
    pub fn record(&self, event: Event) -> bool {
        self.record_for(event, None)
    }
    fn record_for(&self, event: Event, expected_capture: Option<u64>) -> bool {
        if !self.enabled() {
            return false;
        }
        let (mono, wall) = self.clock.now();
        // Serialize outside the admission lock; conservative maximum envelope
        // reserves full-width counters before the accepted sequence is assigned.
        let preview = Envelope {
            schema: 1,
            epoch: self.epoch.clone(),
            capture: u64::MAX,
            seq: u64::MAX,
            monotonic_ms: u64::MAX,
            wall_time_ms: u64::MAX,
            role: self.config.role,
            event: event.clone(),
        };
        let encoded = serde_json::to_vec(&preview).unwrap_or_default();
        if encoded.len() + 1 > MAX_RECORD {
            self.refuse(event.domain(), true);
            return false;
        }
        let fingerprint = serde_json::to_vec(&event).unwrap_or_default();
        let mut s = self.state.lock().unwrap();
        if !self.enabled() {
            return false;
        }
        if expected_capture.is_some_and(|capture| capture != s.capture) {
            self.shared.loss.lock().unwrap().dropped += 1;
            return false;
        }
        if !self.typed_handles(&s, &event) || event.handles().iter().any(|h| !self.valid(&s, h)) {
            self.shared.loss.lock().unwrap().stale_handle += 1;
            return false;
        }
        if s.dedup.contains(&fingerprint) {
            self.shared.loss.lock().unwrap().coalesced += 1;
            return false;
        }
        if mono < s.last_clock.0 || wall < s.last_clock.1 {
            self.shared.loss.lock().unwrap().clock_anomaly += 1
        }
        s.last_clock = (mono.max(s.last_clock.0), wall);
        if mono / 60000 > s.window {
            s.window = mono / 60000;
            s.transitions = 0;
            s.failures = 0;
            s.routine = 0;
        }
        let budget = match event {
            Event::StopFailureMatch { .. } => (&mut s.failures, self.caps.failures_per_minute),
            Event::Aggregate { .. } => (&mut s.routine, self.caps.routine_per_minute),
            _ => (&mut s.transitions, self.caps.transitions_per_minute),
        };
        if *budget.0 >= budget.1 {
            let mut loss = self.shared.loss.lock().unwrap();
            loss.budget += 1;
            loss.by_domain[event.domain() as usize] += 1;
            return false;
        }
        *budget.0 += 1;
        let envelope = Envelope {
            schema: 1,
            epoch: self.epoch.clone(),
            capture: s.capture,
            seq: s.seq + 1,
            monotonic_ms: s.last_clock.0,
            wall_time_ms: wall,
            role: self.config.role,
            event,
        };
        match self.data.try_send(envelope) {
            Ok(()) => {
                s.seq += 1;
                if s.dedup.len() == 128 {
                    s.dedup.pop_front();
                }
                s.dedup.push_back(fingerprint);
                true
            }
            Err(_) => {
                self.refuse(preview.event.domain(), false);
                false
            }
        }
    }
    fn refuse(&self, domain: Domain, invalid: bool) {
        let mut l = self.shared.loss.lock().unwrap();
        if invalid {
            l.invalid += 1
        } else {
            l.dropped += 1
        }
        l.by_domain[domain as usize] += 1;
    }
    pub fn aggregate(&self, delta: AggregateDelta) {
        if !self.enabled() {
            return;
        }
        let now = self.clock.now().0;
        let mut s = self.state.lock().unwrap();
        if !self.enabled() {
            return;
        }
        let key = (delta.domain, delta.metric, delta.work);
        if !s.aggregates.contains_key(&key) && s.aggregates.len() >= self.caps.aggregates {
            self.shared.loss.lock().unwrap().cardinality += 1;
            return;
        }
        let entry = s.aggregates.entry(key).or_insert((0, now));
        let (old, start) = *entry;
        entry.0 = old.saturating_add(delta.count);
        if old.checked_add(delta.count).is_none() {
            self.shared.loss.lock().unwrap().overflow += 1
        }
        if now.saturating_sub(start) < 60000 {
            return;
        }
        let capture = s.capture;
        let (count, start) = s.aggregates.remove(&key).unwrap();
        drop(s);
        if !self.record_for(
            Event::Aggregate {
                domain: delta.domain,
                metric: delta.metric,
                work: delta.work,
                count,
                interval_ms: now.saturating_sub(start),
                coalesced: true,
            },
            Some(capture),
        ) {
            let mut s = self.state.lock().unwrap();
            if self.enabled() && s.capture == capture {
                let entry = s.aggregates.entry(key).or_insert((0, start));
                entry.0 = entry.0.saturating_add(count);
                entry.1 = entry.1.min(start);
            }
        }
        self.shared.loss.lock().unwrap().coalesced += 1;
    }
    /// Checked narrowing for measured populations. Overflow is unavailable,
    /// never a wrapped or saturated reading claiming an exact population.
    pub fn measured_count(&self, value: u64) -> Option<u32> {
        match u32::try_from(value) {
            Ok(value) => Some(value),
            Err(_) => {
                if self.enabled() {
                    self.shared.loss.lock().unwrap().overflow += 1;
                }
                None
            }
        }
    }
    pub fn client_events(&self, batch: Vec<ClientMeasurement>) -> Result<(), ExportError> {
        if !self.enabled() {
            return Ok(());
        }
        if batch.len() > 32 || serde_json::to_vec(&batch).map_or(true, |b| b.len() > 16 * 1024) {
            self.shared.loss.lock().unwrap().invalid += 1;
            return Err(ExportError::Unavailable);
        }
        for observation in batch {
            self.record(Event::Client { observation });
        }
        Ok(())
    }
    pub fn status(&self) -> JournalStatus {
        let mut out = self.shared.status.lock().unwrap().clone();
        let live = self.shared.loss.lock().unwrap().clone();
        out.enabled = self.enabled();
        out.writer_state = if self.shared.unavailable.load(Ordering::Acquire) {
            WriterState::Unavailable
        } else if out.enabled {
            WriterState::Ready
        } else {
            WriterState::Disabled
        };
        out.dropped = live.dropped;
        out.invalid = live.invalid;
        out.coalesced = live.coalesced;
        out.rotated_out = live.rotated_out;
        out.incomplete = live.incomplete();
        out.loss = live;
        out
    }
    pub async fn export_to(&self, destination: PathBuf) -> Result<ExportReceipt, ExportError> {
        if self.shared.unavailable.load(Ordering::Acquire) {
            return Err(ExportError::Unavailable);
        }
        self.shared
            .exporting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| ExportError::Busy)?;
        let (send, receive) = tokio::sync::oneshot::channel();
        let (ready, barrier) = tokio::sync::oneshot::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let state = self.state.lock().unwrap();
            let cutoff = state.seq;
            let loss = self.shared.loss.lock().unwrap().clone();
            let now = self.clock.now().0;
            let deferred = state
                .aggregates
                .iter()
                .map(
                    |(&(domain, metric, work), &(count, start))| DeferredAggregate {
                        domain,
                        metric,
                        work,
                        count,
                        elapsed_ms: now.saturating_sub(start),
                    },
                )
                .collect();
            let lifecycle = self.shared.lifecycle.lock().unwrap().clone();
            let command = writer::Control::Export {
                cutoff,
                loss: Box::new(loss),
                deferred,
                lifecycle,
                destination,
                reply: send,
                ready,
                cancel: cancel.clone(),
            };
            if self.control.try_send(command).is_err() {
                self.shared.exporting.store(false, Ordering::Release);
                return Err(ExportError::Busy);
            }
        }
        match tokio::time::timeout(std::time::Duration::from_secs(5), barrier).await {
            Ok(Ok(())) => receive.await.unwrap_or(Err(ExportError::Unavailable)),
            Ok(Err(_)) => receive.await.unwrap_or(Err(ExportError::Unavailable)),
            Err(_) => {
                cancel.store(true, Ordering::Release);
                Err(ExportError::Timeout)
            }
        }
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.shared.enabled.store(false, Ordering::Release);
        self.shared.stopping.store(true, Ordering::Release);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests;
