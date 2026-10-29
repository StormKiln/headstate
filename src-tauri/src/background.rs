//! Whether the background loops are still doing their job (#1145).
//!
//! Every long-running loop logs its failures and carries on: the health
//! sampler and the Claude live pass every 60 s, the GitHub and GitLab
//! polls, the queue continuation clock, and the Claude activity nudge.
//! Each is deliberately never fatal -- a pass that could not read
//! `~/.claude` must not stop the loop that also records system health --
//! and until #1145 none had any path to the screen. Until #1937 only the
//! first two did, so System Health called background work healthy while
//! PR polling had stopped.
//!
//! # Failed, and stalled
//!
//! Two different ways to stop doing the job. A loop that runs and fails
//! counts [`Task::failed`]. A loop stuck on an `.await` that never
//! returns counts nothing at all -- so each loop also declares, before
//! it waits, when its next cycle should have finished ([`Task::tick`]),
//! and a snapshot taken past that deadline is `stalled`. A loop waiting
//! on purpose (its source switched off) says so with [`Task::park`],
//! which is not a stall.
//!
//! The deadline is on the monotonic clock, the one `tokio::time::sleep`
//! and `std::thread::sleep` wait on, so a laptop lid closed for a night
//! does not read as a night-long stall: the loop's own sleep and its
//! deadline pause together.
//!
//! # The gap that means two different things
//!
//! A user reading the health chart sees a gap and cannot tell whether
//! the app was closed or whether the sampler ran every minute for an
//! hour and failed to write every time. `SystemHealthPage` states "Gaps
//! are gaps. The sampler only runs while the app is open" -- correct for
//! a closed app, and exactly wrong for a sampler that ran and failed.
//!
//! That is #1042's shape: the honest reading and the alarming one look
//! identical, so the page tells the user the reassuring one.
//!
//! # In memory, not SQLite
//!
//! A relaunch re-arms this, which is correct: the counts describe THIS
//! process's loops, and a failure count restored from disk would
//! describe a run that is over. `lib.rs` already gives this reasoning
//! for `Fired`. It also means the health writer failing cannot itself
//! be the reason the failure goes unrecorded -- the one case a
//! disk-backed count would miss.
//!
//! # One blip is weather
//!
//! `poll.rs`'s `FAILURES_BEFORE_BANNER` reasoning applies: a single
//! failed write that fixes itself before the user finishes reading the
//! banner is not worth a banner. [`Task::degraded`] is the threshold,
//! and the RAW count is kept alongside so the page can say how long it
//! has been going rather than only that it is.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Consecutive failures before a task counts as degraded.
///
/// Two, matching `poll.rs`'s banner threshold and for the same reason:
/// it delays a real outage's report by one cycle, which is a fair price
/// for not alarming a user about something that fixed itself.
///
/// For the health sampler that is a 60 s cycle, so a degraded report
/// means "it has been failing for at least a minute" rather than "it
/// blinked once".
pub const FAILURES_BEFORE_DEGRADED: u64 = 2;

