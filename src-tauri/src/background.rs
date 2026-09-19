//! Whether the background loops are still doing their job (#1145).
//!
//! Two long-running loops log their failures and carry on:
//! the health sampler every 60 s, and the Claude live pass. Both are
//! deliberately never fatal -- a pass that could not read `~/.claude`
//! must not stop the loop that also records system health -- and
//! neither had any path to the screen.
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
    /// Whether this crossed the threshold.
    ///
    /// Carried rather than re-derived at each call site, so the page and
    /// any future notifier cannot come to disagree about what counts.
    pub degraded: bool,
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
}

impl Task {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            consecutive: AtomicU64::new(0),
            total: AtomicU64::new(0),
            last_success_ms: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
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
        let consecutive = self.consecutive.load(Ordering::Relaxed);
        let last = self.last_success_ms.load(Ordering::Relaxed);
        TaskHealth {
            task: self.name.to_string(),
            consecutive_failures: consecutive,
            total_failures: self.total.load(Ordering::Relaxed),
            last_error: self.last_error.lock().ok().and_then(|s| s.clone()),
            // 0 is the never-succeeded sentinel, not a 1970 timestamp.
            last_success_ms: (last != 0).then_some(last),
            degraded: consecutive >= FAILURES_BEFORE_DEGRADED,
        }
    }
}

/// The health sampler loop, which writes a row every 60 s.
pub static HEALTH_SAMPLER: Task = Task::new("health-sampler");

/// The Claude Code live pass.
pub static CLAUDE_LIVE: Task = Task::new("claude-live");

/// Every tracked loop, for the command.
///
/// A fixed list rather than a registry: there are two, they are named in
/// `lib.rs`, and a registry would be a lookup table with two entries
/// plus a way to get them wrong.
pub fn snapshot_all() -> Vec<TaskHealth> {
    vec![HEALTH_SAMPLER.snapshot(), CLAUDE_LIVE.snapshot()]
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
    fn both_real_loops_are_reported_and_named_distinctly() {
        // The command returns this list; two entries sharing a name
        // would make the page unable to say which loop is broken.
        let all = snapshot_all();
        assert_eq!(all.len(), 2);
        let names: Vec<&str> = all.iter().map(|t| t.task.as_str()).collect();
        assert!(names.contains(&"health-sampler"), "{names:?}");
        assert!(names.contains(&"claude-live"), "{names:?}");
        let mut uniq = names.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), names.len());
    }

    #[test]
    fn now_ms_is_after_2024_and_is_not_the_sentinel() {
        // 0 is the never-succeeded sentinel, so a clock that produced it
        // would make a real success read as "never".
        let n = now_ms();
        assert!(n > 1_700_000_000_000, "{n}");
    }
}
