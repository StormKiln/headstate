//! A content-free nudge when a running session's transcript changes
//! (#1477, epic #1473).
//!
//! # What it is for
//!
//! The transcript follow (`src/lib/transcriptFollow.ts`, #1476) reads a
//! running session on an adaptive cadence that backs off to 15 s while
//! the file is quiet. The first message after a quiet spell therefore
//! waits up to a whole backoff step. This module closes that gap: the
//! desktop `stat`s every RUNNING session's transcript about once a
//! second and emits [`EVENT`] when one changed, and a follower that has
//! that session open reads at once instead of waiting out its delay.
//!
//! # A nudge, never the data
//!
//! The payload is [`SessionActivity`]: the session id, the file's byte
//! size and a sequence number. **No path, no transcript text, no counts
//! beyond the size.** #1488's masking covers `/v1/call` answers only;
//! it does NOT cover `EVENT_NAMES` payloads, so the only safe thing for
//! an event to carry about a transcript is nothing that came out of it.
//! `the_payload_carries_no_transcript_content` pins the field set.
//!
//! Adding the name to `remote/events.rs`' `EVENT_NAMES` widens nothing:
//! `claude_sessions`, an allowlisted Read, already returns every session
//! id to the phone, and a byte size says only that the file changed,
//! which the phone's next page read would tell it anyway.
//!
//! # Polling, never a watcher (#1201)
//!
//! A dead FSEvents stream says nothing, indistinguishably from "no new
//! content". A stat loop that stops is a loop that stops; it cannot go
//! quietly deaf while looking healthy. Each tick observes running mains plus
//! at most 64 explicitly viewed files. Child leases revalidate containment,
//! so their metadata cost is higher than one stat; see the synthetic timing
//! beside B4 in docs/transcript-performance.md. No child discovery walk runs.
//!
//! # Only RUNNING sessions, from `liveness.rs`
//!
//! The running set is [`liveness::derive`] over the live registry's
//! named entries, re-derived every [`REFRESH_EVERY`] ticks. A session
//! that is not `Running` is not stat'ed at all, so the ~1,500 historical
//! transcripts cost nothing. `Unknown` is not `Running`, and a registry
//! we could not read yields an empty set: a nudge that did not fire
//! costs latency only, because the follow's own poll is the backstop
//! (see below), whereas stat'ing every transcript on the machine "in
//! case" would cost a walk of the whole corpus every second.
//!
//! # Only on change
//!
//! The first sighting of a session is a baseline, not a change. A size
//! that differs from the last one seen -- larger, or smaller after a
//! compaction rewrote the file -- is a change. A stat that failed is
//! neither: it records "size unknown" and emits nothing, and a later
//! successful stat after one is a change (the file appeared).
//!
//! # Volume, and the event stream's `CAPACITY`
//!
//! The hub buffers `CAPACITY = 256` frames per subscriber and ENDS the
//! stream of one that falls further behind (it reconnects and replays
//! the PR snapshot). Nudges must not be what pushes a phone over.
//!
//! Emitting only on change keeps the usual volume at zero frames while
//! nothing is writing and one per second per WRITING session. The worst
//! case is bounded by [`MAX_PER_TICK`]: however many sessions are
//! writing, at most that many frames go out per tick, about 90 bytes
//! each. The rest are deferred, not dropped -- their baseline is not
//! advanced, so they are the change the next tick sees. So the worst
//! case is `MAX_PER_TICK` frames per second, and a subscriber whose
//! socket stops draining entirely is cut after `CAPACITY / MAX_PER_TICK`
//! = 32 s of nudges alone, where it would have been cut anyway once any
//! other burst arrived. A cut costs a reconnect; a nudge lost to it
//! costs latency only.
//!
//! # A lost nudge costs latency, never correctness
//!
//! Nudges are lost: a lagging subscriber is cut, a phone reconnecting
//! misses what was emitted meanwhile, a phone that was backgrounded had
//! no stream at all. None of that matters to what is shown, because the
//! follow never depends on a nudge -- it keeps its adaptive poll, and a
//! nudge only cuts the current wait short. `seq` numbers every nudge this
//! process emitted, so a gap in what one listener saw is visible as a
//! gap; it resets when the desktop restarts, so nothing orders on it.

use crate::claude::liveness::{self, ProcessProbe, Registry};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

