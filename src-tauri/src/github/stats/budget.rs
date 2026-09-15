//! What a scope load COST, accumulated and actually read.
//!
//! The app has always asked GitHub what its requests cost. `PRS_QUERY`,
//! `MERGED_DETAIL_QUERY` and `COUNT_QUERY` all select
//! `rateLimit { cost remaining resetAt }`, and three of the four stats
//! queries do not select it at all -- but the deeper problem is that
//! NOTHING READ `cost`, anywhere, on any query. The one consumer is
//! `client.rs:906-912`, which reads `remaining` and logs a warning below
//! 500. So the app could tell you it was nearly out of budget, and never
//! what had spent it.
//!
//! That is survivable for a poll loop whose spend is a known constant --
//! `poll.rs:1503-1580` pins `PRS_QUERY`'s cost in a CI test precisely so
//! it stays knowable without being measured at runtime. It is not
//! survivable for this feature, where one sidebar click issues a number
//! of requests that depends on how much activity the scope contains. The
//! probe-driven slicer in `slice.rs` cannot be costed in advance by
//! construction: how many slices it takes IS the thing it discovers.
//!
//! # Why an accumulator and not a log line
//!
//! The unit that matters to a user is the CLICK, not the request. "That
//! org cost you 23 points" is actionable; twenty-three separate log lines
//! each saying "cost 1" are not, and at the fan-out this feature
//! reaches they are noise that buries the one line that mattered.
//!
//! So [`Budget`] is handed to a load, threaded through every request it
//! makes, and read once at the end. `remaining` and `reset_at` are kept
//! as the LOWEST and LATEST seen rather than the last: requests run
//! concurrently (see `fetch.rs`), so "the last response to arrive" is a
//! race, and the pessimistic value is the one a refusal should be based
//! on.
//!
//! # On the cost model
//!
//! Read `poll.rs:1503-1580` before adding a field to any stats query. The
//! short version, which that test learned the hard way: cost is driven by
//! NESTED CONNECTIONS, not by the number of searches, and a connection
//! nested inside another is INVISIBLE to a substring count of its parent
//! -- adding `contexts(` inside `statusCheckRollup` took the live cost
//! from 3 to 4 while the guard's count did not move.
//!
//! For this feature specifically: `additions`, `deletions` and
//! `changedFiles` are scalars and cost nothing extra. This module said
//! `reviews { totalCount }` was "a CONNECTION and priced per search" and
//! that a leaderboard adding it "is not free". **Both halves are wrong**,
//! corrected by measurement for #826 -- and correcting them here rather
//! than only where the new code lives, because a wrong fact left in a
//! module doc is the one a future reader finds first.
//!
//! What GitHub prices is the `first:` ARGUMENT, not the connection.
//! MEASURED live 2026-09-11 on the real detail document:
//! `reviews { totalCount }` costs 1 point at 3, 6 and 15 searches, exactly
//! tracking a scalars-only control, while `reviews(first: 1) { totalCount }`
//! costs 2 from 6 searches up. `labels` behaves identically both ways. So a
//! reviewer leaderboard IS free, and `board.rs`'s module docs carry the
//! full table.
//!
//! This narrows `poll.rs:1503-1580` rather than contradicting it: every
//! connection on that test's cost list is a paged one, so its figures were
//! right about the query it measured. The rule to apply before adding a
//! field here is therefore "does it take `first:`", and `MEASURED_*` below
//! records what was measured rather than what was assumed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// GitHub's hourly GraphQL budget, from the `rateLimit` object itself.
///
/// Not a constant the app should rely on -- `Budget` reads the real
/// `remaining` off every response -- but it is the denominator a
/// projection needs before the first response has arrived.
pub const HOURLY_BUDGET: u64 = 5_000;

/// Below this many remaining points, a new scope load is refused.
///
/// The poll loop is the thing being protected, not this feature. It is
/// the only part of the app with a standing obligation: it must keep
/// answering every 60s or the user's PR list goes stale and the tray
/// badge lies. `poll.rs:1503-1580` measures that obligation at well
/// under 500 points/hour for both cadences, and `client.rs:906-912`
/// already treats 500 remaining as the warning line.
///
/// 500 is therefore that same line, reused deliberately: one number for
/// "the budget is nearly gone" rather than two that can disagree. A
/// stats load refused here costs the user a leaderboard they can retry
/// in an hour; a poll loop starved by one costs them the feature the app
/// exists for.
pub const RESERVE: u64 = 500;

/// MEASURED, 2026-09-11, against the live API with `gh api graphql`.
///
/// An aliased `search { issueCount }` document costs 1 point in total
/// regardless of alias count. Confirmed at 36, 60 and 80 aliases, each
/// 3 runs, all `cost: 1` -- which is the fact the whole batching strategy
/// rests on and matches what `query.rs:212-219` recorded at 66 aliases.
pub const MEASURED_PROBE_COST: u64 = 1;

/// MEASURED, 2026-09-11, live: an aliased document of searches taking
/// `first: 100` nodes with `additions deletions changedFiles
/// author { login }` costs **1 point** in total. Confirmed at 12, 24 and
/// 36 aliases over sparse ranges, and at 3 aliases over dense ones (279
/// real PR nodes) -- `cost: 1` every time.
///
/// This is the number that makes a complete leaderboard affordable, and
/// it was worth measuring rather than assuming: 1,200 PR nodes with
/// per-PR diff statistics for one point is not what the
/// `MERGED_DETAIL_QUERY` experience suggests. The difference is that
/// those three fields are SCALARS.
///
/// # A correction to this comment, measured for #826
///
/// It said `MERGED_DETAIL_QUERY` "costs what it costs ... because of its
/// `reviews { totalCount }` and `comments { totalCount }` connections". I
/// ran that query verbatim against the live API, with and without those two
/// fields, 2026-09-11:
///
/// | `first:` | with reviews+comments | without |
/// |---|---|---|
/// | 50 | **cost 1**, 4.26s | **cost 1**, 2.02s |
/// | 100 | **cost 1**, 5.39s | **cost 1**, 2.50s |
///
/// **The cost is 1 either way**, so those connections are not what it costs
/// -- consistent with the module docs above: an unpaged connection read for
/// `totalCount` is free, and both of these are unpaged.
///
/// What they DO cost is LATENCY, and roughly double it: 2.0s to 4.3s at 50
/// nodes, 2.5s to 5.4s at 100. So the observation behind the original
/// comment was real and the attribution was wrong -- it was a latency
/// finding recorded as a cost one, which matters because the two are
/// bounded by different things and mitigated differently.
///
/// # Latency, not cost, is what bounds this shape
///
/// And it is bounded by NODES materialised rather than by aliases -- 3
/// aliases at `first: 100` over dense ranges intermittently 502'd at 11.0s
/// while the same 3 at `first: 50` took 3.4s. See `query::ALIAS_CHUNK` and
/// `fetch::SLICE_PAGE_FULL`, and `board::BOARD_ALIAS_CHUNK` for the figure
/// that made the board's document narrower than the probe's.
pub const MEASURED_DETAIL_CHUNK_COST: u64 = 1;