/// One background loop's recent history.
///
/// Every field answers a question the page asks, and the two counts are
/// NOT the same question: `consecutive_failures` is "is it broken now",
/// `total_failures` is "has it been unreliable". A loop that fails every
/// other minute has a consecutive count that keeps resetting to zero and
/// is plainly not healthy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskHealth {
    /// A stable identifier, e.g. `health-sampler`.
    pub task: String,
    /// How many failures since the last success.
    pub consecutive_failures: u64,
    /// How many failures since the app started.
    pub total_failures: u64,
    /// The most recent failure's message, or `None` if it has never
    /// failed.
    ///
    /// Kept across a later success on purpose: "it failed 40 times and
    /// then recovered" is worth reading, and clearing the reason the
    /// moment it works again is how a user ends up with a gap in a
    /// chart and nothing that explains it.
    pub last_error: Option<String>,
    /// Milliseconds since the epoch of the last success, or `None` if
    /// it has never succeeded.
    ///
    /// `None` is NOT zero and NOT "never ran" -- a loop that has only
    /// ever failed has `None` here and a non-zero failure count, which
    /// is a different state from one that has not started yet (#1042).
    pub last_success_ms: Option<u64>,
    /// Milliseconds since the epoch at which the loop last came round to
    /// wait for its next cycle, whatever the last cycle's outcome; `None`
    /// if it has not got that far yet.
    ///
    /// Distinct from `last_success_ms`: a loop that keeps finishing
    /// cycles that fail is alive, and one that has stopped finishing them
    /// is stuck. Those want different words on the page.
    pub last_tick_ms: Option<u64>,
    /// Whether the loop is past the deadline it set for its next cycle.
    ///
    /// `false` before the first cycle -- "not started yet" is Pending,
    /// not a stall (#1042) -- and while the loop is parked on purpose.
    pub stalled: bool,
    /// Whether this crossed the threshold, or stalled.
    ///
    /// Carried rather than re-derived at each call site, so the page and
    /// any future notifier cannot come to disagree about what counts.
    pub degraded: bool,
    /// Waiting with no deadline, on purpose: its source is switched off
    /// or not set up (#2036). Not a stall, and not a failure.
    pub parked: bool,
    /// Epoch ms at which the loop will try again after being told to
    /// wait -- a rate limit's `Retry-After` (#2036). `None` when it is on
    /// its ordinary cadence, which is not "retry now".
    pub retry_at_ms: Option<u64>,
}

/// How far past its own declared cadence a cycle may run before the loop
/// counts as stalled.
///
/// Generous on purpose -- a stall report that might be wrong is worse
/// than one that is late. The longest bounded cycle here is a GitHub tick
/// (`poll::TICK_TIMEOUT`, 45 s, plus persistence) or a health sample with
/// its Claude live pass; five minutes clears both with room, and still
/// says "stuck" long before a user would otherwise find out.
pub const STALL_GRACE: std::time::Duration = std::time::Duration::from_secs(300);

/// One cycle's result, for a loop that does more than one piece of work
/// per cycle (both PR queues, for instance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cycle {
    /// The work was done.
    Ok,
    /// The work was attempted and failed, with the reason.
    Failed(String),
    /// Another publication won; this attempt contributes no new health
    /// outcome and must neither clear nor increment failure counts.
    Superseded,
    /// Nothing was attempted -- declined by admission, a source switched
    /// off, nothing due. Neither a success nor a failure (#1050: what we
    /// did not ask must not read as what they did not answer).
    NotAsked,
}

/// A background loop's failure state.
///
/// Atomics for the counts -- they are written on a 60 s cadence and read
/// on demand, so contention is not a consideration and a lock per write
/// would be noise. The error string needs a `Mutex` because it is not a
/// scalar; it is written only on failure.
#[derive(Debug)]
pub struct Task {
    name: &'static str,
    consecutive: AtomicU64,
    total: AtomicU64,
    /// 0 means "never succeeded", which `snapshot` renders as `None`.
    ///
    /// A sentinel rather than an `Option` in an atomic, because there is
    /// no `AtomicOption`. 0 is safe as the sentinel: it is 1970, and a
    /// real success timestamp cannot be it.
    last_success_ms: AtomicU64,
    last_error: Mutex<Option<String>>,
    /// Wall-clock ms of the last finished cycle; 0 means none yet.
    last_tick_ms: AtomicU64,
    /// Monotonic ms ([`mono_ms`]) by which the next cycle should have
    /// finished; 0 means no deadline (not started, or parked).
    due_mono_ms: AtomicU64,
    /// Whether the loop last waited parked rather than on a deadline.
    parked: std::sync::atomic::AtomicBool,
    /// Wall-clock ms of the next try after a told-to-wait; 0 means none.
    retry_at_ms: AtomicU64,
}

