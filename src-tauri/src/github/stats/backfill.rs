//! The background worker: pull a small group of days, write them down,
//! and come back for the next group (#1092, #1093, design #1094).
//!
//! # What the user asked for
//!
//! > It should be pulling small groups of information, storing them in the
//! > database, and then fetching the next group over time (keeping in mind
//! > the limit of requests per hour, so it is ok to slowly pull and
//! > backfill the information of a long period of time). It should display
//! > what information it has as it becomes available. And it shouldn't be
//! > requerying for data each time.
//!
//! Four properties. `store::pr_slice` is what makes the fourth true -- a
//! day already retrieved is never requested again. This module is the
//! first three: a small group per tick, written down, over time; and
//! [`Progress`] is what makes the work visible while it happens, which is
//! the third.
//!
//! # Why a separate task, and NOT inside the poll tick
//!
//! `poll::TICK_TIMEOUT` is 45s, and its doc records at length why the
//! margin under `MIN_FOCUSED_SECS` (60) is load-bearing: a tick that
//! overruns overlaps its own successor, and two hung fetches both spend
//! budget on requests that were already useless. Adding a consumer inside
//! that tick spends the margin the margin exists to protect.
//!
//! There is a second reason, and it is the stronger one. `RESERVE` exists
//! to protect the poll loop -- it is the floor a user's stats load must
//! leave behind so the loop's standing obligation survives. Putting the
//! backfill INSIDE the loop that `RESERVE` protects makes the protection
//! self-referential: the consumer being throttled and the thing being
//! protected become the same task.
//!
//! # The floor, and why its `None` arm is the opposite of `permits`'
//!
//! [`BACKFILL_FLOOR`] sits ABOVE `RESERVE`. `RESERVE` protects the poll
//! loop from a load the USER ASKED FOR; this protects both the poll loop
//! and the user's next click from a load NOBODY asked for. Backfill has no
//! deadline -- that is its defining property -- so it is the one consumer
//! that can always yield, and it should yield first.
//!
//! `Budget::permits` treats an unknown remaining figure as permission,
//! because refusing there would fail a user's first load of a session on
//! the absence of information. [`affordable`] treats it as a REFUSAL, and
//! the asymmetry is deliberate: the cost of being wrong here is one
//! minute of waiting, against a foreground load that would simply not
//! happen. A worker that guessed and guessed wrong would spend a starved
//! budget on work nobody is waiting for.
//!
//! # The tick, and the two measurements it rests on
//!
//! One group per tick: up to [`GROUP_SLICES`] day-slices in one detail
//! document, measured at 1 point. At [`BACKFILL_INTERVAL`] that is ~1.3%
//! of the hourly budget, and a 30-day horizon fills in about six minutes.
//!
//! **The first tick probes the whole horizon.** An aliased probe document
//! costs 1 point regardless of alias count -- MEASURED at 36, 60 and 80
//! aliases (`budget::MEASURED_PROBE_COST`) -- so one request buys
//! `issueCount` for every day in the horizon, and the denominator is exact
//! from the first minute rather than growing as the numerator does. That
//! is what lets a board say "1,240 of 2,942" instead of "1,240 of 1,240,
//! complete", and it is why `Coverage::total` can be `Some` long before
//! the rows arrive.

//! # Two of #1094's four open questions are NOT settled here
//!
//! Both need the live API against a real busy organisation, and this
//! change was built without one. Saying so is the point: an unmeasured
//! number presented as a finding is the defect #969 shipped, and #1094
//! asks specifically for measurements rather than guesses.
//!
//! **(3) Does a closed DAY re-fetch identically?** `pr_history`'s module
//! docs record the byte-identical result for a closed 30-day WINDOW --
//! same `issueCount`, same `(repo, number, additions, deletions,
//! mergedAt)` set. It was never taken at day granularity, and the two are
//! not the same claim: a window's total can be stable while a day inside
//! it moves, if GitHub's search index backfills. If a closed day CAN
//! change, then `SliceState::Complete` is not permanent and this design
//! needs a revisit policy.
//!
//! Nothing here depends on the stronger claim being true. A wrong
//! `Complete` costs a stale day, not a corrupt one, and re-fetching a day
//! is ~1 point for five of them -- so the safe response, if it is ever
//! measured and comes back negative, is a re-probe cadence rather than a
//! redesign.
//!
//! **(4) Does backfill traffic slow the foreground?** Unmeasured. The
//! instrumentation to settle it already exists -- the `[diag] cmd
//! stats_board` brackets against these ticks' own `[diag]` lines -- and it
//! needs a session on a real account, not a unit test.
//!
//! So the yield mechanism #1094 sketches in its §3.4(c) is deliberately
//! NOT shipped. Building it would be adding complexity against a cost
//! nobody has observed, and #1094 says to measure before shipping it. What
//! IS shipped is the cheap structural protection: one document per minute,
//! a floor above `RESERVE`, and a task that shares the client's six read
//! permits like any other caller rather than reserving its own.