/// The lowest `remaining` ANY request in this process has been told, or
/// the `u64::MAX` sentinel if none has.
///
/// # Why this exists outside `Budget` (#843)
///
/// `Budget` is per-load by design -- "the unit that matters to a user is
/// the CLICK, not the request" -- and that is the right scope for SPEND.
/// It is the wrong scope for the GATE, and conflating the two is what made
/// `Budget::permits` structurally unable to refuse: the gate asks "is there
/// room left in the hour", which is a property of the process and of the
/// poll loop's standing spend, not of a command that has issued nothing yet.
///
/// Two scopes, two homes. A load reports what IT cost; the gate reads what
/// the HOUR has left.
///
/// # Why a static rather than state threaded through Tauri
///
/// `Budget::record` is the one function that knows where `rateLimit` sits in
/// a response, and it is called from inside spawned tasks holding cloned
/// clients (`fetch.rs`'s `JoinSet`s). Threading a handle to managed state
/// into each of those is reach the feature does not need, and a gate that
/// could be constructed WITHOUT the shared figure would be a gate that can
/// silently go back to always-true -- which is the defect. A static cannot
/// be forgotten at a call site.
///
/// Reset is deliberately absent. GitHub's hourly window rolls over on its
/// own and the next response reports the higher `remaining`, which
/// [`note_remaining`] takes because it tracks the LATEST observation rather
/// than a running minimum -- see its docs for why that differs from
/// `Budget::lowest_remaining`.
static OBSERVED_REMAINING: AtomicU64 = AtomicU64::new(u64::MAX);

/// Record what GitHub said is left, from anywhere in the process.
///
/// # LATEST, not lowest -- the opposite of `Budget::lowest_remaining`
///
/// `Budget` keeps the lowest figure a LOAD saw, because its requests are
/// concurrent and "the last response to arrive" is a race, so a refusal
/// inside one load should rest on the worst thing GitHub said.
///
/// This one keeps the latest, and the difference is the hourly window. A
/// minimum held over the life of the process would latch at whatever the
/// budget was just before a reset and never recover: the window rolls over,
/// GitHub reports 5,000 again, and a stale minimum of 120 would refuse
/// every stats load for the rest of the session. A "remaining" figure is
/// only meaningful as of when it was read, and the most recent read is the
/// most meaningful one.
///
/// That is safe in the direction that matters because the poll loop is the
/// most frequent caller -- every 60-120s, unconditionally
/// (`client.rs:908`) -- so the figure is never stale for long, and the
/// window itself is an hour.
pub fn note_remaining(remaining: u64) {
    OBSERVED_REMAINING.store(remaining, Ordering::Relaxed);
}

/// The lowest remaining budget anything in this process has been told.
///
/// `None` before ANY request anywhere has reported one, which is a genuine
/// cold start and not the same as zero -- the NULL-not-0 rule
/// [`Budget::remaining`] states.
pub fn observed_remaining() -> Option<u64> {
    match OBSERVED_REMAINING.load(Ordering::Relaxed) {
        u64::MAX => None,
        n => Some(n),
    }
}