mod watches;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
pub use watches::{register, WatchLease};

/// The Tauri event name, and the name on the phone's event stream.
pub const EVENT: &str = "claude-session-activity";
pub const TRANSCRIPT_EVENT: &str = "claude-transcript-activity";

/// How often the running transcripts are stat'ed.
pub const TICK: Duration = Duration::from_secs(1);

/// Re-derive the running set (and re-read the prefs) every this many
/// ticks. A session that starts is noticed within this many seconds;
/// its first sighting is a baseline anyway, so nothing is lost.
pub const REFRESH_EVERY: u32 = 5;

/// The most nudges one tick emits. See the module docs' volume section.
pub const MAX_PER_TICK: usize = 8;

/// How long a session id whose transcript could not be found waits
/// before it is looked for again. A bare interactive session has not
/// written a transcript until its first prompt.
pub const RESOLVE_RETRY: Duration = Duration::from_secs(30);

/// The event's payload. Content-free by construction: see the module
/// docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionActivity {
    pub session_id: String,
    /// The transcript's size in bytes when it was stat'ed. The same
    /// measure as a page's `file_bytes`, so a follower that has already
    /// read to this size can ignore the nudge.
    pub size: u64,
    /// Numbers every nudge this process emitted, from 1. Resets on
    /// restart; diagnostic only.
    pub seq: u64,
}

/// Opaque routing only: no filename, path hash, record ID or transcript text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TranscriptActivity {
    pub watch_id: String,
    pub size: u64,
    pub seq: u64,
}

/// The ids of the sessions liveness says are `Running`, from the live
/// registry's named entries.
///
/// A registry that could not be read gives an empty set -- liveness
/// calls every row `Unknown` then -- and so does an entry whose process
/// is gone or whose start time does not match.
pub fn running_ids<P: ProcessProbe>(probe: &P, registry: &Registry) -> Vec<String> {
    let mut ids: Vec<String> = registry
        .entries
        .keys()
        .filter(|id| liveness::derive(probe, registry, id, &[]).is_running())
        .cloned()
        .collect();
    ids.sort();
    ids
}

/// A session id is Claude Code's UUID. Anything else is refused before
/// it is joined onto a path, because it came from another program's
/// file and `..` in it would walk out of the projects directory.
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Finds `<root>/<project>/<id>.jsonl` -- one level down, the only place
/// a session transcript lives (`transcript.rs`' `session_files`) -- and
/// remembers it.
#[derive(Default)]
pub struct Resolver {
    found: HashMap<String, PathBuf>,
    missed: HashMap<String, Instant>,
}

impl Resolver {
    /// The transcript path for `id`, or `None` when it is not there yet
    /// (or was not, less than [`RESOLVE_RETRY`] ago).
    pub fn resolve(&mut self, root: &Path, id: &str, now: Instant) -> Option<PathBuf> {
        if let Some(p) = self.found.get(id) {
            return Some(p.clone());
        }
        if !safe_id(id) {
            return None;
        }
        if let Some(at) = self.missed.get(id) {
            if now.duration_since(*at) < RESOLVE_RETRY {
                return None;
            }
        }
        let name = format!("{id}.jsonl");
        let hit = std::fs::read_dir(root).ok().and_then(|dirs| {
            dirs.filter_map(Result::ok)
                .map(|e| e.path().join(&name))
                .find(|p| p.is_file())
        });
        match hit {
            Some(p) => {
                self.missed.remove(id);
                self.found.insert(id.to_string(), p.clone());
                Some(p)
            }
            None => {
                self.missed.insert(id.to_string(), now);
                None
            }
        }
    }

    /// Forget a path that stopped existing, so it is looked for again.
    pub fn forget(&mut self, id: &str) {
        self.found.remove(id);
    }

    /// Keep only the ids in `keep`.
    pub fn retain(&mut self, keep: &[String]) {
        self.found.retain(|id, _| keep.contains(id));
        self.missed.retain(|id, _| keep.contains(id));
    }
}

/// The sizes last seen, and the sequence counter.
#[derive(Default)]
pub struct Tracker {
    /// `None` = the last stat failed, so the size is not known.
    sizes: HashMap<String, Option<u64>>,
    seq: u64,
    next: usize,
    stats: TickStats,
}