use super::budget::{self, Budget};
use super::query::Slice;
use super::scope::{Measure, Scope, StatsQuery};
use crate::store::pr_slice::{SliceRow, SliceState};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use std::time::Duration;

/// Remaining points below which the worker does nothing this tick.
///
/// `budget::RESERVE` is 500 and protects the poll loop's standing
/// obligation from a load the user asked for. This is higher, and protects
/// two things at once: that same obligation, and the user's NEXT CLICK --
/// a stats load projected at ~45 points that would be refused at the gate
/// because a background walk nobody requested had already spent the
/// headroom.
///
/// 1,500 rather than a round 1,000 because the thing being preserved is a
/// whole foreground interaction, not a single request: `board_projection`
/// bounds a 90-day board at ~45 points, `stats_count` at ~26, and a page
/// issues five commands. Leaving a thousand points above `RESERVE` means
/// the budget can absorb a page load, a retry, and the poll loop, and
/// still refuse the backfill first -- which is the correct order, because
/// backfill is the only consumer with no deadline.
pub const BACKFILL_FLOOR: u64 = 1_500;

/// How many day-slices one tick asks for.
///
/// `board::BOARD_ALIAS_CHUNK`, because the document this issues IS the
/// board's detail document and the measurement belongs to it: 5 aliases at
/// a 50-node page succeeded 3 of 3 at 4.2-4.9s against the ~11s server
/// deadline on real dense day-slices, where 10 aliases failed 0 of 3.
/// Reusing the constant rather than restating the number keeps one
/// measurement in one place.
pub const GROUP_SLICES: usize = super::board::BOARD_ALIAS_CHUNK;

/// How long between ticks.
///
/// # MEASURED, and the constraint is wall clock rather than points
///
/// #1094 asks for this to be timed so ticks cannot overlap, and the
/// measurement already exists in `board.rs`'s module docs -- taken against
/// day-slices of a real dense organisation (245 merged pull requests
/// across 10 days of `org:FNX-Labs`, 2026-08), three runs per cell:
///
/// | Document | Succeeded | Wall clock |
/// |---|---|---|
/// | 5 aliases x 50 | 3 of 3 | **4.2-4.9s** |
/// | 10 aliases x 50 | 0 of 3 | 10.6s, all 502 |
///
/// So a tick at [`GROUP_SLICES`] costs ~5s of wall clock on the densest
/// data measured, and `fetch::LOAD_TIMEOUT` bounds it at 60s in the worst
/// case where the degradation ladder runs. 60 seconds is therefore the
/// smallest interval at which a tick provably cannot overlap its
/// successor -- the same relationship `poll::TICK_TIMEOUT` maintains
/// against `MIN_FOCUSED_SECS`, and for the same reason: an overlapping
/// tick spends budget twice on work nobody is waiting for.
///
/// The margin over the MEASURED figure is an order of magnitude, which is
/// deliberate. `fetch::LOAD_TIMEOUT`'s own doc states the principle -- "the
/// budget exists to convert an unbounded hang into an actionable error,
/// not to tighten a latency target" -- and a backfill has no latency
/// target at all. Points confirm the choice rather than driving it: one
/// document per minute is ~60 points an hour against a 5,000-point budget,
/// ~1.3%.
pub const BACKFILL_INTERVAL: Duration = Duration::from_secs(60);

/// How far back a scope is walked.
///
/// **MEASURED against the product surface rather than guessed.** The day
/// selector offers exactly `[7, 14, 30]` (`ActivityChart.tsx`'s `RANGES`,
/// whose own doc records 30 as the default because it is "the shortest
/// window in which a monthly cadence of work is visible at all"), and
/// `commands::clamp_days` caps any caller -- including the phone's
/// `remote_call`, which passes raw JSON scalars -- at 90.
///
/// So 30 is not a guess about user behaviour: it is every day a user can
/// currently select. A horizon wider than the widest selectable range
/// would spend the budget filling days no screen can display, which is
/// precisely the "walking a busy org back five years for data nobody
/// opens" failure #1094 warns about.
///
/// What is NOT measured is the distribution of ranges users actually
/// pick, because nothing instruments the selector. If that instrumentation
/// lands and shows a p95 above 30, this should follow it up to but not
/// past `clamp_days`' 90 -- the ceiling is a product decision already
/// made, and a horizon above it could never be rendered.
pub const HORIZON_DAYS: u32 = 30;