/// One lock for every TEST that touches [`OBSERVED_REMAINING`], directly or
/// through [`Budget::record`].
///
/// # Why this is crate-visible rather than private to this module's tests
///
/// `OBSERVED_REMAINING` is process-wide by design, and `cargo test` runs test
/// functions on a thread pool -- so ANY test in the crate that calls `record`
/// mutates it, not only the ones in this file. `fetch.rs`'s
/// `a_wave_is_refused_once_the_budget_is_under_the_reserve` does exactly that,
/// and without a shared lock it raced this module's cold-start test: an
/// intermittent failure in one file caused by a test in another, which is the
/// worst kind to diagnose.
///
/// Serialised rather than made injectable per test. An injectable figure is
/// one a production call site could forget to pass, and that is precisely how
/// `permits` came to be always-true (#843).
///
/// Poison is recovered from rather than propagated: a test that panicked while
/// holding this lock has already failed, and turning that into a cascade of
/// unrelated `unwrap` panics in every subsequent test hides the one real
/// failure.
///
/// # This rule did not reach six of its own siblings
///
/// The paragraph above said "directly or through `Budget::record`" from the
/// start, and six tests IN THIS FILE called `record` without the lock anyway:
/// three in the `tests` module and three in `metering`. They mutated
/// `OBSERVED_REMAINING` under `a_seeded_budget_can_actually_refuse`, which
/// reads it -- so the cold-start test failed roughly two runs in three under
/// `--test-threads`, in a file whose own doc comment named the hazard.
///
/// If you add a test here: `record` is not the only reachable path, and
/// "my test does not mention `note_remaining`" is not the question. The
/// question is whether anything it calls can store to `OBSERVED_REMAINING`.
/// Take the lock and capture `RestoreObserved` -- both are cheap, and a test
/// that does not need them loses nothing by holding them.
#[cfg(test)]
pub fn observed_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Serialises tests that hold `fetch::READ_PERMITS`, for the same reason
/// [`observed_test_lock`] exists (#1044).
///
/// `fetch::READ_PERMITS` is a process-wide semaphore of six by design --
/// that is what makes `READ_CONCURRENCY` true of the app rather than of one
/// command -- and process-wide makes it shared state between TESTS as
/// surely as between commands. Two hazards, both observed while building
/// #1044's tests:
///
/// - two tests that stall a request on purpose starve each other, and
///   whichever loses the race for the six permits fails intermittently;
/// - `fetch`'s own `every_stats_read_goes_through_the_process_wide_permit`
///   asserts `available_permits() == READ_CONCURRENCY`, which is false for
///   as long as any other test has a load in flight.
///
/// So every test that takes a permit, and every test that counts them,
/// takes this lock.
///
/// # Why here and not in `fetch.rs`
///
/// `fetch.rs`, `board.rs`, `tree.rs` and `slice.rs` are SCANNED by source
/// tests that split on the first `\n#[cfg(test)]` to find where production
/// ends. A gated helper near the top of one of them silently truncates
/// what those tests scan -- which is exactly what happened when this was
/// first written into `fetch.rs`, and both scans caught it. This file is
/// not scanned, and it already owns the sibling convention.
///
/// # Why a `tokio::sync::Mutex` where [`observed_test_lock`] uses a
/// `std` one
///
/// Because the tests that need THIS one are async, and `std`'s guard
/// cannot be held across an `.await`: clippy's `await_holding_lock` is an
/// error under CI's `cargo clippy -- -D warnings`. `invariants.rs` records
/// that limitation as the reason its own scan skips async tests, and calls
/// giving the lock an async form "a change to `budget.rs`'s public test
/// surface" -- so it is made here, on a new lock, rather than by changing
/// the existing one under the fourteen tests that depend on it.
///
/// The one SYNCHRONOUS caller (`fetch`'s permit-count assertion) is a
/// plain `#[test]` with no runtime of its own, so `blocking_lock` is
/// correct there rather than a panic waiting to happen.
#[cfg(test)]
pub static READ_PERMIT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Restores [`OBSERVED_REMAINING`] on drop, so a test that seeds it cannot
/// leak a figure into whatever runs next.
///
/// A guard rather than a line at the end of each test: an early `panic!` from
/// a failing assertion would skip a manual restore and turn one genuine
/// failure into a cascade of unrelated ones.
#[cfg(test)]
pub struct RestoreObserved(u64);