impl Task {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            consecutive: AtomicU64::new(0),
            total: AtomicU64::new(0),
            last_success_ms: AtomicU64::new(0),
            last_error: Mutex::new(None),
            last_tick_ms: AtomicU64::new(0),
            due_mono_ms: AtomicU64::new(0),
            parked: std::sync::atomic::AtomicBool::new(false),
            retry_at_ms: AtomicU64::new(0),
        }
    }

    /// The loop finished a cycle and is about to wait up to `next_in`
    /// before starting the next one.
    ///
    /// Call it after recording the cycle's outcome and before the wait,
    /// with the wait's own length: the deadline is that wait plus
    /// [`STALL_GRACE`] for the next cycle's work.
    pub fn tick(&self, next_in: std::time::Duration) {
        self.last_tick_ms.store(now_ms(), Ordering::Relaxed);
        let due = mono_ms()
            .saturating_add(next_in.as_millis() as u64)
            .saturating_add(STALL_GRACE.as_millis() as u64);
        // `max(1)`: 0 is the no-deadline sentinel.
        self.due_mono_ms.store(due.max(1), Ordering::Relaxed);
        self.parked.store(false, Ordering::Relaxed);
        self.retry_at_ms.store(0, Ordering::Relaxed);
    }

    /// As [`Task::tick`], for a wait the loop was TOLD to take -- a rate
    /// limit -- rather than its own cadence. The page says "retrying at"
    /// rather than leaving the reader to guess why nothing moved.
    pub fn retry_in(&self, wait: std::time::Duration) {
        self.tick(wait);
        let at = now_ms().saturating_add(wait.as_millis() as u64);
        self.retry_at_ms.store(at.max(1), Ordering::Relaxed);
    }

    /// The loop is about to wait with no deadline, on purpose -- its
    /// source is switched off and only a wake will start it again.
    ///
    /// Cleared by the next [`Task::tick`].
    pub fn park(&self) {
        self.due_mono_ms.store(0, Ordering::Relaxed);
        self.parked.store(true, Ordering::Relaxed);
        self.retry_at_ms.store(0, Ordering::Relaxed);
    }

    /// Record one cycle made of several pieces of work.
    ///
    /// Any failure fails the cycle -- naming the first reason -- because
    /// a loop that refreshes one queue and not the other is not doing its
    /// job. Otherwise any success is a success. A cycle that asked for
    /// nothing changes neither count: it would be a lie to call it a
    /// success, and a worse one to call it a failure.
    ///
    /// Returns whether the task is NOW degraded, as [`Task::failed`] does.
    pub fn record_cycle(&self, outcomes: &[Cycle]) -> bool {
        if let Some(why) = outcomes.iter().find_map(|o| match o {
            Cycle::Failed(why) => Some(why.clone()),
            _ => None,
        }) {
            return self.failed(why);
        }
        if outcomes.contains(&Cycle::Ok) {
            self.ok(now_ms());
        }
        false
    }

    /// Record a successful cycle.
    ///
    /// Resets the consecutive count and NOT `last_error` -- see
    /// [`TaskHealth::last_error`] for why the reason outlives the
    /// recovery.
    pub fn ok(&self, now_ms: u64) {
        self.consecutive.store(0, Ordering::Relaxed);
        self.last_success_ms.store(now_ms, Ordering::Relaxed);
    }

    /// Record a failed cycle. Returns whether the task is NOW degraded.
    ///
    /// The return value is what the caller emits an event on, and it is
    /// deliberately the threshold rather than the raw failure: emitting
    /// on every failure would put a banner up for a blip.
    pub fn failed(&self, why: impl Into<String>) -> bool {
        let n = self.consecutive.fetch_add(1, Ordering::Relaxed) + 1;
        self.total.fetch_add(1, Ordering::Relaxed);
        // A poisoned lock must not take down a loop whose entire design
        // is "never fatal". The count is the load-bearing part and it is
        // in an atomic; losing one error string is survivable.
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(why.into());
        }
        n >= FAILURES_BEFORE_DEGRADED
    }

    /// What the page should show.
    pub fn snapshot(&self) -> TaskHealth {
        self.snapshot_at(mono_ms())
    }

    /// [`Task::snapshot`] against a given monotonic instant, so a test can
    /// look past a deadline without sleeping through it.
    fn snapshot_at(&self, mono_now: u64) -> TaskHealth {
        let consecutive = self.consecutive.load(Ordering::Relaxed);
        let last = self.last_success_ms.load(Ordering::Relaxed);
        let tick = self.last_tick_ms.load(Ordering::Relaxed);
        let due = self.due_mono_ms.load(Ordering::Relaxed);
        let stalled = due != 0 && mono_now > due;
        let retry_at = self.retry_at_ms.load(Ordering::Relaxed);
        TaskHealth {
            task: self.name.to_string(),
            consecutive_failures: consecutive,
            total_failures: self.total.load(Ordering::Relaxed),
            last_error: self.last_error.lock().ok().and_then(|s| s.clone()),
            // 0 is the never-succeeded sentinel, not a 1970 timestamp.
            last_success_ms: (last != 0).then_some(last),
            last_tick_ms: (tick != 0).then_some(tick),
            stalled,
            degraded: consecutive >= FAILURES_BEFORE_DEGRADED || stalled,
            parked: self.parked.load(Ordering::Relaxed),
            retry_at_ms: (retry_at != 0).then_some(retry_at),
        }
    }
}