/// What the backfill is DOING for a scope, as the page must render it.
///
/// `bool` was not enough states. A page that knows only "running" or
/// "not running" cannot tell a user why nothing is changing, and the
/// reported failure (#1103) was exactly that: a board sat at "0 of 30
/// days measured" for ten minutes while the worker was alive, solvent and
/// deliberately waiting -- indistinguishable, to the reader, from broken.
///
/// Derived from [`TickOutcome`], which already carried every distinction
/// the page needs; it simply never reached the page.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum BackfillPhase {
    /// A request is in flight, or one just advanced the coverage.
    Working,
    /// Solvent and scheduled. The common steady state, and the one the
    /// page had no way to express.
    Waiting,
    /// The rate budget is too low to spend. Carries the remaining figure
    /// when one has been observed; `None` is a cold start, where nothing
    /// has reported a budget yet.
    Paused { remaining: Option<u64> },
    /// Every day in the horizon is covered. Nothing further to do until
    /// the horizon moves.
    Converged,
    /// The last attempt failed. The days stay uncovered and are retried,
    /// so this is distinct from `Paused` -- the wait is not deliberate.
    Stalled,
}

impl TickOutcome {
    /// How this outcome reads to a user.
    ///
    /// The mapping lives here rather than in the emitter so that adding a
    /// `TickOutcome` variant is a compile error until someone decides
    /// what the page should say about it.
    pub fn phase(&self) -> BackfillPhase {
        match self {
            // Nothing has been opened, so nothing is owed an explanation
            // yet -- but the page is still waiting rather than stopped.
            TickOutcome::NoScope => BackfillPhase::Waiting,
            TickOutcome::Skipped { remaining } => BackfillPhase::Paused {
                remaining: *remaining,
            },
            TickOutcome::Complete => BackfillPhase::Converged,
            TickOutcome::Advanced { .. } => BackfillPhase::Working,
            TickOutcome::Failed(_) => BackfillPhase::Stalled,
        }
    }
}

/// What a tick did, for the log and the progress stream.
#[derive(Debug, Clone, PartialEq)]
pub enum TickOutcome {
    /// No scope has been opened, so there is nothing to walk.
    NoScope,
    /// The budget is too low, or nothing has reported one yet.
    ///
    /// Carries the remaining figure when there is one. `None` means the
    /// process has not yet seen a `rateLimit` -- a genuine cold start,
    /// which this worker treats as a reason to wait rather than proceed.
    Skipped { remaining: Option<u64> },
    /// Every day in the horizon is covered. Nothing left to do for this
    /// scope until the horizon moves.
    Complete,
    /// A group was fetched.
    Advanced {
        /// Days this tick newly covered.
        days: usize,
        /// Pull requests written down.
        prs: usize,
    },
    /// The attempt failed. The days stay uncovered and are retried.
    Failed(String),
}

/// Whether the worker may spend this tick.
///
/// **`None` means skip**, which is the opposite of `Budget::permits`'
/// `None` arm and is the point of this function existing separately.
/// `permits` is consulted before a load the user is waiting for, where
/// refusing on the absence of information would fail their first click of
/// a session. Nobody is waiting for this, so an unknown budget is a reason
/// to wait one minute and ask again -- by which time the poll loop will
/// have reported a figure.
pub fn affordable(observed: Option<u64>, projected: u64) -> bool {
    match observed {
        None => false,
        Some(remaining) => remaining.saturating_sub(projected) >= BACKFILL_FLOOR,
    }
}

/// What one tick will spend, before it spends it.
///
/// One probe document plus one detail document, each MEASURED at 1 point
/// (`budget::MEASURED_PROBE_COST`, `budget::MEASURED_DETAIL_CHUNK_COST`) --
/// the probe costs 1 regardless of how many days it asks about, which is
/// what makes probing a whole horizon affordable. Doubled as headroom,
/// because the degradation ladder can reissue a document and a projection
/// that is exactly right has no room to be slightly wrong.
pub const TICK_PROJECTION: u64 = (budget::MEASURED_PROBE_COST + 1) * 2;

/// Where a tick reports what it is doing.
///
/// A trait rather than an `AppHandle`, following `branches::Progress`: the
/// worker then has no dependency on Tauri and its behaviour is testable
/// without a running app. The emitting implementation lives beside the
/// event name in `commands.rs`, which is where every other emitter in this
/// codebase lives.
pub trait Progress: Send + Sync {
    /// A tick began against this scope, with the coverage it starts from.
    ///
    /// `total` is `Option` all the way to the UI. A board with 400
    /// collected and an unknown denominator must never render "400 of 0"
    /// or "400 of 400, complete", and the only way to keep that true is
    /// for the unknown to stay unknown rather than being defaulted at some
    /// layer in between.
    fn tick(&self, report: &Report);
}

