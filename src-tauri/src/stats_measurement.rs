//! Observations of work already performed. No provider/store/demand access.
use crate::{measurement::*, store::stats_owner::StatsOwner};
use chrono::Datelike;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
#[derive(Clone)]
pub(crate) enum Source {
    Shared(Arc<Recorder>),
    Desktop(&'static Recorder),
}
impl Source {
    pub fn recorder(&self) -> &Recorder {
        match self {
            Self::Shared(r) => r,
            Self::Desktop(r) => r,
        }
    }
}
#[derive(Clone)]
pub(crate) struct Observation {
    source: Source,
    pub id: OpaqueId,
}
impl Observation {
    pub fn emit(
        &self,
        outcome: StatsOutcome,
        board: Option<&crate::github::stats::Board>,
        elapsed: Option<u64>,
    ) {
        let r = self.source.recorder();
        let days = board.and_then(|b| (b.days_total > 0).then_some(b.days_total));
        r.record(Event::StatsProgress {
            scope: self.id.clone(),
            days: days.and_then(|v| r.measured_days(v)),
            covered_days: board
                .filter(|_| days.is_some())
                .and_then(|b| r.measured_days(b.days_covered)),
            partial_days: None,
            unknown_days: None,
            retrieved: board.and_then(|b| r.measured_count(b.retrieved)),
            accumulated: board
                .filter(|b| b.accumulating)
                .and_then(|b| r.measured_count(b.accumulated)),
            total: board
                .and_then(|b| b.total.filter(|_| b.total_verified))
                .and_then(|v| r.measured_count(v)),
            outcome,
            elapsed_ms: elapsed,
        });
    }
    pub fn progress(&self, report: &crate::github::stats::backfill::Report) {
        let r = self.source.recorder();
        r.record(Event::StatsProgress {
            scope: self.id.clone(),
            days: r.measured_days(report.days_total),
            covered_days: r.measured_days(report.days_covered),
            partial_days: None,
            unknown_days: None,
            retrieved: None,
            accumulated: r.measured_count(report.collected),
            total: report.total.and_then(|v| r.measured_count(v)),
            outcome: StatsOutcome::Accepted,
            elapsed_ms: None,
        });
    }
    pub fn commit(&self, rows: usize, covered: Option<usize>, days: usize, work: WorkClass) {
        let r = self.source.recorder();
        aggregate(Some(r), AggregateKind::Upsert, work, rows as u64);
        r.record(Event::StatsProgress {
            scope: self.id.clone(),
            days: r.measured_days(days),
            covered_days: covered.and_then(|v| r.measured_days(v)),
            partial_days: None,
            unknown_days: None,
            retrieved: None,
            accumulated: None,
            total: None,
            outcome: StatsOutcome::Committed,
            elapsed_ms: None,
        });
    }
}
struct State {
    generation: i64,
    owner: OpaqueId,
    source: Source,
    scopes: HashMap<String, (i32, i32, OpaqueId)>,
    revision: u64,
}
impl Drop for State {
    fn drop(&mut self) {
        self.source.recorder().retire(&self.owner);
    }
}
pub(crate) struct Tracker {
    identity: u64,
    state: Mutex<Option<State>>,
}
impl Default for Tracker {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self {
            identity: NEXT.fetch_add(1, Ordering::Relaxed),
            state: Mutex::new(None),
        }
    }
}
impl Tracker {
    pub fn scope(
        &self,
        source: Option<Source>,
        owner: &StatsOwner,
        key: &str,
        from: &str,
        to: &str,
    ) -> Option<Observation> {
        let source = source?;
        let r = source.recorder();
        if !r.enabled() {
            self.state.lock().unwrap().take();
            return None;
        }
        let from = chrono::NaiveDate::parse_from_str(from, "%Y-%m-%d")
            .ok()?
            .num_days_from_ce();
        let to = chrono::NaiveDate::parse_from_str(to, "%Y-%m-%d")
            .ok()?
            .num_days_from_ce();
        let mut state = self.state.lock().unwrap();
        if state
            .as_ref()
            .is_some_and(|s| s.generation > owner.generation())
        {
            return None;
        }
        let private = format!(
            "stats:{}:{}:{}",
            self.identity,
            owner.generation(),
            owner.viewer()
        );
        let parent = r.intern(Key::Owner(&private))?;
        if state.as_ref().is_none_or(|s| s.owner != parent) {
            *state = Some(State {
                generation: owner.generation(),
                owner: parent,
                source: source.clone(),
                scopes: HashMap::new(),
                revision: 0,
            });
        }
        let state = state.as_mut().unwrap();
        if let Some((a, b, id)) = state.scopes.get(key) {
            if (*a, *b) == (from, to) {
                return Some(Observation {
                    source,
                    id: id.clone(),
                });
            }
            r.retire(id);
            state.scopes.remove(key);
        }
        if state.scopes.len() >= 32 {
            aggregate(Some(r), AggregateKind::Declined, WorkClass::Unknown, 1);
            return None;
        }
        state.revision = state.revision.checked_add(1)?;
        let scope_key = format!("{}:{key}", state.revision);
        let id = r.intern(Key::StatsScope {
            owner: &private,
            scope: &scope_key,
            from_day: from,
            to_day: to,
        })?;
        state.scopes.insert(key.into(), (from, to, id.clone()));
        Some(Observation { source, id })
    }
}
pub(crate) fn aggregate(r: Option<&Recorder>, metric: AggregateKind, work: WorkClass, count: u64) {
    if let Some(r) = r {
        r.aggregate(AggregateDelta {
            domain: Domain::Stats,
            metric,
            work,
            count,
        });
    }
}
pub(crate) fn elapsed(start: Instant) -> Option<u64> {
    u64::try_from(start.elapsed().as_millis()).ok()
}

pub(crate) struct CommitGuard<'a> {
    observation: Option<&'a Observation>,
    pub committed: bool,
}
impl<'a> CommitGuard<'a> {
    pub fn new(observation: Option<&'a Observation>) -> Self {
        Self {
            observation,
            committed: false,
        }
    }
}
impl Drop for CommitGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            if let Some(o) = self.observation {
                o.emit(StatsOutcome::CommitFailed, None, None);
            }
        }
    }
}

/// A scoped request that exits early still has an observed, count-free refusal.
pub(crate) struct AnswerGuard {
    observation: Option<Observation>,
    started: Instant,
    complete: bool,
}
impl AnswerGuard {
    pub fn new(observation: Option<Observation>, started: Instant) -> Self {
        Self {
            observation,
            started,
            complete: false,
        }
    }
    pub fn finish(&mut self, outcome: StatsOutcome, board: Option<&crate::github::stats::Board>) {
        if let Some(o) = &self.observation {
            o.emit(outcome, board, elapsed(self.started));
        }
        self.complete = true;
    }
}
impl Drop for AnswerGuard {
    fn drop(&mut self) {
        if !self.complete {
            self.finish(StatsOutcome::Rejected, None);
        }
    }
}