/// Separate a successful bounded pass from a scheduling stop or real fault.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum IndexOutcome {
    Healthy,
    Deferred,
    Paused,
    #[default]
    Busy,
    Cancelled,
    TimedOut,
    Invalidated,
    Failed(String),
}

impl IndexOutcome {
    pub fn from_index(result: Result<&crate::claude::search::Indexed, &str>) -> Self {
        use crate::claude::transcript::PopulationStatus;
        let done = match result {
            Ok(done) => done,
            Err(error) => return Self::Failed(error.to_owned()),
        };
        // Faults take precedence over a later cancellation or deadline.
        let faults = done.all_unreadable();
        if !faults.is_empty() {
            return Self::Failed(faults.join("; "));
        }
        match done.population.status {
            PopulationStatus::Failed => Self::Failed("Transcript indexing failed".into()),
            PopulationStatus::Cancelled => Self::Cancelled,
            PopulationStatus::TimedOut => Self::TimedOut,
            PopulationStatus::Invalidated => Self::Invalidated,
            PopulationStatus::Running | PopulationStatus::Deferred => Self::Deferred,
            PopulationStatus::Complete if done.remaining > 0 => Self::Deferred,
            PopulationStatus::Complete => Self::Healthy,
        }
    }

    /// Return true only when an actual fault warrants a degradation event.
    pub fn record_health(&self, task: &Task, now_ms: u64) -> bool {
        match self {
            Self::Healthy | Self::Deferred => {
                task.ok(now_ms);
                false
            }
            Self::Failed(reason) => task.failed(reason.clone()),
            _ => false,
        }
    }
}

/// The health sampler loop, which writes a row every 60 s.
pub static HEALTH_SAMPLER: Task = Task::new("health-sampler");

/// The Claude Code live pass.
pub static CLAUDE_LIVE: Task = Task::new("claude-live");

/// The transcript content indexer, which rides along on the live pass
/// (#1203).
///
/// Its OWN task rather than folded into `claude-live`, because the two
/// fail independently and mean different things. The live pass failing
/// means the session list is stale; the indexer failing means SEARCH
/// coverage has stopped growing, and a user searching a corpus whose
/// index quietly stopped advancing would read a partial answer as a
/// settled one. Counting an index failure as a live-pass failure would
/// also let a healthy live pass reset the consecutive count and hide a
/// permanently stuck indexer.
pub static CLAUDE_INDEX: Task = Task::new("claude-index");

/// The GitHub poll loop: both PR queues, on the configured cadence
/// (#1937).
pub static GITHUB_POLL: Task = Task::new("github-poll");

/// The 15 s clock that advances incomplete GitHub queue traversals
/// between full ticks (#1935, #1937).
pub static GITHUB_CONTINUATION: Task = Task::new("github-continuation");

/// The GitLab poll loop (#1937).
pub static GITLAB_POLL: Task = Task::new("gitlab-poll");

/// The one-second Claude session activity nudge (#1477, #1937).
pub static CLAUDE_ACTIVITY: Task = Task::new("claude-activity");

/// The Jira status refresh (#2036): linked and assigned issues into the
/// cache every ten minutes, window open or not. Parked while Jira is off.
pub static JIRA_REFRESH: Task = Task::new("jira-refresh");