/// What the worker knows after a tick, in the shape the UI renders.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// The scope this is about, as `StatsQuery::cache_key` spells it.
    pub scope_key: String,
    /// Days of the horizon that are covered.
    pub days_covered: usize,
    /// Days in the horizon.
    pub days_total: usize,
    /// Pull requests held across the covered days.
    pub collected: u64,
    /// GitHub's exact total for the covered days, or `None` when nothing
    /// has been measured yet.
    ///
    /// **Never defaulted to 0.** See [`Progress::tick`].
    pub total: Option<u64>,
    /// What the worker is doing about this scope right now.
    ///
    /// The page must distinguish "backfill is running" from "backfill has
    /// stopped": a caveat identical in both cases is #1042's indefinite
    /// skeleton at page level, where the reader cannot tell waiting from
    /// broken. A `bool` could not say WHY the wait was happening, which
    /// #1103 showed is the distinction that matters.
    pub phase: BackfillPhase,
    /// When the next tick is due, as a Unix millisecond timestamp.
    ///
    /// From the BACKEND, never computed as `now + BACKFILL_INTERVAL` on
    /// the page: the worker rotates across registered scopes, so any one
    /// scope's next tick is N intervals away. A frontend countdown would
    /// hit zero, nothing would happen, and the page would look broken in
    /// a new way -- with a timer to make it look deliberate.
    ///
    /// `None` when no further tick is scheduled for this scope.
    pub next_tick_at_ms: Option<i64>,
}

/// The days of a horizon ending yesterday, oldest first.
///
/// Ends YESTERDAY, never today. A slice whose `to` is today is a range
/// still being written to: its `issueCount` can grow after it is measured,
/// so a `complete` claim about it would be a claim about a moving target,
/// and the ledger's whole value is that a settled row never needs
/// revisiting. `commands::parse_scope_request` already ends its window
/// yesterday for the same reason.
pub fn horizon_days(now: DateTime<Utc>, days: u32) -> Vec<String> {
    let yesterday = now.date_naive() - ChronoDuration::days(1);
    let start = yesterday - ChronoDuration::days(i64::from(days).saturating_sub(1).max(0));
    let mut out = Vec::new();
    let mut d = start;
    while d <= yesterday {
        out.push(d.to_string());
        d += ChronoDuration::days(1);
    }
    out
}

/// The window a horizon spans, for a coverage read.
pub fn horizon_window(now: DateTime<Utc>, days: u32) -> Option<(String, String)> {
    let all = horizon_days(now, days);
    Some((all.first()?.clone(), all.last()?.clone()))
}

/// Turn a group of days into the slices to ask about.
///
/// One slice per day, which is the floor of GitHub's date grammar and what
/// a dense scope subdivides to anyway. A sparse scope would be better
/// served by month-sized slices -- #1094 notes fetching 30 day-documents
/// for a scope with 2 PRs/month spends 6 points where 1 would do -- and
/// that optimisation is deliberately NOT taken here: it needs the probe
/// counts to decide, and taking it before the ledger has been proven in
/// production would be tuning a mechanism whose correctness is the thing
/// actually at stake.
pub fn day_slices(days: &[String]) -> Vec<Slice> {
    days.iter()
        .map(|d| Slice::new(d.clone(), d.clone()))
        .collect()
}

/// Classify what one probed and fetched slice should be recorded as.
///
/// The three-way decision the ledger stores, made in one place so a caller
/// cannot invent a fourth. `retrieved >= issue_count` with no refusals is
/// the only path to `Complete`; a single day over the cap is
/// `Irreducible`, because subdividing it is not expressible and re-asking
/// would spend the budget on a question with no better answer; everything
/// else is `Refused` and will be asked again.
pub fn classify(slice: &Slice, issue_count: u64, retrieved: u64, refused_fields: u64) -> SliceRow {
    let one_day = slice.from == slice.to;
    let state = if issue_count >= super::slice::SEARCH_CAP && one_day {
        SliceState::Irreducible
    } else if refused_fields == 0 && retrieved >= issue_count {
        SliceState::Complete
    } else {
        SliceState::Refused
    };
    SliceRow {
        from: slice.from.clone(),
        to: slice.to.clone(),
        state,
        issue_count,
        retrieved,
        refused_fields,
    }
}

/// Whether this worker can actually COMPLETE a day for a measure.
///
/// `Board::retrieved_prs` keeps only pull requests with a readable
/// `mergedAt` -- it is what distinguishes a merged pull request from an
/// open one, and the outlier lists and `repo_counts` are drawn from that
/// same population. For `Measure::Merged` the search is `is:merged`, so
/// every node qualifies and `retrieved` can reach `issue_count`.
///
/// For `Measure::Opened` it cannot. The search counts pull requests
/// CREATED in the range, open ones included, so `issue_count` is a
/// superset of what the mapper keeps and a day would be recorded
/// `Refused` however many times it is fetched -- a range the worker
/// returns to forever, spending a point each time and never settling it.
///
/// So an opened scope is not walked, and this is where that is decided
/// rather than at the call site: a future caller who registers one gets
/// the same answer without having to know why. Nothing registers one
/// today (`StatsPage` asks the board for `merged` only), which is exactly
/// why the check belongs here -- the day that changes, the failure would
/// otherwise be a silent, permanent spend on days that never settle.
///
/// The honest fix, when an opened board exists, is for the mapper to keep
/// open pull requests for that measure. That is a change to the board's
/// population and belongs with the feature that needs it, not here.
pub fn walkable(measure: &str) -> bool {
    measure == "merged"
}