#[derive(Debug, Default)]
struct TickStats {
    observations: usize,
    changed: usize,
    deferred: usize,
}

impl Tracker {
    /// Stat each `(id, path)` of the RUNNING set and return a nudge for
    /// each that changed, at most [`MAX_PER_TICK`]. Ids absent from
    /// `running` are forgotten, so a session that runs again later
    /// starts from a fresh baseline.
    ///
    /// Returns the ids whose file could not be found as well, so the
    /// caller can have their path looked up again.
    pub fn observe(
        &mut self,
        running: &[(String, PathBuf)],
    ) -> (Vec<SessionActivity>, Vec<String>) {
        let keep: HashSet<_> = running.iter().map(|(id, _)| id.as_str()).collect();
        self.sizes.retain(|id, _| keep.contains(id.as_str()));
        self.stats = TickStats::default();
        let mut out = Vec::new();
        let mut vanished = Vec::new();
        let mut observed = HashMap::new();
        for i in 0..running.len() {
            let (id, path) = &running[(self.next + i) % running.len()];
            let (now, missing) = *observed.entry(path).or_insert_with(|| {
                self.stats.observations += 1;
                match std::fs::metadata(path) {
                    Ok(m) => (Some(m.len()), false),
                    Err(e) => (None, e.kind() == std::io::ErrorKind::NotFound),
                }
            });
            if missing {
                vanished.push(id.clone());
            }
            let Some(before) = self.sizes.get(id).copied() else {
                // First sighting: a baseline, not a change.
                self.sizes.insert(id.clone(), now);
                continue;
            };
            let Some(size) = now else {
                // Could not stat it: not a change, and not zero.
                self.sizes.insert(id.clone(), None);
                continue;
            };
            if before == Some(size) {
                continue;
            }
            self.stats.changed += 1;
            if out.len() >= MAX_PER_TICK {
                self.stats.deferred += 1;
                // Deferred, not dropped: the baseline is left where it
                // was, so the next tick sees this change again.
                continue;
            }
            self.seq += 1;
            self.sizes.insert(id.clone(), Some(size));
            out.push(SessionActivity {
                session_id: id.clone(),
                size,
                seq: self.seq,
            });
        }
        if !running.is_empty() {
            self.next = (self.next + MAX_PER_TICK) % running.len();
        }
        (out, vanished)
    }
}

/// The actual steady tick, also exercised by the synthetic timing test.
/// Registry snapshotting is separate and never holds its lock across IO.
fn observe_tick(
    tracker: &mut Tracker,
    resolver: &mut Resolver,
    running: &[(String, PathBuf)],
    children: &[watches::Watch],
    root: Option<&Path>,
) -> (
    Vec<SessionActivity>,
    Vec<TranscriptActivity>,
    Vec<watches::Watch>,
) {
    let mut inputs = running.to_vec();
    let mut invalid = Vec::new();
    let mut child_keys = HashMap::new();
    for child in children {
        // Revalidate every tick so a replaced path component cannot move a
        // lease outside the admitted tree. Metadata remains content-free.
        let valid = root
            .zip(child.path.to_str())
            .and_then(|(root, path)| watches::validate(root, path).ok());
        if valid.as_ref() != Some(&child.path) {
            invalid.push(child.clone());
            continue;
        }
        // A registry session ID cannot contain NUL; identities never collide.
        let key = format!("\0watch:{}", child.id);
        child_keys.insert(key.clone(), child.id.clone());
        inputs.push((key, child.path.clone()));
    }
    let (events, vanished) = tracker.observe(&inputs);
    for id in vanished {
        if let Some(watch_id) = child_keys.get(&id) {
            if let Some(w) = children.iter().find(|w| &w.id == watch_id) {
                invalid.push(w.clone());
            }
        } else {
            resolver.forget(&id);
        }
    }
    let mut main = Vec::new();
    let mut child = Vec::new();
    for e in events {
        if let Some(watch_id) = child_keys.get(&e.session_id) {
            child.push(TranscriptActivity {
                watch_id: watch_id.clone(),
                size: e.size,
                seq: e.seq,
            });
        } else {
            main.push(e);
        }
    }
    (main, child, invalid)
}