/// Every tracked task, for the command.
///
/// Still a fixed list rather than a registry, now of eight: every
/// long-running loop, plus the transcript indexer that rides on the live
/// pass and fails independently of it. A registry would let a loop
/// register itself and so make the list correct by construction -- but
/// only for a loop that remembered to register, which is the same
/// remembering this list needs. What keeps it complete instead is
/// `invariants.rs`' `every_background_task_is_reported_and_recorded`,
/// which finds every `Task` static in the tree and fails if one is
/// missing here or is never written to.
pub fn snapshot_all() -> Vec<TaskHealth> {
    vec![
        HEALTH_SAMPLER.snapshot(),
        CLAUDE_LIVE.snapshot(),
        CLAUDE_INDEX.snapshot(),
        GITHUB_POLL.snapshot(),
        GITHUB_CONTINUATION.snapshot(),
        GITLAB_POLL.snapshot(),
        CLAUDE_ACTIVITY.snapshot(),
        JIRA_REFRESH.snapshot(),
    ]
}

/// Tell the frontend a task just crossed into degraded, so the notice
/// appears now rather than on the next 30 s health poll.
///
/// Only on the threshold -- callers pass what [`Task::failed`] or
/// [`Task::record_cycle`] returned -- for the reason `FAILURES_BEFORE_DEGRADED`
/// gives. A stall cannot announce itself (the stuck loop is the one that
/// would have to), so stalls reach the page through that poll alone.
pub fn announce_degraded<R: tauri::Runtime>(app: &tauri::AppHandle<R>, task: &Task, crossed: bool) {
    if crossed {
        use tauri::Emitter;
        let _ = app.emit("background-degraded", task.snapshot());
    }
}

