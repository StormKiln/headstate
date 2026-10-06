//! Bounded sampled comparisons, never upstream turn attribution.
use super::*;
const RETAIN_MS: u64 = 600_000;
#[derive(Clone)]
struct Hook {
    at: Option<i64>,
    seen: u64,
}
#[derive(Clone)]
struct Boundary {
    at: i64,
    seen: u64,
    ordinal: u32,
    censored: bool,
}
struct Session {
    id: OpaqueId,
    hooks: VecDeque<Hook>,
    boundaries: VecDeque<Boundary>,
    busy: bool,
    last_idle: Option<i64>,
    process: Option<(u32, String)>,
    last_pair: Option<(i64, i64, usize, usize)>,
    touched: u64,
}
#[derive(Default)]
pub(super) struct Observer {
    sessions: HashMap<String, Session>,
    next: u32,
    last_mono: u64,
}
impl Observer {
    pub fn clear(&mut self) -> u64 {
        let n = self
            .sessions
            .values()
            .map(|s| s.hooks.len() + s.boundaries.len())
            .sum::<usize>() as u64;
        self.sessions.clear();
        n
    }
}
impl Recorder {
    pub fn failure_count(&self, metric: AggregateKind, count: u64) {
        if count == 0 {
            return;
        }
        self.aggregate(AggregateDelta {
            domain: Domain::StopFailure,
            metric,
            work: WorkClass::Unknown,
            count,
        });
    }
    // Keep the closed event provenance and each optional source time explicit.
    #[allow(clippy::too_many_arguments)]
    fn failure_event(
        &self,
        id: &OpaqueId,
        observation: FailureObservation,
        boundary: Option<u32>,
        delta: Option<i64>,
        lag: Option<u64>,
        age: Option<u64>,
        outcome: MatchOutcome,
    ) {
        self.record(Event::StopFailureMatch {
            session: id.clone(),
            observed_boundary: boundary,
            observation,
            delta_ms: delta,
            observed_lag_ms: lag.and_then(|v| i64::try_from(v).ok()),
            hook_age_ms: age,
            outcome,
        });
    }
    fn failure_session<'a>(
        &self,
        o: &'a mut Observer,
        sid: &str,
        now: u64,
    ) -> Option<&'a mut Session> {
        if now < o.last_mono {
            for s in o.sessions.values() {
                self.retire(&s.id);
            }
            let n = o.clear();
            self.failure_count(AggregateKind::Eviction, n);
        }
        o.last_mono = now;
        let expired: Vec<_> = o
            .sessions
            .iter()
            .filter(|(_, s)| now.saturating_sub(s.touched) >= RETAIN_MS)
            .map(|(k, _)| k.clone())
            .collect();
        for k in expired {
            if let Some(s) = o.sessions.remove(&k) {
                self.failure_count(
                    AggregateKind::Eviction,
                    (s.hooks.len() + s.boundaries.len()) as u64,
                );
                self.retire(&s.id);
            }
        }
        if !o.sessions.contains_key(sid) {
            if sid.len() > 1024 || o.sessions.len() >= 128 {
                self.failure_count(AggregateKind::Declined, 1);
                return None;
            }
            o.next = o.next.checked_add(1)?;
            let id = self.intern(Key::Session(&format!("sample:{}:{sid}", o.next)))?;
            o.sessions.insert(
                sid.into(),
                Session {
                    id,
                    hooks: VecDeque::new(),
                    boundaries: VecDeque::new(),
                    busy: false,
                    last_idle: None,
                    process: None,
                    last_pair: None,
                    touched: now,
                },
            );
        }
        let s = o.sessions.get_mut(sid)?;
        s.touched = now;
        let before = s.hooks.len() + s.boundaries.len();
        s.hooks.retain(|h| now.saturating_sub(h.seen) < RETAIN_MS);
        s.boundaries
            .retain(|b| now.saturating_sub(b.seen) < RETAIN_MS);
        self.failure_count(
            AggregateKind::Eviction,
            (before - s.hooks.len() - s.boundaries.len()) as u64,
        );
        Some(s)
    }
    /// Called only after the hook transaction commits. Source time stays absent
    /// when legacy storage supplied an ingestion-time fallback.
    pub fn failure_hook(&self, sid: &str, at: Option<i64>, inserted: bool) {
        if !self.enabled() {
            return;
        }
        let (mono, wall) = self.clock.now();
        let mut o = self.failure_observer.lock().unwrap();
        let Some(s) = self.failure_session(&mut o, sid, mono) else {
            return;
        };
        let age = at
            .and_then(|t| u64::try_from(t).ok())
            .and_then(|t| wall.checked_sub(t));
        let outcome = if !inserted {
            MatchOutcome::Duplicate
        } else if at.is_none() || age.is_none() {
            MatchOutcome::ClockAnomaly
        } else {
            MatchOutcome::HookObserved
        };
        self.failure_event(
            &s.id,
            FailureObservation::IngestedHook,
            None,
            None,
            None,
            age,
            outcome,
        );
        if !inserted {
            return;
        }
        if s.hooks.len() == 4 {
            s.hooks.pop_front();
            self.failure_count(AggregateKind::Eviction, 1);
        }
        s.hooks.push_back(Hook { at, seen: mono });
        if s.boundaries.is_empty() {
            self.failure_event(
                &s.id,
                FailureObservation::CandidatePair,
                None,
                None,
                None,
                None,
                MatchOutcome::Unpaired,
            );
        }
        self.failure_pair(s, None, wall);
    }
    /// Source timestamps come from the existing registry/newest-failure read.
    /// `process` is native-only identity, not a logged label or authority.
    pub fn failure_sample(
        &self,
        sid: &str,
        process: Option<(u32, String)>,
        busy: bool,
        idle: Option<i64>,
        newest: Option<i64>,
    ) {
        if !self.enabled() {
            return;
        }
        let (mono, wall) = self.clock.now();
        let mut o = self.failure_observer.lock().unwrap();
        let ordinal = o.next.checked_add(1);
        if idle.is_some() {
            o.next = ordinal.unwrap_or(o.next);
        }
        let Some(s) = self.failure_session(&mut o, sid, mono) else {
            return;
        };
        if process.as_ref().is_some_and(|p| p.1.len() > 1024) {
            self.failure_count(AggregateKind::Declined, 1);
            return;
        }
        if s.process.is_some() && s.process != process {
            self.failure_count(
                AggregateKind::Eviction,
                (s.hooks.len() + s.boundaries.len()) as u64,
            );
            s.hooks.clear();
            s.boundaries.clear();
            s.last_pair = None;
            s.busy = false;
            s.last_idle = None;
        }
        s.process = process;
        if busy {
            s.busy = true;
            return;
        }
        let Some(at) = idle else {
            s.busy = false;
            self.failure_count(AggregateKind::NoWork, 1);
            return;
        };
        if s.last_idle == Some(at) {
            self.failure_count(AggregateKind::Coalesced, 1);
            if s.boundaries.is_empty() {
                if let Some(hook) = newest {
                    let pair = (at, hook, 0, 0);
                    if s.last_pair != Some(pair) {
                        s.last_pair = Some(pair);
                        let lag = u64::try_from(at).ok().and_then(|t| wall.checked_sub(t));
                        let outcome =
                            if lag.is_none() || u64::try_from(hook).ok().is_none_or(|t| t > wall) {
                                MatchOutcome::ClockAnomaly
                            } else {
                                MatchOutcome::Censored
                            };
                        self.failure_event(
                            &s.id,
                            FailureObservation::RetrospectivePair,
                            None,
                            s.hooks
                                .iter()
                                .any(|h| h.at == Some(hook))
                                .then(|| hook.checked_sub(at))
                                .flatten(),
                            lag,
                            None,
                            outcome,
                        );
                    }
                }
            }
            self.failure_pair(s, newest, wall);
            return;
        }
        s.last_idle = Some(at);
        let Some(ordinal) = ordinal else {
            self.failure_count(AggregateKind::Eviction, 1);
            return;
        };
        let lag = u64::try_from(at).ok().and_then(|t| wall.checked_sub(t));
        let censored = !s.busy;
        s.busy = false;
        self.failure_event(
            &s.id,
            FailureObservation::SampledIdle,
            Some(ordinal),
            None,
            lag,
            None,
            if lag.is_none() {
                MatchOutcome::ClockAnomaly
            } else if censored {
                MatchOutcome::Censored
            } else {
                MatchOutcome::BoundaryObserved
            },
        );
        if s.boundaries.len() == 4 {
            s.boundaries.pop_front();
            self.failure_count(AggregateKind::Eviction, 1);
        }
        s.boundaries.push_back(Boundary {
            at,
            seen: mono,
            ordinal,
            censored,
        });
        self.failure_pair(s, newest, wall);
    }
    fn failure_pair(&self, s: &mut Session, newest: Option<i64>, wall: u64) {
        let Some(b) = s.boundaries.back() else {
            return;
        };
        let independent = s.hooks.back().and_then(|h| h.at);
        let Some(hook) = newest.or(independent) else {
            self.failure_event(
                &s.id,
                FailureObservation::CandidatePair,
                Some(b.ordinal),
                None,
                None,
                None,
                MatchOutcome::Unpaired,
            );
            return;
        };
        let pair = (b.at, hook, s.hooks.len(), s.boundaries.len());
        if s.last_pair == Some(pair) {
            return;
        }
        s.last_pair = Some(pair);
        let observed = s.hooks.iter().any(|h| h.at == Some(hook));
        // History may contain the legacy ingestion-time fallback. Only an
        // independently retained source timestamp establishes a hook-time delta.
        let delta = observed.then(|| hook.checked_sub(b.at)).flatten();
        let lag = u64::try_from(b.at).ok().and_then(|t| wall.checked_sub(t));
        let outcome = if (observed && delta.is_none())
            || lag.is_none()
            || u64::try_from(hook).ok().is_none_or(|t| t > wall)
        {
            MatchOutcome::ClockAnomaly
        } else if !observed || b.censored {
            MatchOutcome::Censored
        } else if s.hooks.len() != 1 || s.boundaries.len() != 1 {
            MatchOutcome::Ambiguous
        } else if delta.is_some_and(|d| d < -15_000) {
            MatchOutcome::OutOfWindow
        } else {
            MatchOutcome::Matched
        };
        self.failure_event(
            &s.id,
            if observed {
                FailureObservation::CandidatePair
            } else {
                FailureObservation::RetrospectivePair
            },
            Some(b.ordinal),
            delta,
            lag,
            None,
            outcome,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    struct Time(AtomicU64);
    impl Clock for Time {
        fn now(&self) -> (u64, u64) {
            let t = self.0.load(Ordering::Relaxed);
            (t, 1_000_000 + t)
        }
    }
    #[tokio::test]
    async fn correlation_caps_expiry_disable_and_clock_anomaly_are_explicit_without_cross_capture_joins(
    ) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let clock = Arc::new(Time(AtomicU64::new(1)));
        let r = Recorder::with_clock(
            Config {
                directory: root.join("journal"),
                epoch: [56; 16],
                role: Role::Desktop,
                platform: Platform::Macos,
                build: "synthetic".into(),
            },
            Caps::default(),
            clock.clone(),
        )
        .unwrap();
        r.set_enabled(true);
        for n in 0..6 {
            r.failure_hook("one", Some(990_000 + n), true);
            r.failure_sample("one", Some((1, "start".into())), true, None, None);
            r.failure_sample(
                "one",
                Some((1, "start".into())),
                false,
                Some(990_100 + n),
                None,
            );
        }
        {
            let o = r.failure_observer.lock().unwrap();
            let s = o.sessions.get("one").unwrap();
            assert_eq!(s.hooks.len(), 4);
            assert_eq!(s.boundaries.len(), 4);
        }
        for n in 0..140 {
            r.failure_hook(&format!("synthetic-{n}"), None, true);
        }
        assert_eq!(r.failure_observer.lock().unwrap().sessions.len(), 128);
        clock.0.store(600_002, Ordering::Relaxed);
        r.failure_hook("later", Some(1_600_003), true);
        assert_eq!(r.failure_observer.lock().unwrap().sessions.len(), 1);
        r.set_enabled(false);
        assert!(r.failure_observer.lock().unwrap().sessions.is_empty());
        let old = root.join("old");
        r.export_to(old.clone()).await.unwrap();
        let text = std::fs::read_to_string(old).unwrap();
        assert!(text.contains("eviction"));
        assert!(text.contains("clock_anomaly"));
        assert!(text.contains("ambiguous"));
        r.set_enabled(true);
        r.failure_sample(
            "later",
            Some((2, "new".into())),
            false,
            Some(1_600_000),
            None,
        );
        let o = r.failure_observer.lock().unwrap();
        let s = o.sessions.get("later").unwrap();
        assert!(s.hooks.is_empty());
        assert!(s.boundaries[0].censored);
    }
    #[tokio::test]
    async fn gaps_replacements_and_retrospective_newest_values_never_become_certain_turns() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let r = Recorder::new(Config {
            directory: root.join("journal"),
            epoch: [57; 16],
            role: Role::Desktop,
            platform: Platform::Macos,
            build: "synthetic".into(),
        })
        .unwrap();
        r.set_enabled(true);
        r.failure_sample("s", Some((1, "a".into())), true, None, None);
        r.failure_sample("s", None, false, None, None); // unreadable registry gap
        r.failure_sample("s", Some((1, "a".into())), false, Some(1000), Some(900));
        r.failure_hook("s", Some(900), true);
        r.failure_sample("s", Some((2, "b".into())), false, Some(2000), Some(900));
        let path = root.join("report");
        r.export_to(path.clone()).await.unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("retrospective_pair"));
        assert!(text.contains("censored"));
        assert!(!text.contains("\"outcome\":\"matched\""));
        let o = r.failure_observer.lock().unwrap();
        assert!(o.sessions["s"].hooks.is_empty());
    }
}