/// Rebuild the query a registered scope describes.
///
/// The scalars are stored exactly as `stats_board` receives them, so this
/// reconstructs the same question the user's click asked rather than a
/// parallel encoding of it. An unrecognised kind or measure yields `None`
/// -- a row this build cannot interpret is skipped rather than guessed at,
/// because guessing would file rows under a scope key that does not
/// describe them.
pub fn query_for(scope_kind: &str, scope_value: &str, measure: &str) -> Option<StatsQuery> {
    let scope = match scope_kind {
        "repo" => Scope::Repo(scope_value.to_string()),
        "org" => Scope::Org(scope_value.to_string()),
        "user" => Scope::Personal(scope_value.to_string()),
        // The union the click carried, round-tripped through
        // `scope_value`. A row whose value cannot name a viewer yields
        // `None` -- skipped, like any other row this build cannot
        // interpret, rather than falling back to an unqualified search
        // over the whole of GitHub (#1114).
        "all" => Scope::All(super::scope::AccountScope::parse(scope_value)?),
        _ => return None,
    };
    let measure = match measure {
        "merged" => Measure::Merged,
        "opened" => Measure::Opened,
        _ => return None,
    };
    Some(StatsQuery::new(None, scope, measure))
}

/// Probe every day of a horizon in one document, and record what it says.
///
/// This is the tick that makes the denominator exact from minute one. The
/// probe is `issueCount` with no nodes, and an aliased document of them
/// costs **1 point in total regardless of alias count** -- MEASURED at 36,
/// 60 and 80 aliases. So a 30-day horizon is one request and one point,
/// after which every later tick moves only the numerator.
///
/// The rows written carry `retrieved = 0` and a NON-settled state, because
/// nothing has been retrieved: a `Complete` row with no pull requests
/// behind it is exactly the lying ledger `pr_slice` exists to prevent. A
/// day measured at zero is the one exception and is settled honestly --
/// GitHub said there is nothing there, so there is nothing to fetch.
pub fn probe_rows(slices: &[Slice], counts: &[u64]) -> Vec<SliceRow> {
    slices
        .iter()
        .zip(counts)
        .map(|(slice, &count)| {
            if count == 0 {
                // A MEASURED zero. GitHub was asked and said none, so the
                // day is genuinely covered and no fetch is owed. This is
                // the one place a settled row is written without rows
                // behind it, and it is settled because zero rows is the
                // complete set.
                classify(slice, 0, 0, 0)
            } else {
                SliceRow {
                    from: slice.from.clone(),
                    to: slice.to.clone(),
                    // NOT settled: the count is known, the pull requests
                    // are not. Recording this as complete would make the
                    // worker skip a day it has never fetched.
                    state: SliceState::Refused,
                    issue_count: count,
                    retrieved: 0,
                    refused_fields: 0,
                }
            }
        })
        .collect()
}

/// A budget seeded from what the process has observed.
///
/// The worker's own accumulator starts empty, and `Budget::permits` would
/// then permit anything (its `None` arm). [`affordable`] is consulted
/// instead, against [`budget::observed_remaining`], which is fed by every
/// `rateLimit` this process reads -- including the poll loop's, which runs
/// whether or not a stats page is open.
pub fn tick_budget() -> Budget {
    Budget::new()
}