/// Run the loop on its own thread for the life of the app.
///
/// Blocking, like the health sampler: it stats files and reads the
/// process table, both of which belong off the async workers. Gated on
/// `claude_integrations_enabled`, re-read every [`REFRESH_EVERY`] ticks
/// so turning the capability off stops the stat'ing within seconds.
pub fn spawn(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        use tauri::Emitter;
        let mut tracker = Tracker::default();
        let mut resolver = Resolver::default();
        let mut running: Vec<(String, PathBuf)> = Vec::new();
        let mut tick: u32 = 0;
        let mut enabled = false;
        loop {
            std::thread::sleep(TICK);
            if tick.is_multiple_of(REFRESH_EVERY) {
                enabled = crate::commands::read_ui_prefs(&app).claude_integrations_enabled;
                running = if enabled {
                    refresh(&mut resolver)
                } else {
                    Vec::new()
                };
            }
            tick = tick.wrapping_add(1);
            // Observed even when empty, so stale baselines are dropped.
            let children = watches::snapshot(Instant::now(), enabled);
            let root = crate::claude::transcript::projects_dir();
            let (nudges, child_nudges, invalid) = observe_tick(
                &mut tracker,
                &mut resolver,
                &running,
                &children,
                root.as_deref(),
            );
            watches::forget(&invalid);
            for n in nudges {
                let _ = app.emit(EVENT, &n);
            }
            for n in child_nudges {
                let _ = app.emit(TRANSCRIPT_EVENT, &n);
            }
        }
    });
}

