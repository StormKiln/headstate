//! One retained owner per executable/host, with explicit per-operation context.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, LazyLock, Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Instant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Foreground,
    Background,
    Advisory,
}
struct State {
    touched: Instant,
    admitted: bool,
    generation: u64,
    viewer: Option<String>,
    blocked: Option<Instant>,
    probe: bool,
    revision: u64,
    reset: Option<u64>,
    cycle: Instant,
    advisory: usize,
}
pub struct Owner {
    state: Mutex<State>,
    total: Arc<Semaphore>,
    background: Arc<Semaphore>,
    pub identity: tokio::sync::Mutex<()>,
}
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
type Owners = HashMap<(PathBuf, String), Arc<Owner>>;
static OWNERS: LazyLock<Mutex<Owners>> = LazyLock::new(Mutex::default);
#[derive(Clone)]
pub struct Context {
    pub owner: Arc<Owner>,
    pub generation: u64,
    pub class: Class,
    pub deadline: Instant,
    pub allowance: Option<Arc<AtomicUsize>>,
}
/// Internal publication token. No credentials or wire representation.
#[derive(Clone)]
pub(crate) struct Token {
    owner: Arc<Owner>,
    generation: u64,
}
impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitLabSession")
            .field("generation", &self.generation)
            .finish()
    }
}
impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner) && self.generation == other.generation
    }
}
impl Eq for Token {}
impl Token {
    // Synchronous only: never hold this authority across process or async work.
    // Publication gate -> owner -> snapshot/state locks; transport only takes owner.
    pub(crate) fn with_current<T>(&self, work: impl FnOnce() -> T) -> Option<T> {
        let state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation != self.generation {
            return None;
        }
        Some(work())
    }
}
impl Context {
    pub(crate) fn with_token(&self, token: Token) -> Self {
        let mut next = self.clone();
        next.owner = token.owner;
        next.generation = token.generation;
        next
    }
    pub(crate) fn token(&self) -> Token {
        Token {
            owner: self.owner.clone(),
            generation: self.generation,
        }
    }
    pub fn new(program: &Path, host: &str, class: Class, deadline: Instant) -> Self {
        let mut owners = OWNERS.lock().unwrap_or_else(|e| e.into_inner());
        let key = (program.to_owned(), host.to_lowercase());
        if !owners.contains_key(&key) && owners.len() >= 512 {
            let victim = owners
                .iter()
                .filter_map(|(key, owner)| {
                    let state = owner.state.lock().unwrap_or_else(|e| e.into_inner());
                    (Arc::strong_count(owner) == 1
                        && state.blocked.is_none()
                        && !state.probe
                        && state.cycle.elapsed() >= Duration::from_secs(30))
                    .then(|| (key.clone(), state.touched))
                })
                .min_by_key(|(_, touched)| *touched)
                .map(|(key, _)| key);
            if let Some(victim) = victim {
                owners.remove(&victim);
            }
        }
        let owner = owners.get(&key).cloned().unwrap_or_else(|| {
            let admitted = owners.len() < 512;
            let owner = Arc::new(Owner {
                state: Mutex::new(State {
                    touched: Instant::now(),
                    admitted,
                    generation: NEXT_OWNER.fetch_add(1, Ordering::AcqRel),
                    viewer: None,
                    blocked: None,
                    probe: false,
                    revision: 0,
                    reset: None,
                    cycle: Instant::now(),
                    advisory: 0,
                }),
                total: Arc::new(Semaphore::new(4)),
                background: Arc::new(Semaphore::new(2)),
                identity: tokio::sync::Mutex::new(()),
            });
            if admitted {
                owners.insert(key, owner.clone());
            }
            owner
        });
        let generation = {
            let mut state = owner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.touched = Instant::now();
            state.generation
        };
        Self {
            owner,
            generation,
            class,
            deadline,
            allowance: None,
        }
    }
    pub fn current(&self) -> bool {
        self.owner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .generation
            == self.generation
    }
    pub async fn admit(&self, identity: bool) -> Result<Permit, ()> {
        // Check cooldown before joining a semaphore queue, and again at dispatch.
        self.eligible(identity)?;
        let background = if self.class != Class::Foreground {
            Some(
                tokio::time::timeout_at(
                    self.deadline,
                    self.owner.background.clone().acquire_owned(),
                )
                .await
                .map_err(|_| ())?
                .map_err(|_| ())?,
            )
        } else {
            None
        };
        let total =
            tokio::time::timeout_at(self.deadline, self.owner.total.clone().acquire_owned())
                .await
                .map_err(|_| ())?
                .map_err(|_| ())?;
        self.eligible(identity)?;
        let mut state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation != self.generation || state.probe || Instant::now() >= self.deadline {
            return Err(());
        }
        let recovery = state.blocked.is_some();
        if recovery && (!identity || state.blocked.is_some_and(|until| until > Instant::now())) {
            return Err(());
        }
        if state.cycle.elapsed() >= Duration::from_secs(30) {
            state.cycle = Instant::now();
            state.advisory = 0;
        }
        if self.class == Class::Advisory && state.advisory >= 8 {
            return Err(());
        }
        if let Some(allowance) = &self.allowance {
            allowance
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
                .map_err(|_| ())?;
        }
        if self.class == Class::Advisory {
            state.advisory += 1;
        }
        if recovery {
            state.probe = true;
        }
        Ok(Permit {
            context: self.clone(),
            revision: state.revision,
            recovery,
            finished: false,
            spawned: false,
            _total: total,
            _background: background,
        })
    }
    fn eligible(&self, identity: bool) -> Result<(), ()> {
        let s = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s.admitted
            || s.generation != self.generation
            || s.probe
            || s.blocked.is_some_and(|at| at > Instant::now() || !identity)
            || Instant::now() >= self.deadline
        {
            Err(())
        } else {
            Ok(())
        }
    }
}
pub struct Permit {
    context: Context,
    revision: u64,
    recovery: bool,
    finished: bool,
    pub spawned: bool,
    _total: OwnedSemaphorePermit,
    _background: Option<OwnedSemaphorePermit>,
}
impl Permit {
    pub fn observe(
        &self,
        status: u16,
        remaining: Option<u64>,
        reset: Option<u64>,
        retry: Option<Duration>,
    ) {
        let mut s = self
            .context
            .owner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if s.generation != self.context.generation
            || matches!((s.reset, reset), (Some(old), Some(new)) if new < old)
        {
            return;
        }
        if let Some(reset) = reset {
            s.reset = Some(reset);
        }
        if status == 429 || remaining == Some(0) {
            let duration = retry
                .or_else(|| {
                    reset.map(|epoch| {
                        Duration::from_secs(
                            epoch
                                .saturating_sub(chrono::Utc::now().timestamp().max(0) as u64)
                                .max(1),
                        )
                    })
                })
                .unwrap_or(Duration::from_secs(60));
            let until = Instant::now()
                .checked_add(duration)
                .unwrap_or_else(|| Instant::now() + Duration::from_secs(315360000));
            s.blocked = Some(s.blocked.map_or(until, |old| old.max(until)));
            s.revision += 1;
        }
    }
    pub fn finish(&mut self, viewer: Option<&str>) -> Option<Token> {
        let mut s = self
            .context
            .owner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if s.generation != self.context.generation {
            return None;
        }
        if let Some(viewer) = viewer {
            if s.viewer.as_deref().is_some_and(|old| old != viewer) {
                s.generation = NEXT_OWNER.fetch_add(1, Ordering::AcqRel);
                // Only this fully consumed identity response may retire an account.
                if s.revision == self.revision {
                    s.blocked = None;
                    s.reset = None;
                }
                s.cycle = Instant::now();
                s.advisory = 0;
            }
            s.viewer = Some(viewer.to_owned());
        }
        if self.recovery && viewer.is_some() && s.revision == self.revision {
            s.blocked = None;
        }
        if self.recovery {
            s.probe = false;
            if viewer.is_none() {
                let until = Instant::now() + Duration::from_secs(5);
                s.blocked = Some(s.blocked.map_or(until, |old| old.max(until)));
            }
        }
        self.finished = true;
        Some(Token {
            owner: self.context.owner.clone(),
            generation: s.generation,
        })
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut s = self
            .context
            .owner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if s.generation != self.context.generation {
            return;
        }
        if !self.spawned {
            if self.context.class == Class::Advisory {
                s.advisory = s.advisory.saturating_sub(1);
            }
            if let Some(allowance) = &self.context.allowance {
                allowance.fetch_add(1, Ordering::Release);
            }
        }
        if self.recovery && !self.finished {
            s.probe = false;
            let until = Instant::now() + Duration::from_secs(5);
            s.blocked = Some(s.blocked.map_or(until, |old| old.max(until)));
        }
    }
}