/// Parse a `YYYY-MM-DD` day.
pub fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// How many days `[from, to]` spans, inclusive.
///
/// The DENOMINATOR of "34 of 90 days measured", so an unreadable bound
/// yields 0 rather than a guess: a denominator nobody can compute must not
/// be invented, for the same reason `Coverage::total` is `Option`. A zero
/// denominator renders as "no days in range", which is visibly wrong and
/// therefore fixable; a guessed one is invisibly wrong.
pub fn days_between(from: &str, to: &str) -> usize {
    let (Some(f), Some(t)) = (parse_day(from), parse_day(to)) else {
        return 0;
    };
    ((t - f).num_days() + 1).max(0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 16, 10, 0, 0).unwrap()
    }

    /// **The `None` arm is a SKIP**, which is the opposite of
    /// `Budget::permits`' and the one thing about this gate most likely to
    /// be "fixed" into agreement with it.
    ///
    /// `permits` permits on `None` because refusing would fail a user's
    /// first load of a session on the absence of information. Nobody is
    /// waiting for a backfill, so the same absence costs one minute.
    #[test]
    fn an_unknown_budget_skips_rather_than_proceeds() {
        assert!(
            !affordable(None, TICK_PROJECTION),
            "an unknown remaining figure must STOP the worker; `permits` \
             takes the opposite arm deliberately, and copying it here \
             would spend a starved budget on work nobody asked for"
        );
        // And the contrast is real, not asserted in the abstract: the
        // foreground gate permits the same unknown.
        assert!(Budget::new().permits(TICK_PROJECTION));
    }

    /// The floor is above `RESERVE`, and it actually refuses there.
    #[test]
    fn the_floor_sits_above_the_poll_loops_reserve() {
        // A `const` block rather than a runtime assertion, matching
        // `pr_history::the_growth_bound_is_stated`: a change that put the
        // floor under `RESERVE` would then fail to COMPILE, which is
        // strictly stronger than failing a test.
        const _: () = assert!(
            BACKFILL_FLOOR > budget::RESERVE,
            "backfill must yield before a load the user asked for does"
        );
        // A budget that a foreground load would still accept is refused
        // here, which is the whole point of a second floor.
        let between = budget::RESERVE + 100;
        assert!(
            !affordable(Some(between), TICK_PROJECTION),
            "a budget above RESERVE but below the backfill floor must \
             refuse the backfill and leave the headroom for a click"
        );
        assert!(Budget::new().permits(TICK_PROJECTION));
    }

    /// A healthy budget is spent.
    #[test]
    fn a_healthy_budget_is_affordable() {
        assert!(affordable(Some(4_500), TICK_PROJECTION));
        assert!(affordable(
            Some(BACKFILL_FLOOR + TICK_PROJECTION),
            TICK_PROJECTION
        ));
        assert!(
            !affordable(Some(BACKFILL_FLOOR + TICK_PROJECTION - 1), TICK_PROJECTION),
            "the boundary is exact rather than approximate"
        );
    }

    /// The horizon ends YESTERDAY. A slice ending today is a range still
    /// being written to, and a `complete` claim about it would be a claim
    /// about a moving target.
    #[test]
    fn the_horizon_ends_yesterday() {
        let days = horizon_days(now(), 3);
        assert_eq!(days, vec!["2026-09-13", "2026-09-14", "2026-09-15"]);
        assert!(
            !days.contains(&"2026-09-16".to_string()),
            "today is still being written to; settling a claim about it \
             would settle a claim about a number that can still grow"
        );
    }

    /// The horizon is the width asked for, oldest first.
    #[test]
    fn the_horizon_is_the_width_asked_for() {
        assert_eq!(horizon_days(now(), 30).len(), 30);
        assert_eq!(horizon_days(now(), 1), vec!["2026-09-15"]);
        let w = horizon_window(now(), 30).unwrap();
        assert_eq!(w.0, "2026-08-17");
        assert_eq!(w.1, "2026-09-15");
    }

    /// The budget gate is a PAUSE, and it says so.
    ///
    /// This is the path the reported failure sat behind (#1103): on a
    /// large account `affordable` returns false, the tick returns before
    /// issuing any request, and the page was told nothing at all -- so a
    /// worker that was alive, solvent and deliberately waiting looked
    /// identical to one that had died.
    #[test]
    fn a_budget_skip_reads_as_paused_and_carries_its_figure() {
        let outcome = TickOutcome::Skipped {
            remaining: Some(1_200),
        };
        assert_eq!(
            outcome.phase(),
            BackfillPhase::Paused {
                remaining: Some(1_200)
            },
            "a skipped tick must say it is paused, and say how much is left"
        );
    }

    /// A cold start is still a pause, with an unknown figure rather than a
    /// zero. Absent is not zero.
    #[test]
    fn a_cold_start_is_paused_with_an_unknown_figure() {
        assert_eq!(
            TickOutcome::Skipped { remaining: None }.phase(),
            BackfillPhase::Paused { remaining: None }
        );
    }

    /// The remaining outcomes each map to their own state.
    ///
    /// Asserted together because the POINT is that they stay distinct: a
    /// mapping that collapsed any two would put the page back to being
    /// unable to say why nothing is changing.
    #[test]
    fn every_outcome_reads_as_its_own_state() {
        assert_eq!(
            TickOutcome::Advanced { days: 5, prs: 40 }.phase(),
            BackfillPhase::Working
        );
        assert_eq!(TickOutcome::Complete.phase(), BackfillPhase::Converged);
        assert_eq!(
            TickOutcome::Failed("boom".into()).phase(),
            BackfillPhase::Stalled
        );
        assert_eq!(TickOutcome::NoScope.phase(), BackfillPhase::Waiting);

        // And specifically: a failure is NOT a pause. One is deliberate
        // and will lift on its own; the other is not and may not.
        assert_ne!(
            TickOutcome::Failed("boom".into()).phase(),
            TickOutcome::Skipped { remaining: None }.phase(),
            "a stall and a pause must not render identically"
        );
    }

    /// A day is only `Complete` when everything GitHub counted arrived.
    #[test]
    fn a_short_day_is_refused_rather_than_complete() {
        let day = Slice::new("2026-08-01", "2026-08-01");
        assert_eq!(classify(&day, 10, 10, 0).state, SliceState::Complete);
        assert_eq!(
            classify(&day, 10, 4, 0).state,
            SliceState::Refused,
            "fewer rows than GitHub counted is not a complete day"
        );
        assert_eq!(
            classify(&day, 10, 10, 2).state,
            SliceState::Refused,
            "a refused field is partiality even when the node count matches"
        );
    }

    /// A single day over the search cap is `Irreducible`, not `Refused`:
    /// the date grammar has no finer unit, so re-asking cannot help and a
    /// worker that retried it forever would spend the budget on a question
    /// with no better answer.
    #[test]
    fn a_day_over_the_cap_is_irreducible_and_not_retried() {
        let day = Slice::new("2026-08-01", "2026-08-01");
        let row = classify(&day, 1_200, 1_000, 0);
        assert_eq!(row.state, SliceState::Irreducible);
        assert!(row.state.settled(), "re-asking cannot improve it");

        // A multi-day range over the cap is NOT irreducible -- it can
        // still be cut.
        let range = Slice::new("2026-08-01", "2026-08-07");
        assert_eq!(classify(&range, 1_200, 1_000, 0).state, SliceState::Refused);
    }

    /// **A probe records the COUNT without claiming the pull requests.**
    ///
    /// This is where the denominator becomes exact from minute one, and
    /// also where it would be easiest to ship a ledger that lies: a probe
    /// that wrote `Complete` would make the worker skip every day it had
    /// merely counted.
    #[test]
    fn a_probe_states_the_count_but_never_claims_the_rows() {
        let slices = day_slices(&["2026-08-01".to_string(), "2026-08-02".to_string()]);
        let rows = probe_rows(&slices, &[42, 7]);
        for row in &rows {
            assert_eq!(row.retrieved, 0);
            assert!(
                !row.state.settled(),
                "a counted day is not a retrieved day; settling it here \
                 would skip a fetch that never happened"
            );
        }
        assert_eq!(rows[0].issue_count, 42, "the count is exact and recorded");
    }

    /// A day GitHub measured at zero IS settled -- zero rows is the
    /// complete set -- and is therefore distinguishable from a day nobody
    /// probed, which has no row at all.
    #[test]
    fn a_probed_zero_day_is_settled_and_a_probed_day_is_not_the_same_as_an_unprobed_one() {
        let slices = day_slices(&["2026-08-01".to_string()]);
        let rows = probe_rows(&slices, &[0]);
        assert_eq!(rows[0].state, SliceState::Complete);
        assert!(rows[0].state.settled());
        assert_eq!(rows[0].issue_count, 0);
        // The contrast that matters: this is a MEASURED zero. A day nobody
        // probed produces no row from this function at all.
        assert!(probe_rows(&[], &[]).is_empty());
    }

    /// The group size is the board's measured chunk, not a second number
    /// that can drift from it.
    #[test]
    fn the_group_is_the_measured_document_size() {
        assert_eq!(GROUP_SLICES, super::super::board::BOARD_ALIAS_CHUNK);
        assert_eq!(GROUP_SLICES, 5);
    }

    /// The interval clears the ceiling on one tick, so ticks cannot
    /// overlap.
    ///
    /// The relationship `poll::TICK_TIMEOUT` maintains against
    /// `MIN_FOCUSED_SECS`, asserted the same way: against the constants
    /// rather than against a comment about them.
    #[test]
    fn a_tick_cannot_overlap_its_successor() {
        assert!(
            BACKFILL_INTERVAL >= super::super::fetch::LOAD_TIMEOUT,
            "a tick bounded at LOAD_TIMEOUT must finish before the next \
             one starts, or two of them spend the budget at once"
        );
    }

    /// The horizon does not exceed what a user can ask to see.
    ///
    /// `clamp_days` caps every caller at 90, so a wider horizon would
    /// fetch days no screen can render.
    #[test]
    fn the_horizon_stays_within_what_a_screen_can_show() {
        const _: () = assert!(
            HORIZON_DAYS <= 90,
            "clamp_days caps a board at 90 days; a wider horizon spends \
             the budget on days nothing can display"
        );
    }

    /// **A group of days is fetched, stored, and never asked for again.**
    ///
    /// The user's fourth requirement -- "it shouldn't be requerying for
    /// data each time" -- asserted end to end against a real mock server
    /// and a real database, through the same `load_detail_chunked` and
    /// `record_with_rows` the worker calls.
    ///
    /// Asserted on REQUESTS ISSUED, which is what #1092 asks for by name:
    /// the answer already converged before this change, the cost did not.
    #[tokio::test]
    async fn a_second_pass_does_not_re_request_a_day_already_stored() {
        use crate::store::pr_slice;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let days: Vec<String> = (1..=5).map(|d| format!("2026-08-{d:02}")).collect();
        let slices = day_slices(&days);

        Mock::given(method("POST"))
            .respond_with(move |req: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                let doc = body["query"].as_str().unwrap_or("").to_string();
                let mut data = serde_json::Map::new();
                for (i, _) in (0..16).enumerate() {
                    let alias = format!("s{i}");
                    if !doc.contains(&format!("{alias}: search(")) {
                        continue;
                    }
                    data.insert(
                        alias,
                        serde_json::json!({
                            "issueCount": 1,
                            "nodes": [{
                                "number": 100 + i,
                                "title": "t",
                                "url": "https://github.com/acme/repo/pull/1",
                                "repository": { "nameWithOwner": "acme/repo" },
                                "author": { "login": "alice" },
                                "createdAt": "2026-08-01T00:00:00Z",
                                // The day the row is filed under, and the
                                // reason a second pass can skip it.
                                "mergedAt": format!("2026-08-0{}T01:00:00Z", i + 1),
                                "additions": 10,
                                "deletions": 2,
                                "changedFiles": 1,
                                "reviews": { "totalCount": 1 },
                            }]
                        }),
                    );
                }
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": data,
                    // `cost` only, NO `remaining`. A mock supplying
                    // `remaining` writes the process-wide
                    // OBSERVED_REMAINING and arms the cross-test race an
                    // async test cannot lock against -- the race that
                    // burned the v5.20.0 tag (#1048). This test does not
                    // need the figure: it drives the fetch and the store,
                    // never the gate, and `affordable` is asserted
                    // directly in its own sync tests above.
                    "extensions": { "rateLimit": { "cost": 1 } }
                }))
            })
            .mount(&server)
            .await;

        let oc = octocrab::Octocrab::builder()
            .base_uri(server.uri())
            .unwrap()
            .personal_token("test-token".to_string())
            .build()
            .unwrap();
        let client = crate::github::client::GitHubClient::new(oc);
        let q = query_for("org", "acme", "merged").unwrap();
        let key = q.cache_key("octocat");

        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();

        // Pass one: every day is uncovered, so every day is asked for.
        let first = pr_slice::uncovered_days(&conn, &key, &days[0], &days[4]).unwrap();
        assert_eq!(first.len(), 5, "nothing stored means nothing covered");

        let budget = Budget::new();
        let map =
            super::super::fetch::load_detail_chunked(&client, &q, &slices, &budget, GROUP_SLICES)
                .await
                .unwrap();
        let prs = super::super::Board::retrieved_prs(&map, &slices);
        for (i, slice) in slices.iter().enumerate() {
            let mine: Vec<_> = prs
                .iter()
                .filter(|p| p.merged_at == slice.from)
                .cloned()
                .collect();
            let count = map[super::super::query::slice_alias(i)]["issueCount"]
                .as_u64()
                .unwrap_or(0);
            let row = classify(slice, count, mine.len() as u64, 0);
            pr_slice::record_with_rows(&mut conn, &key, &row, &mine, Utc::now()).unwrap();
        }
        let issued = server.received_requests().await.unwrap().len();
        assert!(issued >= 1, "pass one must actually fetch");

        // Pass two: the SAME question, now that the days are stored.
        let second = pr_slice::uncovered_days(&conn, &key, &days[0], &days[4]).unwrap();
        assert!(
            second.is_empty(),
            "every day was retrieved and stored, so a second pass must ask \
             for NONE of them -- this is the cost convergence #1092 is \
             about, and it is asserted on what would be requested rather \
             than on the answer, which already converged before this change"
        );

        // And the rows really are readable by a window nobody loaded.
        let held =
            crate::store::pr_history::count(&conn, &key, "2026-07-01", "2026-09-30").unwrap();
        assert_eq!(
            held, 5,
            "a WIDER window reads the narrow pass's rows, because the key \
             is the day and not the question"
        );
        let cov = pr_slice::coverage(&conn, &key, &days[0], &days[4]).unwrap();
        assert_eq!(cov.days_covered(), 5);
        assert_eq!(cov.total, Some(5), "an exact denominator, from the ledger");
    }

    /// An `opened` scope is NOT walked, because a day of it could never
    /// settle.
    ///
    /// The search counts created pull requests including open ones, while
    /// the mapper keeps only those with a `mergedAt` -- so `retrieved`
    /// can never reach `issue_count`, the day records `Refused`, and the
    /// worker returns to it forever at a point a time. Nothing registers
    /// an opened scope today, which is precisely why this is checked in
    /// code rather than left to the caller that might.
    #[test]
    fn an_opened_scope_is_not_walked_because_its_days_could_never_settle() {
        assert!(walkable("merged"));
        assert!(
            !walkable("opened"),
            "an opened day's issueCount counts pull requests the mapper \
             drops, so the day would be refetched forever without ever \
             settling"
        );
        assert!(!walkable("something-else"));
    }

    /// An unknown scope kind or measure is skipped rather than guessed.
    #[test]
    fn an_unreadable_scope_is_skipped_rather_than_guessed() {
        assert!(query_for("org", "X", "merged").is_some());
        assert!(query_for("galaxy", "X", "merged").is_none());
        assert!(query_for("org", "X", "sideways").is_none());
    }

    /// The two measures are different questions, and produce different
    /// keys -- so one backfill's rows can never be served as the other's.
    #[test]
    fn the_two_measures_key_differently() {
        let merged = query_for("org", "X", "merged").unwrap();
        let opened = query_for("org", "X", "opened").unwrap();
        assert_ne!(merged.cache_key("me"), opened.cache_key("me"));
    }
}