#[cfg(test)]
impl RestoreObserved {
    pub fn capture() -> Self {
        Self(OBSERVED_REMAINING.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
impl Drop for RestoreObserved {
    fn drop(&mut self) {
        OBSERVED_REMAINING.store(self.0, Ordering::Relaxed);
    }
}

/// What one scope load spent.
///
/// Cheap to clone: the counters are in an `Arc`, so every concurrent
/// request in a load shares one accumulator rather than reporting into
/// separate ones that somebody then has to remember to add up.
#[derive(Debug, Clone)]
pub struct Budget {
    spent: Arc<AtomicU64>,
    requests: Arc<AtomicU64>,
    /// The LOWEST `remaining` any response reported, or `u64::MAX` if
    /// none has. The pessimistic value, because requests are concurrent
    /// and a refusal should be based on the worst thing GitHub said
    /// rather than on whichever response happened to land last.
    lowest_remaining: Arc<AtomicU64>,
    /// Requests that reported no cost -- see [`Budget::unmetered`].
    unmetered: Arc<AtomicU64>,
    /// The LATEST `resetAt` seen, as epoch seconds, or 0.
    ///
    /// Latest rather than first for the same reason: if the window rolled
    /// over mid-load, the later reset is the one a "try again at" message
    /// must quote, or the message tells the user to retry at a time that
    /// has already passed.
    reset_at: Arc<AtomicU64>,
}

impl Default for Budget {
    fn default() -> Self {
        Self::new()
    }
}

impl Budget {
    pub fn new() -> Self {
        Self {
            spent: Arc::new(AtomicU64::new(0)),
            requests: Arc::new(AtomicU64::new(0)),
            lowest_remaining: Arc::new(AtomicU64::new(u64::MAX)),
            unmetered: Arc::new(AtomicU64::new(0)),
            reset_at: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Record one response's `rateLimit` object.
    ///
    /// Takes the whole `data` value rather than three numbers so there is
    /// exactly one place that knows where `rateLimit` sits in a response,
    /// and so a query that FORGOT to select it is visible: the request is
    /// still counted, and `unmetered()` reports the gap.
    ///
    /// A missing `rateLimit` does NOT default `cost` to 1. That would be
    /// a guess recorded as a measurement, which is the failure mode this
    /// whole module exists to fix -- the accumulated total would look
    /// authoritative while containing invented numbers. It counts as
    /// unmetered instead, and says so.
    pub fn record(&self, data: &serde_json::Value) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        let rl = &data["rateLimit"];
        let Some(cost) = rl["cost"].as_u64() else {
            self.unmetered.fetch_add(1, Ordering::Relaxed);
            return;
        };
        self.spent.fetch_add(cost, Ordering::Relaxed);
        if let Some(remaining) = rl["remaining"].as_u64() {
            self.lowest_remaining
                .fetch_min(remaining, Ordering::Relaxed);
            // And into the process-wide figure the GATE reads, so the next
            // load's `permits` knows what this one spent. Without this the
            // only feed is the poll loop, and two stats loads in a row would
            // both be gated on the figure from before the first (#843).
            note_remaining(remaining);
        }
        if let Some(reset) = rl["resetAt"].as_str() {
            if let Ok(t) = chrono::DateTime::parse_from_rfc3339(reset) {
                let secs = t.timestamp().max(0) as u64;
                self.reset_at.fetch_max(secs, Ordering::Relaxed);
            }
        }
    }

    /// Points spent by this load so far.
    pub fn spent(&self) -> u64 {
        self.spent.load(Ordering::Relaxed)
    }

    /// Requests issued by this load so far, metered or not.
    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    /// The lowest `remaining` GitHub reported during this load.
    ///
    /// `None` before any response has carried a `rateLimit` -- which is
    /// genuinely different from "zero remaining" and must not render as
    /// it. The same NULL-not-0 rule `store/schema.rs` migration 8 states
    /// for health samples: "not measured" and "measured as zero" are
    /// opposite answers.
    pub fn remaining(&self) -> Option<u64> {
        match self.lowest_remaining.load(Ordering::Relaxed) {
            u64::MAX => None,
            n => Some(n),
        }
    }

    /// When the hourly window rolls over, if a response said.
    pub fn reset_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        match self.reset_at.load(Ordering::Relaxed) {
            0 => None,
            secs => chrono::DateTime::from_timestamp(secs as i64, 0),
        }
    }

    /// Requests that carried no `rateLimit` object.
    ///
    /// Should be zero: every stats query selects it, and
    /// `tests::every_stats_query_meters_itself` fails CI if one stops.
    /// Non-zero at runtime means a request failed before GitHub answered,
    /// or a query was added without the field -- and either way the
    /// accumulated spend UNDERSTATES the truth, which is the direction
    /// that matters. A reported total must say so rather than look exact.
    ///
    /// COUNTED, not derived. An earlier version of this derived it as
    /// `requests - min(spent, requests)`, on the reasoning that a metered
    /// request costs at least one point so the count of metered requests
    /// cannot exceed the points spent. The bound is true and the
    /// derivation is still wrong: it compares a number of POINTS against a
    /// number of REQUESTS, so one request costing 3 points masks two that
    /// reported nothing, and the load claims to be exactly metered while
    /// two thirds of it was not. A unit confusion is not the thing to
    /// economise a counter on.
    pub fn unmetered(&self) -> u64 {
        self.unmetered.load(Ordering::Relaxed)
    }

    /// Whether there is budget for a load projected to cost `projected`.
    ///
    /// Refuses when the spend would leave less than [`RESERVE`], so the
    /// poll loop keeps working. Item 1 of #824 asks for "refuse, or warn
    /// hard" -- this is the refusal, and [`Spend::pressure`] is the warning.
    ///
    /// # Why this reads a PROCESS-WIDE figure and not only its own (#843)
    ///
    /// This used to be `match self.remaining() { None => true, ... }`, and
    /// that arm was taken at 4 of 4 gates, in every session, permanently.
    /// `lowest_remaining` starts at the `u64::MAX` sentinel, so
    /// `remaining()` is `None` until a response has been recorded -- and
    /// every gate constructs a fresh `Budget` IMMEDIATELY before checking it
    /// (`commands.rs:2317`, `:2640`, `:2741`, `:2890`). A brand-new
    /// accumulator has recorded nothing by construction, so `permits` could
    /// not refuse. The gates looked like refusals and were unconditional
    /// approvals.
    ///
    /// The old doc defended the `None` arm as "the state at the START of the
    /// first load of a session", which is sound for a budget that outlives
    /// its first request. This one does not: it is a per-COMMAND
    /// accumulator consulted once, before request one. And the arm's stated
    /// purpose -- "the one case where `remaining` is already near the floor
    /// because something else spent it" -- names precisely the thing a fresh
    /// `Budget` cannot know.
    ///
    /// So the gate now consults [`observed_remaining`], a process-lifetime
    /// floor fed by every `rateLimit` this process reads -- including the
    /// poll loop's (`client.rs:908`), which is the "something else" in
    /// question and runs every 60-120s whether or not a stats page is open.
    /// Its own accumulator is still consulted and still wins when it is the
    /// lower of the two, because a load that has already driven the budget
    /// down mid-flight is the most current information there is.
    ///
    /// `None` from BOTH -- nothing anywhere in the process has seen a
    /// `rateLimit` yet -- is still permitted, and now means what the old
    /// comment claimed: a genuine cold start, before the poll loop's first
    /// tick. Refusing there would fail the first load of a session on the
    /// absence of information.
    pub fn permits(&self, projected: u64) -> bool {
        // The pessimistic figure: whichever is lower of what this load has
        // seen and what the process has seen. `min` over two `Option`s via
        // `chain`, so "one of them knows" is not the same as "neither does".
        let floor = [self.remaining(), observed_remaining()]
            .into_iter()
            .flatten()
            .min();
        match floor {
            None => true,
            Some(remaining) => remaining.saturating_sub(projected) >= RESERVE,
        }
    }

    /// A snapshot for the UI and the logs.
    /// A budget seeded with a remaining figure, for tests, touching NOTHING
    /// shared (#1050).
    ///
    /// `record` is the production way to get a remaining figure into an
    /// accumulator, but it also calls `note_remaining`, which writes the
    /// process-wide `OBSERVED_REMAINING`. A test that seeds via `record`
    /// therefore mutates state every other test's gate reads -- and an
    /// ASYNC test cannot serialise on `observed_test_lock`, because clippy's
    /// `await_holding_lock` under `-D warnings` refuses a `std::sync` guard
    /// held across an `.await`.
    ///
    /// That combination is exactly the race that burned the v5.20.0 tag
    /// (#1048): a mock supplying `rateLimit.remaining` from an async test.
    /// Seeding the local accumulator alone keeps `permits` -- which takes the
    /// LOWER of this figure and the process one -- refusing as intended,
    /// while leaving the static untouched, so the test needs no lock and
    /// starves nobody.
    #[cfg(test)]
    pub fn seeded_for_test(remaining: u64, reset_at_epoch: u64) -> Self {
        let b = Self::new();
        b.lowest_remaining.store(remaining, Ordering::Relaxed);
        b.reset_at.store(reset_at_epoch, Ordering::Relaxed);
        b
    }

    pub fn snapshot(&self) -> Spend {
        Spend {
            points: self.spent(),
            requests: self.requests(),
            unmetered: self.unmetered(),
            remaining: self.remaining(),
            reset_at: self.reset_at(),
        }
    }
}

/// What a load cost, in the form the frontend and the log both read.
///
/// Serialised camelCase to match every other command return type in this
/// app.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spend {
    /// Rate-limit points this load spent.
    pub points: u64,
    /// Requests it issued.
    pub requests: u64,
    /// Requests whose cost could not be read -- see [`Budget::unmetered`].
    /// Non-zero means `points` is a FLOOR, not a total.
    pub unmetered: u64,
    /// The lowest remaining budget GitHub reported. `None` means nothing
    /// reported one, which is not the same as zero.
    pub remaining: Option<u64>,
    pub reset_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Spend {
    /// Whether `points` is exact or a floor.
    ///
    /// #824's item 8 applied to the budget itself: "anything partial,
    /// capped or assembled from slices says so in its return value".
    /// A spend total assembled from responses, some of which never
    /// reported a cost, is exactly that shape.
    pub fn is_exact(&self) -> bool {
        self.unmetered == 0
    }

    /// How close the budget is to the reserve, 0.0 to 1.0.
    ///
    /// `None` when nothing reported a remaining figure. 1.0 means the
    /// reserve is reached -- the point at which [`Budget::permits`]
    /// starts refusing.
    pub fn pressure(&self) -> Option<f64> {
        let remaining = self.remaining?;
        let usable = HOURLY_BUDGET.saturating_sub(RESERVE) as f64;
        if usable <= 0.0 {
            return Some(1.0);
        }
        let used = HOURLY_BUDGET.saturating_sub(remaining) as f64;
        Some((used / usable).clamp(0.0, 1.0))
    }
}

/// `pub(crate)`: `github::query`'s shape-guard coverage check reads this
/// module's `every_query_document` rather than keeping a second copy of
/// the derivation (#854). `remote::events` does the same for the same
/// reason.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    fn rl(cost: u64, remaining: u64, reset: &str) -> serde_json::Value {
        json!({ "rateLimit": { "cost": cost, "remaining": remaining, "resetAt": reset } })
    }

    #[test]
    fn accumulates_cost_across_requests() {
        let _g = observed_test_lock();
        let _restore = RestoreObserved::capture();
        let b = Budget::new();
        b.record(&rl(1, 4999, "2026-09-11T16:52:14Z"));
        b.record(&rl(4, 4995, "2026-09-11T16:52:14Z"));
        assert_eq!(b.spent(), 5);
        assert_eq!(b.requests(), 2);
        assert!(b.snapshot().is_exact());
    }

    /// Requests in a load are CONCURRENT, so "the last response" is a
    /// race. A refusal has to be based on the worst thing GitHub said,
    /// not on whichever reply won.
    #[test]
    fn remaining_is_the_lowest_seen_not_the_last() {
        let _g = observed_test_lock();
        let _restore = RestoreObserved::capture();
        let b = Budget::new();
        b.record(&rl(1, 900, "2026-09-11T16:52:14Z"));
        b.record(&rl(1, 400, "2026-09-11T16:52:14Z"));
        // Arrives last, reports a HIGHER figure -- a response that was
        // issued earlier and overtaken.
        b.record(&rl(1, 890, "2026-09-11T16:52:14Z"));
        assert_eq!(b.remaining(), Some(400));
    }

    /// If the window rolled over mid-load, the LATER reset is the one a
    /// "try again at" message must quote -- the earlier one has already
    /// passed by the time the user reads it.
    #[test]
    fn reset_at_is_the_latest_seen() {
        let _g = observed_test_lock();
        let _restore = RestoreObserved::capture();
        let b = Budget::new();
        b.record(&rl(1, 100, "2026-09-11T16:00:00Z"));
        b.record(&rl(1, 100, "2026-09-11T17:00:00Z"));
        b.record(&rl(1, 100, "2026-09-11T16:30:00Z"));
        assert_eq!(
            b.reset_at().map(|t| t.to_rfc3339()),
            Some("2026-09-11T17:00:00+00:00".to_string())
        );
    }

    /// "Nothing reported a remaining figure" and "zero remaining" are
    /// opposite answers. Rendering the first as the second would claim
    /// the budget is gone on a cold start.
    ///
    /// # RE-SCOPED, because the old version asserted the defect (#843)
    ///
    /// It used to end with `assert!(b.permits(50))` -- "the absence does not
    /// refuse the first load of a session" -- which pinned the behaviour that
    /// made `permits` structurally unable to refuse at 4 of 4 gates, in every
    /// session, permanently. That assertion is why the defect survived
    /// review: the test said the bug was the intent.
    ///
    /// What is GENUINELY about this accumulator, and is kept, is the
    /// NULL-not-0 rule: a fresh `Budget` has seen nothing, and `None` must
    /// not render as zero. `permits` is no longer a property of a fresh
    /// accumulator alone -- it reads the process-wide figure too -- so it is
    /// tested in `a_seeded_budget_can_actually_refuse` and
    /// `a_cold_start_is_still_permitted` instead, where the process figure
    /// can be set deliberately.
    #[test]
    fn no_response_yet_is_not_zero_remaining() {
        let b = Budget::new();
        assert_eq!(b.remaining(), None);
        assert_eq!(b.reset_at(), None);
        assert_eq!(b.spent(), 0);
        assert_eq!(b.requests(), 0);
        // And a spend of zero requests is exactly metered, not a floor: the
        // `unmetered` counter is about requests that ANSWERED without a cost,
        // and there have been none.
        assert!(b.snapshot().is_exact());
    }

    /// `permits` can refuse in a real session. This is #843's first
    /// acceptance criterion, and the property the old code could not have.
    ///
    /// A FRESH `Budget` is used deliberately -- that is what every gate
    /// constructs (`commands.rs:2317`, `:2640`, `:2741`, `:2890`) -- with the
    /// process-wide figure seeded the way the poll loop seeds it
    /// (`client.rs:908`). Before the fix this took the `None => true` arm and
    /// returned true for every projection including `u64::MAX`.
    ///
    /// Serialised with the other tests that touch `OBSERVED_REMAINING`: it is
    /// process-wide by design, so two tests mutating it in parallel would
    /// race. `cargo test` runs test fns on a thread pool, so the lock is
    /// necessary rather than decorative.
    #[test]
    fn a_seeded_budget_can_actually_refuse() {
        let _g = observed_lock();
        let restore = RestoreObserved::capture();

        // The poll loop has been running for most of the hour and has spent
        // the budget down near the reserve. Nothing a stats load did.
        note_remaining(600);
        let b = Budget::new();
        assert_eq!(
            b.remaining(),
            None,
            "the gate's own accumulator has recorded nothing -- that is the point"
        );
        assert!(b.permits(100), "600 - 100 = 500, exactly the reserve");
        assert!(
            !b.permits(101),
            "a fresh Budget MUST be able to refuse; this is the whole of #843"
        );
        assert!(!b.permits(10_000));

        // And a load already under the reserve refuses everything.
        note_remaining(499);
        assert!(!Budget::new().permits(1));

        drop(restore);
    }

    /// The load's OWN figure still wins when it is the lower of the two.
    ///
    /// A load that has driven the budget down mid-flight holds the most
    /// current information there is, and the process-wide figure may be a
    /// poll-loop reading from a minute ago. Pessimistic, which is the same
    /// rule `remaining_is_the_lowest_seen_not_the_last` states one scope down.
    #[test]
    fn the_lower_of_the_two_figures_is_what_gates() {
        let _g = observed_lock();
        let restore = RestoreObserved::capture();

        // Process says plenty; this load has already spent it down.
        note_remaining(4_900);
        let b = Budget::new();
        b.record(&rl(1, 520, "2026-09-11T16:52:14Z"));
        assert!(
            !b.permits(100),
            "the load's own 520 must gate, not the process-wide 4900"
        );

        // And the other way round: the load looks fine, the process does not.
        // `record` feeds the static too, so the process figure is set
        // explicitly AFTER recording to model a poll tick landing in between.
        let b = Budget::new();
        b.record(&rl(1, 4_800, "2026-09-11T16:52:14Z"));
        note_remaining(520);
        assert!(
            !b.permits(100),
            "the process-wide 520 must gate, not this load's own 4800"
        );

        drop(restore);
    }

    /// A genuine cold start is still permitted -- nothing ANYWHERE in the
    /// process has seen a `rateLimit`, which is the state before the poll
    /// loop's first tick.
    ///
    /// This is the case the old `None => true` arm claimed to be for. It is
    /// now the only case it covers, rather than all four gates forever.
    #[test]
    fn a_cold_start_is_still_permitted() {
        let _g = observed_lock();
        let restore = RestoreObserved::capture();

        OBSERVED_REMAINING.store(u64::MAX, Ordering::Relaxed);
        assert_eq!(observed_remaining(), None);
        let b = Budget::new();
        assert!(
            b.permits(50),
            "refusing on the absence of information would fail the first load \
             of a session"
        );
        // Even an absurd projection: with no information there is nothing to
        // refuse it against, and the first response supplies the real number
        // before the load has spent anything meaningful.
        assert!(b.permits(u64::MAX));

        drop(restore);
    }

    /// `note_remaining` keeps the LATEST figure, not a running minimum, so
    /// the hourly reset recovers.
    ///
    /// A process-lifetime minimum would latch at whatever the budget was just
    /// before the window rolled over and refuse every load for the rest of
    /// the session -- GitHub reporting 5,000 again would never be believed.
    /// That is the opposite of `Budget::lowest_remaining`'s rule, and the
    /// difference is deliberate: one figure is scoped to a set of concurrent
    /// requests, the other to an hour that ends.
    #[test]
    fn the_observed_figure_recovers_after_the_window_rolls_over() {
        let _g = observed_lock();
        let restore = RestoreObserved::capture();

        note_remaining(80);
        assert_eq!(observed_remaining(), Some(80));
        assert!(!Budget::new().permits(1), "under the reserve, so refused");

        // The hour rolls over and GitHub reports a full budget.
        note_remaining(HOURLY_BUDGET);
        assert_eq!(observed_remaining(), Some(HOURLY_BUDGET));
        assert!(
            Budget::new().permits(100),
            "a stale minimum would refuse for the rest of the session"
        );

        drop(restore);
    }

    /// Recording a response feeds the process-wide figure, so a second load
    /// is gated on what the first one spent.
    ///
    /// Without this the only feed is the poll loop, and two stats loads back
    /// to back would both be gated on the figure from before the first.
    #[test]
    fn recording_a_response_feeds_the_process_wide_figure() {
        let _g = observed_lock();
        let restore = RestoreObserved::capture();

        OBSERVED_REMAINING.store(u64::MAX, Ordering::Relaxed);
        Budget::new().record(&rl(4, 530, "2026-09-11T16:52:14Z"));
        assert_eq!(observed_remaining(), Some(530));
        // A DIFFERENT, fresh accumulator -- the next command's gate.
        assert!(!Budget::new().permits(100), "530 - 100 < 500");

        drop(restore);
    }

    /// The lock and the restore guard live at module scope rather than here,
    /// because tests in OTHER files mutate `OBSERVED_REMAINING` too -- any
    /// test calling `Budget::record` does. See `observed_test_lock`.
    use super::{observed_test_lock as observed_lock, RestoreObserved};

    /// A query that forgot `rateLimit` must NOT have its cost guessed at
    /// one point. The guess would be indistinguishable from a measurement
    /// in the accumulated total, which is the exact defect this module
    /// exists to fix.
    #[test]
    fn an_unmetered_response_is_counted_but_not_costed() {
        let _g = observed_lock();
        let _restore = RestoreObserved::capture();
        let b = Budget::new();
        b.record(&rl(3, 4997, "2026-09-11T16:52:14Z"));
        b.record(&json!({ "merged_week": { "issueCount": 7 } }));
        assert_eq!(b.spent(), 3, "no invented cost for the unmetered request");
        assert_eq!(b.requests(), 2);
        assert_eq!(b.unmetered(), 1);
        let s = b.snapshot();
        assert!(
            !s.is_exact(),
            "a total containing an unmetered request is a floor, not a total"
        );
    }

    /// The poll loop must keep working. It is the only part of the app
    /// with a standing obligation, and a leaderboard is never worth
    /// starving it.
    ///
    /// Takes `observed_lock` because `record` now feeds the process-wide
    /// figure as well (#843), so this test MUTATES shared state even though
    /// it reads only its own accumulator. Without the lock it races the tests
    /// that seed that figure deliberately.
    #[test]
    fn refuses_a_load_that_would_eat_the_poll_loops_reserve() {
        let _g = observed_lock();
        let restore = RestoreObserved::capture();
        let b = Budget::new();
        b.record(&rl(1, 600, "2026-09-11T16:52:14Z"));
        assert!(b.permits(100), "600 - 100 = 500, exactly the reserve");
        assert!(!b.permits(101), "601 would breach it");
        assert!(!b.permits(10_000));
        drop(restore);
    }

    /// A load that has already driven the budget under the reserve
    /// refuses even a single further point.
    #[test]
    fn refuses_everything_once_under_the_reserve() {
        let _g = observed_lock();
        let restore = RestoreObserved::capture();
        let b = Budget::new();
        b.record(&rl(1, 499, "2026-09-11T16:52:14Z"));
        assert!(!b.permits(1));
        assert!(!b.permits(0) || RESERVE == 499);
        drop(restore);
    }

    #[test]
    fn pressure_rises_from_zero_to_one_at_the_reserve() {
        let _g = observed_lock();
        let _restore = RestoreObserved::capture();
        let b = Budget::new();
        b.record(&rl(1, HOURLY_BUDGET, "2026-09-11T16:52:14Z"));
        assert_eq!(b.snapshot().pressure(), Some(0.0));

        let b = Budget::new();
        b.record(&rl(1, RESERVE, "2026-09-11T16:52:14Z"));
        assert_eq!(b.snapshot().pressure(), Some(1.0));

        // Past the reserve it clamps rather than exceeding 1.0, so a
        // gauge cannot render past full.
        let b = Budget::new();
        b.record(&rl(1, 0, "2026-09-11T16:52:14Z"));
        assert_eq!(b.snapshot().pressure(), Some(1.0));

        // And unknown stays unknown rather than becoming 0.0, which
        // would render as "plenty of budget" on no information at all.
        assert_eq!(Budget::new().snapshot().pressure(), None);
    }

    /// One accumulator shared across concurrent requests, which is the
    /// whole reason the counters are atomics in an `Arc`.
    #[test]
    fn clones_share_one_accumulator() {
        let _g = observed_lock();
        let _restore = RestoreObserved::capture();
        let b = Budget::new();
        let c = b.clone();
        b.record(&rl(2, 4998, "2026-09-11T16:52:14Z"));
        c.record(&rl(3, 4995, "2026-09-11T16:52:14Z"));
        assert_eq!(b.spent(), 5);
        assert_eq!(c.spent(), 5);
        assert_eq!(b.requests(), 2);
    }

    /// The reserve is the number `client.rs` already warns at. Two
    /// numbers for "the budget is nearly gone" could disagree, and the
    /// user would get a warning from one and a refusal from the other at
    /// different moments.
    #[test]
    fn the_reserve_matches_the_existing_low_budget_warning() {
        let src = include_str!("../client.rs");
        assert!(
            src.contains("remaining < 500"),
            "client.rs no longer warns at 500; RESERVE must move with it"
        );
        assert_eq!(RESERVE, 500);
    }

    /// Every document in both query files, DERIVED from the files rather
    /// than named.
    ///
    /// # Why derivation matters here (#844)
    ///
    /// This guard used to enumerate six names by hand -- and it omitted
    /// `fetch_viewer`'s document, which was an inline string literal in
    /// `client.rs` selecting no `rateLimit` at all. That is the same
    /// list-based blind spot as #842's poll cost guard and #847's missing
    /// shape guards: a hand-written list cannot cover the document nobody
    /// remembered to add to it, and all three defects were exactly that.
    ///
    /// So the list comes from the source. A document in this app is either
    /// a `pub const <NAME>_QUERY` or a `pub fn <name>_query`, both of which
    /// are greppable and neither of which a new document can avoid being
    /// while staying reachable from a caller. `VIEWER_QUERY` is now one
    /// (promoted out of the inline literal for this reason), so it is covered
    /// without being named.
    ///
    /// # What counts as a document
    ///
    /// Any `pub const` or `pub fn` whose name CONTAINS `query` / `QUERY`,
    /// rather than one that ends in it. Ends-with was tried first and was too
    /// narrow by exactly the amount that matters: it matched `history_query`
    /// but not `history_query_range`, `history_query_with_periods` or
    /// `history_query_range_with_periods` -- three of the four builders in
    /// that family, including the one that actually writes the document. A
    /// pattern that covers the delegator and misses the delegate is worse than
    /// no pattern, because it looks like coverage.
    ///
    /// Contains-`query` admits a few non-documents, which is the right
    /// direction to be wrong in: a false positive is a test failure somebody
    /// reads, and a false negative is a document nobody checks. The
    /// delegation exemption below is what keeps the false positives quiet
    /// without silencing anything real.
    ///
    /// Returns `(file, name, body)` per document, the body scoped to the next
    /// top-level item so the field has to be inside THIS document rather than
    /// merely somewhere in a file that has many -- the scoping rule an earlier
    /// version of this test got wrong by anchoring on the first mention of a
    /// name rather than on its definition, then reading the wrong region and
    /// reporting a defect at a location that did not have one.
    /// `pub(crate)` since #854: `github::query`'s shape-guard coverage
    /// check derives the same document list, and a second copy of this
    /// scan would drift from it silently -- which is the whole defect
    /// class this guard was written for.
    ///
    /// `stats/tree.rs` is in the list since #854 as well. It was NOT
    /// before, and that is this derivation's own instance of the bug it
    /// exists to prevent: the file list was two `include_str!` paths, so
    /// `orgs_query` and `tree_query` -- two real documents, one of them
    /// the most expensive read on the scope sidebar -- sat outside the
    /// metering guard for exactly the reason `VIEWER_QUERY` did. Derived
    /// SUBJECTS with an enumerated FILE list is only half a derivation.
    pub(crate) fn every_query_document() -> Vec<(&'static str, String, String)> {
        let files = [
            ("query.rs", include_str!("../query.rs")),
            ("stats/query.rs", include_str!("query.rs")),
            ("stats/tree.rs", include_str!("tree.rs")),
        ];
        let mut out = Vec::new();
        for (file, src) in files {
            for kind in ["pub const ", "pub fn "] {
                let mut at = 0usize;
                while let Some(i) = src[at..].find(kind) {
                    let start = at + i;
                    at = start + kind.len();
                    // The identifier that follows the keyword.
                    let rest = &src[at..];
                    let end_name = rest
                        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                        .unwrap_or(rest.len());
                    let name = &rest[..end_name];
                    if !(name.contains("query") || name.contains("QUERY")) {
                        continue;
                    }
                    // To the next top-level item. `\npub ` rather than
                    // `\npub fn `, so a `pub const` following a `pub fn`
                    // terminates the body too.
                    let body = &src[start..];
                    let end = body[1..].find("\npub ").map_or(body.len(), |j| j + 1);
                    out.push((file, name.to_string(), body[..end].to_string()));
                }
            }
        }
        out
    }

    /// Item 1 of #824: `rateLimit` on EVERY query document, derived rather
    /// than enumerated (#844).
    ///
    /// Asserted on the query source, because no mapper test can tell the
    /// difference -- they feed JSON literals and pass happily against a
    /// document that never asked for the field. This is the same reasoning
    /// `query.rs`'s own
    /// `the_detail_query_asks_for_every_thread_and_its_true_count` gives.
    ///
    /// A document with no `rateLimit` has its spend counted as `unmetered`
    /// rather than guessed, which is honest -- but it makes the reported total
    /// a FLOOR for no reason, and in `fetch_viewer`'s case the request was not
    /// recorded at all, so `points` understated while `is_exact()` still
    /// returned true. That is what `budget.rs:252-255` forbids.
    #[test]
    fn every_stats_query_meters_itself() {
        let docs = every_query_document();
        // The scan is asserted to have FOUND something, so a rename that
        // breaks the pattern fails loudly rather than passing vacuously over
        // an empty list -- which is how a derived guard dies quietly.
        assert!(
            docs.len() >= 10,
            "only {} query documents found; the scan is broken, not the \
             documents",
            docs.len()
        );
        // Builders that DELEGATE: their whole body is a call to another
        // document builder, so the field is selected one level down and
        // asserting on their own text would be asserting on a forwarding
        // line. Checked by looking for a call to another `_query` in the body
        // rather than by naming them, so a new delegator is covered and a
        // delegator that grows a document of its own stops being exempt.
        //
        // This is the derivation's real limitation and it is stated rather
        // than papered over: the scan reads TEXT, so it cannot follow a call.
        // What it can do is tell a forwarding body from a document body,
        // which is enough -- a delegator that does not forward has a
        // document, and a document must meter itself.
        let names: Vec<&str> = docs.iter().map(|(_, n, _)| n.as_str()).collect();
        for (file, name, body) in &docs {
            if body.contains("rateLimit") {
                continue;
            }
            // The body after the signature, so the function's OWN name does
            // not count as a call to itself.
            let after_sig = body.split_once(')').map_or("", |(_, r)| r);
            let delegates_to = names
                .iter()
                .find(|other| **other != name.as_str() && after_sig.contains(&format!("{other}(")));
            assert!(
                delegates_to.is_some(),
                "{file}'s {name} does not select rateLimit and does not delegate \
                 to a document that does, so its cost cannot be read. Either add \
                 `rateLimit {{ cost remaining resetAt }}` -- MEASURED free on \
                 every document in this app, including the viewer lookup \
                 (`query::VIEWER_QUERY`), the detail query and the checks page \
                 -- or record here why this one cannot."
            );
        }
        // `VIEWER_QUERY` specifically, by name, because its ABSENCE from the
        // derived list is the failure this guard could not previously see: it
        // was an inline literal in `client.rs` and matched no pattern at all.
        // If someone inlines it again, the derived scan would silently stop
        // covering it and this is what says so.
        assert!(
            docs.iter().any(|(_, n, _)| n == "VIEWER_QUERY"),
            "VIEWER_QUERY is not a named document any more -- inlining it back \
             into client.rs puts it outside every guard in this file, which is \
             exactly how it came to be unmetered (#844)"
        );
    }

    /// `fetch_viewer`'s point is RECORDED, not merely askable.
    ///
    /// Selecting `rateLimit` is half the fix; the other half is that somebody
    /// calls `record`. The defect was both: the document did not ask, and the
    /// call happened before `Budget::new()` so there was nothing to ask on
    /// behalf of. A stats command calling plain `fetch_viewer` instead of
    /// `fetch_viewer_metered` would reintroduce the second half while the
    /// guard above still passed.
    #[test]
    fn the_stats_commands_meter_their_viewer_lookup() {
        let src = include_str!("../../commands.rs");
        // The stats commands, which are the ones inside a budgeted load.
        // `get_viewer`, the remote gate and startup have no accumulator to
        // report into and are outside this file.
        let stats_region = src
            .find("pub async fn stats_count")
            .expect("stats_count not found in commands.rs");
        let region = &src[stats_region..];
        let plain = region.matches("fetch_viewer()").count();
        assert_eq!(
            plain, 0,
            "{plain} stats command(s) call plain `fetch_viewer()`, which spends \
             a rate-limit point outside any accumulator -- `Spend.points` would \
             understate by one while `is_exact()` returned true (#844). Use \
             `fetch_viewer_metered(&budget)`."
        );
        assert!(
            region.matches("fetch_viewer_metered(&budget)").count() >= 2,
            "both stats commands that resolve the viewer must meter it"
        );
    }
}