/// Milliseconds on the monotonic clock since this process first asked.
///
/// `Instant`, not wall time, for the reason the module docs give: it is
/// the clock the loops' own sleeps run on, so a suspended machine pauses
/// both together.
fn mono_ms() -> u64 {
    static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    BASE.get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

/// Milliseconds since the epoch.
///
/// Saturating rather than unwrapping: a machine whose clock is before
/// 1970 is not a reason to panic a background loop.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh task per test: the real ones are `static` and would carry
    /// state between tests, which is the cross-talk that makes a suite
    /// pass in one order and fail in another.
    fn task() -> Task {
        Task::new("t")
    }

    #[test]
    fn scheduling_outcomes_preserve_health_without_fabricating_success() {
        let task = task();
        task.failed("disk full");
        let before = task.snapshot();
        for _ in 0..5 {
            for outcome in [
                IndexOutcome::Paused,
                IndexOutcome::Busy,
                IndexOutcome::Cancelled,
                IndexOutcome::TimedOut,
                IndexOutcome::Invalidated,
            ] {
                assert!(!outcome.record_health(&task, 123));
                assert_eq!(task.snapshot(), before);
            }
        }
        assert!(IndexOutcome::Failed("still full".into()).record_health(&task, 124));
        assert!(!IndexOutcome::Healthy.record_health(&task, 125));
        assert_eq!(task.snapshot().last_success_ms, Some(125));
    }

    #[test]
    fn successful_bounded_pass_breaks_consecutive_faults() {
        let task = task();
        IndexOutcome::Failed("read failure".into()).record_health(&task, 1);
        IndexOutcome::Deferred.record_health(&task, 2);
        assert_eq!(task.snapshot().last_success_ms, Some(2));
        assert!(!IndexOutcome::Failed("new failure".into()).record_health(&task, 3));
        assert_eq!(task.snapshot().consecutive_failures, 1);
        assert_eq!(task.snapshot().total_failures, 2);
    }

    #[test]
    fn real_faults_win_over_later_scheduling_stops() {
        use crate::claude::{search::Indexed, transcript::PopulationStatus};
        for status in [
            PopulationStatus::Cancelled,
            PopulationStatus::TimedOut,
            PopulationStatus::Invalidated,
        ] {
            let mut done = Indexed::default();
            done.population.status = status;
            assert!(!matches!(
                IndexOutcome::from_index(Ok(&done)),
                IndexOutcome::Failed(_)
            ));
            done.unreadable.push("permission denied".into());
            assert_eq!(
                IndexOutcome::from_index(Ok(&done)),
                IndexOutcome::Failed("permission denied".into())
            );
        }
        assert_eq!(
            IndexOutcome::from_index(Err("database locked")),
            IndexOutcome::Failed("database locked".into())
        );
    }

    #[test]
    fn a_new_task_has_never_succeeded_and_never_failed() {
        // `None`, not a zero timestamp. A loop that has not started is
        // a different state from one that succeeded in 1970.
        let s = task().snapshot();
        assert_eq!(s.last_success_ms, None);
        assert_eq!(s.last_error, None);
        assert_eq!(s.consecutive_failures, 0);
        assert!(!s.degraded);
    }

    #[test]
    fn one_failure_is_not_degraded() {
        // One blip is weather. `poll.rs` applies the same threshold for
        // the same reason: a failure that fixes itself before the user
        // finishes reading the banner is not worth a banner.
        let t = task();
        assert!(!t.failed("boom"));
        assert!(!t.snapshot().degraded);
        assert_eq!(t.snapshot().consecutive_failures, 1);
    }

    #[test]
    fn the_second_consecutive_failure_is_degraded() {
        let t = task();
        t.failed("boom");
        assert!(t.failed("boom again"), "the threshold must be crossed here");
        assert!(t.snapshot().degraded);
    }

    #[test]
    fn a_success_clears_degraded_but_keeps_the_reason() {
        // "It failed 40 times and then recovered" is worth reading, and
        // clearing the reason on recovery is how a user ends up with a
        // gap in a chart and nothing that explains it.
        let t = task();
        t.failed("disk full");
        t.failed("disk full");
        t.ok(1_700_000_000_000);

        let s = t.snapshot();
        assert!(!s.degraded);
        assert_eq!(s.consecutive_failures, 0);
        assert_eq!(s.last_error.as_deref(), Some("disk full"));
        assert_eq!(s.last_success_ms, Some(1_700_000_000_000));
    }

    #[test]
    fn the_total_survives_a_success_although_the_consecutive_count_does_not() {
        // The two answer different questions: "is it broken now" and
        // "has it been unreliable". A loop failing every other minute
        // keeps resetting the first and is plainly not healthy.
        let t = task();
        t.failed("a");
        t.ok(1);
        t.failed("b");
        t.ok(2);

        let s = t.snapshot();
        assert_eq!(s.consecutive_failures, 0);
        assert_eq!(s.total_failures, 2);
    }

    #[test]
    fn a_task_that_only_ever_failed_reports_no_success_rather_than_zero() {
        // `None` with a non-zero failure count is a THIRD state (#1042):
        // not "working", not "not started", but "has never once
        // succeeded". Rendering it as a 1970 timestamp would be a
        // confident wrong answer.
        let t = task();
        t.failed("x");
        t.failed("x");
        let s = t.snapshot();
        assert_eq!(s.last_success_ms, None);
        assert!(s.degraded);
        assert_eq!(s.total_failures, 2);
    }

    #[test]
    fn the_latest_reason_replaces_the_previous_one() {
        let t = task();
        t.failed("first");
        t.failed("second");
        assert_eq!(t.snapshot().last_error.as_deref(), Some("second"));
    }

    #[test]
    fn every_real_loop_is_reported_and_named_distinctly() {
        // The command returns this list; two entries sharing a name
        // would make the page unable to say which loop is broken.
        //
        // Three since #1203. The content indexer is its own entry
        // rather than part of `claude-live` because the two fail
        // independently and mean different things: a failed live pass
        // means the session list is stale, a failed index means SEARCH
        // COVERAGE has stopped growing. Folded together, a healthy live
        // pass would reset the consecutive count every minute and hide
        // a permanently stuck indexer -- and a user searching a corpus
        // whose index quietly stopped would read a partial answer as a
        // settled one.
        //
        // Seven since #1937: the GitHub poll, the queue continuation
        // clock, the GitLab poll and the Claude activity nudge were
        // untracked, so the page said "healthy" while PR polling stopped.
        //
        // Eight since #2036: the Jira status refresh.
        let all = snapshot_all();
        assert_eq!(all.len(), 8);
        let names: Vec<&str> = all.iter().map(|t| t.task.as_str()).collect();
        for name in [
            "health-sampler",
            "claude-live",
            "claude-index",
            "github-poll",
            "github-continuation",
            "gitlab-poll",
            "claude-activity",
            "jira-refresh",
        ] {
            assert!(names.contains(&name), "{name} missing from {names:?}");
        }
        let mut uniq = names.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), names.len());
    }

    #[test]
    fn a_loop_past_its_deadline_is_stalled_and_degraded() {
        // #1937: a loop stuck on an `.await` records no failure at all.
        // The deadline it set before waiting is what gives it away.
        let t = task();
        assert!(!t.snapshot().stalled, "not started is Pending, not stalled");
        t.ok(now_ms());
        t.tick(std::time::Duration::from_secs(60));
        let due = t.due_mono_ms.load(Ordering::Relaxed);
        let on_time = t.snapshot_at(due);
        assert!(!on_time.stalled && !on_time.degraded);
        assert!(on_time.last_tick_ms.is_some());
        let late = t.snapshot_at(due + 1);
        assert!(late.stalled);
        assert!(late.degraded, "a stall is degraded with zero failures");
        assert_eq!(late.consecutive_failures, 0);
    }

    #[test]
    fn a_parked_loop_is_not_stalled() {
        let t = task();
        t.tick(std::time::Duration::from_secs(1));
        assert!(!t.snapshot().parked);
        t.park();
        assert!(!t.snapshot_at(u64::MAX).stalled);
        assert!(t.snapshot().parked, "and says it is parked");
        // The next cycle re-arms the deadline.
        t.tick(std::time::Duration::from_secs(1));
        assert!(t.snapshot_at(u64::MAX).stalled);
    }

    #[test]
    fn a_told_to_wait_loop_says_when_it_retries_until_its_next_cycle() {
        let t = task();
        assert_eq!(t.snapshot().retry_at_ms, None, "not told to wait");
        let before = now_ms();
        t.retry_in(std::time::Duration::from_secs(90));
        let at = t.snapshot().retry_at_ms.expect("retrying");
        assert!(at >= before + 90_000, "{at}");
        assert!(!t.snapshot().parked);
        // The wait plus grace is still the deadline: not stalled early.
        let due = t.due_mono_ms.load(Ordering::Relaxed);
        assert!(!t.snapshot_at(due).stalled);
        t.tick(std::time::Duration::from_secs(1));
        assert_eq!(t.snapshot().retry_at_ms, None, "the ordinary cadence again");
        t.retry_in(std::time::Duration::from_secs(1));
        t.park();
        assert_eq!(t.snapshot().retry_at_ms, None, "parked is not retrying");
    }

    #[test]
    fn a_cycle_that_asked_nothing_is_neither_success_nor_failure() {
        // #1050's rule at the loop level: a declined or switched-off cycle
        // did not succeed and did not fail.
        let t = task();
        assert!(!t.record_cycle(&[Cycle::NotAsked, Cycle::NotAsked]));
        let s = t.snapshot();
        assert_eq!(s.last_success_ms, None);
        assert_eq!(s.total_failures, 0);
    }

    #[test]
    fn one_failed_queue_fails_the_cycle_and_names_its_reason() {
        let t = task();
        assert!(!t.record_cycle(&[Cycle::Ok, Cycle::Failed("review queue: 502".into())]));
        assert_eq!(t.snapshot().consecutive_failures, 1);
        assert_eq!(
            t.snapshot().last_error.as_deref(),
            Some("review queue: 502")
        );
        assert!(t.record_cycle(&[Cycle::Failed("timed out".into()), Cycle::NotAsked]));
        assert!(!t.record_cycle(&[Cycle::Ok, Cycle::NotAsked]));
        assert_eq!(t.snapshot().consecutive_failures, 0);
        assert!(t.snapshot().last_success_ms.is_some());
    }

    #[test]
    fn now_ms_is_after_2024_and_is_not_the_sentinel() {
        // 0 is the never-succeeded sentinel, so a clock that produced it
        // would make a real success read as "never".
        let n = now_ms();
        assert!(n > 1_700_000_000_000, "{n}");
    }
}