/// The running set, with each session's transcript path where it has
/// one.
fn refresh(resolver: &mut Resolver) -> Vec<(String, PathBuf)> {
    let (Some(dir), Some(root)) = (
        liveness::registry_dir(),
        crate::claude::transcript::projects_dir(),
    ) else {
        return Vec::new();
    };
    let registry = liveness::read_registry(&dir);
    let probe = liveness::SysinfoProbe::for_pids(&registry.probe_pids());
    let ids = running_ids(&probe, &registry);
    resolver.retain(&ids);
    let now = Instant::now();
    ids.into_iter()
        .filter_map(|id| resolver.resolve(&root, &id, now).map(|p| (id, p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::liveness::RegistryEntry;
    use std::io::Write;

    /// `Fri Sep 11 09:43:48 2026` UTC, as the registry spells it.
    const PROC_START: &str = "Fri Sep 11 09:43:48 2026";
    const PROC_START_EPOCH: i64 = 1_789_119_828;

    /// Pid 100 is alive with the recorded start time; every other pid
    /// is gone.
    struct Table;
    impl ProcessProbe for Table {
        fn start_time(&self, pid: u32) -> Result<Option<i64>, String> {
            Ok((pid == 100).then_some(PROC_START_EPOCH))
        }
    }

    fn entry(id: &str, pid: u32) -> RegistryEntry {
        RegistryEntry {
            pid,
            session_id: id.into(),
            proc_start: Some(PROC_START.into()),
            cwd: Some("/tmp/example".into()),
            ..Default::default()
        }
    }

    fn append(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    /// The field set IS the privacy argument (#1488): the event stream
    /// is not masked, so nothing out of a transcript may ride on it.
    #[test]
    fn the_payload_carries_no_transcript_content() {
        let v = serde_json::to_value(SessionActivity {
            session_id: "aaaa-1".into(),
            size: 42,
            seq: 7,
        })
        .unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["session_id", "size", "seq"]);
    }

    /// The name is on BOTH allowlists, or the phone never hears it: the
    /// desktop's hub only taps names in its list, and the phone's
    /// subscriber drops any name absent from its own.
    #[test]
    fn the_event_is_on_the_desktops_allowlist() {
        assert!(crate::remote::events::EVENT_NAMES.contains(&EVENT));
    }

    #[test]
    fn only_running_sessions_are_in_the_set() {
        let mut r = Registry::default();
        r.entries.insert("live".into(), entry("live", 100));
        r.entries.insert("dead".into(), entry("dead", 200));
        assert_eq!(running_ids(&Table, &r), ["live"]);

        // A registry we could not read: nothing is known to be running.
        r.failure = Some("permission denied".into());
        assert!(running_ids(&Table, &r).is_empty());
    }

    #[test]
    fn emitted_on_growth_and_not_without_a_change() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s1.jsonl");
        append(&path, "{}\n");
        let running = vec![("s1".to_string(), path.clone())];
        let mut t = Tracker::default();

        // First sighting: a baseline.
        assert!(t.observe(&running).0.is_empty());
        // Nothing changed: nothing.
        assert!(t.observe(&running).0.is_empty());
        assert!(t.observe(&running).0.is_empty());

        append(&path, "{\"x\":1}\n");
        let (got, _) = t.observe(&running);
        assert_eq!(
            got,
            [SessionActivity {
                session_id: "s1".into(),
                size: 11,
                seq: 1
            }]
        );
        // Once per change, not once per tick.
        assert!(t.observe(&running).0.is_empty());

        // A rewrite that SHRANK the file is a change too.
        std::fs::write(&path, "{}\n").unwrap();
        let (got, _) = t.observe(&running);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].size, got[0].seq), (3, 2));
    }

    /// A session that is not running is not stat'ed at all, however much
    /// its file grows -- and when it runs again it starts from a fresh
    /// baseline rather than reporting the growth it made while not in
    /// the set.
    #[test]
    fn not_emitted_for_a_session_that_is_not_running() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s1.jsonl");
        append(&path, "{}\n");
        let running = vec![("s1".to_string(), path.clone())];
        let mut t = Tracker::default();
        t.observe(&running);

        // It stops running; its file keeps changing.
        append(&path, "{}\n");
        assert!(t.observe(&[]).0.is_empty());
        append(&path, "{}\n");
        assert!(t.observe(&[]).0.is_empty());

        // Running again: a baseline, not a change.
        assert!(t.observe(&running).0.is_empty());
    }

    /// A stat that failed is not a size of zero, and not a change.
    #[test]
    fn a_missing_file_is_not_a_change_and_its_appearance_is() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s1.jsonl");
        let running = vec![("s1".to_string(), path.clone())];
        let mut t = Tracker::default();
        let (got, vanished) = t.observe(&running);
        assert!(got.is_empty());
        assert_eq!(vanished, ["s1"]);
        assert!(t.observe(&running).0.is_empty());

        append(&path, "{}\n");
        let (got, _) = t.observe(&running);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].size, 3);
    }

    /// Past `MAX_PER_TICK` changes the rest wait a tick; none is lost.
    #[test]
    fn a_burst_is_capped_per_tick_and_deferred_not_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let n = MAX_PER_TICK + 3;
        let running: Vec<(String, PathBuf)> = (0..n)
            .map(|i| {
                let p = tmp.path().join(format!("s{i}.jsonl"));
                append(&p, "{}\n");
                (format!("s{i}"), p)
            })
            .collect();
        let mut t = Tracker::default();
        t.observe(&running);
        for (_, p) in &running {
            append(p, "{}\n");
        }
        let (first, _) = t.observe(&running);
        assert_eq!(first.len(), MAX_PER_TICK);
        let (second, _) = t.observe(&running);
        assert_eq!(second.len(), 3);
        let mut all: Vec<String> = first
            .iter()
            .chain(&second)
            .map(|a| a.session_id.clone())
            .collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n);
        assert!(t.observe(&running).0.is_empty());
    }

    fn child_fixture(root: &Path, parent: &str) -> PathBuf {
        let dir = root.join(parent).join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("agent-same-name.jsonl");
        append(&p, "private-content-sentinel");
        p.canonicalize().unwrap()
    }

    #[test]
    fn only_registered_children_emit_opaque_nudges_and_expiry_drops_baselines() {
        let tmp = tempfile::tempdir().unwrap();
        let a = child_fixture(tmp.path(), "a");
        let b = child_fixture(tmp.path(), "b");
        let unopened = child_fixture(tmp.path(), "unopened");
        let now = Instant::now();
        let mut leases = watches::Watches::default();
        let wa = leases.register_validated(a.clone(), now).unwrap();
        assert_eq!(
            wa.watch_id,
            leases.register_validated(a.clone(), now).unwrap().watch_id
        );
        let wb = leases.register_validated(b.clone(), now).unwrap();
        assert_ne!(wa.watch_id, wb.watch_id);
        let mut tracker = Tracker::default();
        let mut resolver = Resolver::default();
        let watched = leases.snapshot(now);
        assert!(
            observe_tick(&mut tracker, &mut resolver, &[], &watched, Some(tmp.path()))
                .1
                .is_empty()
        );
        append(&a, "x");
        append(&unopened, "x");
        let (main, child, invalid) =
            observe_tick(&mut tracker, &mut resolver, &[], &watched, Some(tmp.path()));
        assert!(main.is_empty() && invalid.is_empty());
        assert_eq!(tracker.stats.observations, 2);
        assert_eq!(child.len(), 1);
        assert_eq!(child[0].watch_id, wa.watch_id);
        let v = serde_json::to_value(&child[0]).unwrap();
        assert_eq!(
            v.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["watch_id", "size", "seq"]
        );
        assert!(!v.to_string().contains("sentinel"));
        assert!(!v.to_string().contains("subagents"));
        assert!(crate::remote::events::EVENT_NAMES.contains(&TRANSCRIPT_EVENT));
        let expired = leases.snapshot(now + watches::WATCH_TTL);
        observe_tick(&mut tracker, &mut resolver, &[], &expired, Some(tmp.path()));
        assert!(tracker.sizes.is_empty());
        assert_eq!(tracker.stats.observations, 0);
    }

    #[test]
    fn children_share_the_main_event_cap_and_are_not_starved() {
        let tmp = tempfile::tempdir().unwrap();
        let running: Vec<_> = (0..20)
            .map(|i| {
                let p = tmp.path().join(format!("main-{i}.jsonl"));
                append(&p, "x");
                (format!("s-{i}"), p)
            })
            .collect();
        let path = child_fixture(tmp.path(), "child");
        let mut leases = watches::Watches::default();
        let now = Instant::now();
        leases.register_validated(path.clone(), now).unwrap();
        let children = leases.snapshot(now);
        let mut tracker = Tracker::default();
        let mut resolver = Resolver::default();
        observe_tick(
            &mut tracker,
            &mut resolver,
            &running,
            &children,
            Some(tmp.path()),
        );
        let mut child_seen = false;
        for _ in 0..4 {
            for (_, p) in &running {
                append(p, "x");
            }
            append(&path, "x");
            let (m, c, _) = observe_tick(
                &mut tracker,
                &mut resolver,
                &running,
                &children,
                Some(tmp.path()),
            );
            assert!(m.len() + c.len() <= MAX_PER_TICK);
            child_seen |= !c.is_empty();
        }
        assert!(child_seen);
        std::fs::remove_file(&path).unwrap();
        let (_, c, invalid) = observe_tick(
            &mut tracker,
            &mut resolver,
            &running,
            &children,
            Some(tmp.path()),
        );
        assert!(c.is_empty());
        assert_eq!(invalid.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_registered_path_replaced_by_an_outside_symlink_is_no_longer_observed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("projects");
        let p = child_fixture(&root, "parent");
        let outside = tmp.path().join("outside.jsonl");
        append(&outside, "private-outside-sentinel");
        let mut leases = watches::Watches::default();
        let now = Instant::now();
        leases.register_validated(p.clone(), now).unwrap();
        let children = leases.snapshot(now);
        let mut tracker = Tracker::default();
        let mut resolver = Resolver::default();
        observe_tick(&mut tracker, &mut resolver, &[], &children, Some(&root));
        std::fs::remove_file(&p).unwrap();
        std::os::unix::fs::symlink(&outside, &p).unwrap();
        let (_, events, invalid) =
            observe_tick(&mut tracker, &mut resolver, &[], &children, Some(&root));
        assert!(events.is_empty());
        assert_eq!(invalid.len(), 1);
        assert_eq!(tracker.stats.observations, 0);
    }

    #[test]
    fn sustained_early_writers_cannot_starve_later_views() {
        let tmp = tempfile::tempdir().unwrap();
        let paths: Vec<_> = (0..MAX_PER_TICK + 1)
            .map(|i| {
                let p = tmp.path().join(format!("{i}.jsonl"));
                append(&p, "x");
                (format!("id-{i}"), p)
            })
            .collect();
        let mut t = Tracker::default();
        t.observe(&paths);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..3 {
            for (_, p) in &paths {
                append(p, "x");
            }
            let (events, _) = t.observe(&paths);
            assert!(events.len() <= MAX_PER_TICK);
            seen.extend(events.into_iter().map(|e| e.session_id));
        }
        assert_eq!(seen.len(), paths.len());
    }

    #[test]
    fn the_resolver_finds_a_transcript_one_level_down_and_refuses_an_unsafe_id() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("projects");
        let project = root.join("-tmp-example");
        std::fs::create_dir_all(&project).unwrap();
        let now = Instant::now();
        let mut r = Resolver::default();

        // Not written yet: a miss, not retried within RESOLVE_RETRY.
        assert_eq!(r.resolve(&root, "abc-1", now), None);
        append(&project.join("abc-1.jsonl"), "{}\n");
        assert_eq!(
            r.resolve(&root, "abc-1", now + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            r.resolve(&root, "abc-1", now + RESOLVE_RETRY),
            Some(project.join("abc-1.jsonl"))
        );

        // An id that would walk out of the projects directory.
        std::fs::write(tmp.path().join("escape.jsonl"), "{}\n").unwrap();
        assert_eq!(r.resolve(&root, "../escape", now), None);
        assert_eq!(r.resolve(&root, "", now), None);
    }
    /// Opt-in synthetic filesystem measurement of the actual shared tick path.
    /// Fixture creation/mutation and independent lease admission are not timed as ticks.
    #[test]
    #[ignore = "manual synthetic timing; no private corpus"]
    fn synthetic_watch_tick_measurement() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut running = Vec::new();
        for i in 0..200 {
            let p = root.join(format!("main-{i}.jsonl"));
            std::fs::write(&p, b"fixture").unwrap();
            running.push((format!("main-{i}"), p));
        }
        let mut watches = watches::Watches::default();
        let mut paths = Vec::new();
        for i in 0..watches::MAX_WATCHES {
            paths.push(child_fixture(&root, &format!("parent-{i}")));
        }
        fn report(name: &str, times: &mut [f64]) {
            times.sort_by(f64::total_cmp);
            println!(
                "{name}: n={} median_ms={:.3} p95_ms={:.3}",
                times.len(),
                times[times.len() / 2],
                times[times.len() * 95 / 100]
            );
        }
        let mut admissions = Vec::new();
        for round in 0..110 {
            let started = Instant::now();
            for p in &paths {
                let admitted = watches::validate(&root, p.to_str().unwrap()).unwrap();
                watches
                    .register_validated(admitted, Instant::now())
                    .unwrap();
            }
            if round >= 10 {
                admissions.push(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
        report("admission+renewal batch64 (separate)", &mut admissions);
        for mode in ["unchanged", "growth", "shrink", "missing"] {
            let mut tracker = Tracker::default();
            let mut resolver = Resolver::default();
            let mut times = Vec::new();
            for round in 0..110 {
                for (_, p) in &running {
                    std::fs::write(p, vec![b'x'; 1000]).unwrap();
                }
                for p in &paths {
                    std::fs::write(p, vec![b'x'; 1000]).unwrap();
                }
                tracker.observe(&[]);
                observe_tick(
                    &mut tracker,
                    &mut resolver,
                    &running,
                    &watches.snapshot(Instant::now()),
                    Some(&root),
                );
                match mode {
                    "growth" | "shrink" => {
                        let bytes = if mode == "growth" { 2000 } else { 10 };
                        for (_, p) in &running {
                            std::fs::write(p, vec![b'x'; bytes]).unwrap();
                        }
                        for p in &paths {
                            std::fs::write(p, vec![b'x'; bytes]).unwrap();
                        }
                    }
                    "missing" => {
                        for (_, p) in &running {
                            std::fs::remove_file(p).unwrap();
                        }
                        for p in &paths {
                            std::fs::remove_file(p).unwrap();
                        }
                    }
                    _ => {}
                }
                let started = Instant::now();
                let children = watches.snapshot(Instant::now());
                let (main, child, invalid) = observe_tick(
                    &mut tracker,
                    &mut resolver,
                    &running,
                    &children,
                    Some(&root),
                );
                let elapsed = started.elapsed().as_secs_f64() * 1000.0;
                assert!(main.len() + child.len() <= MAX_PER_TICK);
                if round >= 10 {
                    times.push(elapsed);
                }
                if round == 109 {
                    println!("{mode}: observations={} changed={} emitted={} deferred={} retained={} invalid={}", tracker.stats.observations, tracker.stats.changed, main.len()+child.len(), tracker.stats.deferred, tracker.sizes.len(), invalid.len());
                }
            }
            report(mode, &mut times);
        }
    }
}
