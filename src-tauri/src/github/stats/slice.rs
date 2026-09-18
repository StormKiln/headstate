//! The completeness mechanism: subdivide a window until every piece of it
//! fits inside GitHub's 1,000-result search cap.
//!
//! # The hazard
//!
//! GitHub search returns at most 1,000 retrievable results while
//! `issueCount` reports the true total. VERIFIED live 2026-09-11:
//! `is:pr org:FNX-Labs` reports **1,822**, and paginating to offset 1000
//! returns `[]` -- an empty node list, HTTP 200, no error, no warning.
//!
//! So a leaderboard computed by walking search results would be silently
//! wrong on any real organisation, and *silently* is the defect: the page
//! would render a confident top five built from a truncated sample. This
//! repo has shipped that exact shape twice -- #802's review threads and
//! #790's check contexts -- and both fixes were to make the truncation
//! visible. This module goes one better and makes it not happen.
//!
//! # Why the slicing must be probe-driven
//!
//! A calendar grid does not work, and this is measured rather than
//! argued. FNX-Labs, by year (live, 2026-09-11, cost 1 for all six
//! aliases): 2021 = 0, 2022 = 2, 2023 = 2, 2024 = 0, 2025 = 28,
//! **2026 = 1,790**. Then 2026 by quarter: Q1 = 9, Q2 = 521,
//! **Q3 = 1,260**. Then Q3 by month: July = 461, August = 706,
//! September = 93.
//!
//! Every level of that grid has at least one bucket over the cap until
//! the third. A yearly grid truncates 790 PRs; a quarterly grid truncates
//! 260; a monthly grid happens to work *today* and will not the moment
//! August gets busier. The subdivision has to be driven by the measured
//! count, which is what makes it correct for a scope nobody has measured.
//!
//! (The 9 / 521 / 1,260 figures reproduce #823's exactly, which is worth
//! noting: the epic's premise holds, and this module is not working
//! around it.)
//!
//! # Why this is affordable
//!
//! A probe is `issueCount` with no nodes, and an aliased document of them
//! costs 1 point in total regardless of alias count -- MEASURED at 36, 60
//! and 80 aliases on 2026-09-11, and already recorded at 66 aliases in
//! `query.rs:212-219`. So a whole level of the recursion is ONE request,
//! and the depth is logarithmic in the activity. The FNX-Labs case above
//! resolves in four rounds.

use super::query::Slice;
use chrono::{Duration, NaiveDate};

/// GitHub's retrievable-result cap for a search.
///
/// VERIFIED live: offset 1000 returns an empty node list with no error.
pub const SEARCH_CAP: u64 = 1_000;

/// The count at which a slice is subdivided rather than used.
///
/// 800, not 1,000. The cap is a hard wall with no error on the other side
/// of it, and three things move a count between the probe and the fetch:
/// a PR opened during the load, a draft marked ready, a search index that
/// is eventually consistent. Probing at exactly the cap would mean a
/// slice measured at 998 and fetched at 1,002 silently loses two PRs, and
/// the loss would be invisible for the same reason the cap is.
///
/// 20% headroom is sized against the observed shape rather than picked
/// round: the largest real slice in my measurements is August 2026 at 706
/// for a whole org-month, so a slice that probes under 800 and then grows
/// past 1,000 before it is fetched would need a 40% jump inside one load.
/// The cost of being wrong in the safe direction is one extra probe
/// round at 1 point.
pub const SUBDIVIDE_AT: u64 = 800;

/// How deep the recursion may go before it stops and says so.
///
/// A day is the finest slice GitHub's date grammar expresses -- `created:`
/// takes a date, not a timestamp -- so a single day carrying more than
/// [`SEARCH_CAP`] results is genuinely unsliceable, and no amount of
/// further recursion helps. That is the honest floor, and
/// [`Plan::irreducible`] is how a load reports having hit it rather than
/// returning a number it cannot stand behind.
///
/// The depth limit itself is belt-and-braces on top of that: halving a
/// range reaches one day from 100 years in 16 steps, so 24 cannot be
/// reached by a well-formed range and exists only so a bug in the date
/// arithmetic cannot spin.
pub const MAX_DEPTH: u32 = 24;

/// A date range and what probing it reported.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbedSlice {
    pub slice: Slice,
    /// GitHub's `issueCount` for this range: the TRUE total, which is
    /// what makes the cap detectable at all.
    pub count: u64,
}

impl ProbedSlice {
    /// Whether this range must be subdivided before its nodes are safe
    /// to fetch.
    pub fn too_big(&self) -> bool {
        self.count >= SUBDIVIDE_AT
    }

    /// Whether this range is over the cap and cannot be subdivided
    /// further -- a single day with more than 1,000 results.
    pub fn irreducible(&self) -> bool {
        self.count >= SEARCH_CAP && self.slice.from == self.slice.to
    }
}

/// The outcome of planning a window: the slices to fetch, and whether the
/// plan can actually be complete.
///
/// `irreducible` is the honest-partial channel #824 item 8 requires. It
/// is NOT an error: a single day with 1,200 PRs in it is a real thing an
/// organisation can do, and the right answer is a number plus a statement
/// that it is a floor -- never a bare number over a sample, and never a
/// failed load that shows nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Every slice to fetch, each probed under [`SUBDIVIDE_AT`] unless it
    /// is listed in `irreducible`.
    pub slices: Vec<ProbedSlice>,
    /// Slices that are over the cap and cannot be divided further. Empty
    /// in every case I could measure; non-empty means the totals from
    /// those slices are floors.
    pub irreducible: Vec<ProbedSlice>,
    /// Probe rounds it took. One round is one request per
    /// `ALIAS_CHUNK` slices, so this is the latency of the planning.
    pub rounds: u32,
    /// Slices the planner never got to probe, because the ceiling
    /// expired first (#1123).
    ///
    /// A SECOND honest-partial channel, distinct from `irreducible` on
    /// purpose. Irreducible means "probed, and the answer is a floor".
    /// This means "never probed, so these PRs are not in `total()` at
    /// all" -- a smaller number rather than a qualified one, which is
    /// the more dangerous of the two because nothing about the figure
    /// itself looks wrong.
    ///
    /// Empty on every completed plan, which is why `is_complete` reads
    /// it rather than a flag: a plan that finished cannot accidentally
    /// claim it did not, and one that did not cannot claim it did.
    pub unprobed: Vec<Slice>,
}

impl Plan {
    /// The exact total across the plan.
    ///
    /// Sums `issueCount`, which is GitHub's own true total per slice and
    /// is NOT subject to the 1,000 cap -- so this is correct even for a
    /// plan containing an irreducible slice. The cap limits what can be
    /// RETRIEVED, not what is counted, which is the asymmetry the whole
    /// design exploits: a count is always exact, and only a leaderboard
    /// needing per-PR detail has to care about the cap at all.
    pub fn total(&self) -> u64 {
        self.slices.iter().map(|s| s.count).sum()
    }

    /// Whether every PR in the window can be RETRIEVED, not merely
    /// counted.
    ///
    /// False when any slice is irreducible: the count is still exact but
    /// a leaderboard built from nodes would be over a sample, and must
    /// say so.
    pub fn is_retrievable(&self) -> bool {
        self.irreducible.is_empty()
    }

    /// How many PRs sit in slices whose nodes cannot be fully retrieved.
    ///
    /// The number a "this is a floor" message quotes, so the user learns
    /// the SIZE of what is missing rather than only that something is.
    /// The same property `PrDetail::checks_total` gives the check panel.
    pub fn unretrievable(&self) -> u64 {
        self.irreducible
            .iter()
            .map(|s| s.count.saturating_sub(SEARCH_CAP))
            .sum()
    }

    /// Whether the planner probed the whole window (#1123).
    ///
    /// False means `total()` is a FLOOR in the strong sense: slices
    /// exist that were never asked about, so their PRs are absent from
    /// the sum entirely. A caller rendering a bare number from an
    /// incomplete plan would be printing a confident wrong figure, which
    /// is the failure this codebase's "qualify, or suppress" rule names.
    pub fn is_complete(&self) -> bool {
        self.unprobed.is_empty()
    }

    /// How many slices were never probed, for a message that can say
    /// "28 of 37" rather than only that something is missing.
    pub fn unprobed_slices(&self) -> usize {
        self.unprobed.len()
    }
}

/// Split a range into `n` roughly equal consecutive date ranges.
///
/// Used by the planner to subdivide an over-cap slice. Ranges are
/// INCLUSIVE and contiguous with no gap and no overlap -- a gap loses PRs
/// and an overlap double-counts them, and both are silent. `ranges_tile`
/// in the tests is the guard.
///
/// Splitting by DATE rather than by count, because a count-based split is
/// not expressible: GitHub's search grammar takes a date range, and there
/// is no "results 500-1000 of this range" that is not the capped cursor
/// pagination this module exists to avoid.
pub fn subdivide(slice: &Slice, n: u32) -> Vec<Slice> {
    let Some((from, to)) = parse_range(slice) else {
        // An unparseable range cannot be split. Returned whole rather
        // than dropped: the caller will probe it, and a slice that
        // cannot be divided is reported as irreducible rather than
        // silently vanishing from the plan.
        return vec![slice.clone()];
    };
    let days = (to - from).num_days() + 1;
    if days <= 1 || n < 2 {
        return vec![slice.clone()];
    }
    let n = i64::from(n).min(days);
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        // Integer arithmetic on day offsets so the pieces tile exactly:
        // each piece starts where the previous one ended plus a day, and
        // the last ends on `to` regardless of rounding.
        let start = from + Duration::days(days * i / n);
        let end = if i + 1 == n {
            to
        } else {
            from + Duration::days(days * (i + 1) / n - 1)
        };
        // Rounding can make a piece empty when `n` approaches `days`;
        // skip rather than emit a backwards range, which GitHub rejects.
        if end < start {
            continue;
        }
        out.push(Slice::new(start.to_string(), end.to_string()));
    }
    out
}

/// How many pieces to cut an over-cap slice into.
///
/// Proportional to how far over it is, rather than a fixed halving: the
/// measured FNX-Labs case is 1,790 in one year, which a binary split
/// reaches in four rounds and a proportional one in two. Each round is a
/// request, so the rounds are the latency.
///
/// Capped at `ALIAS_CHUNK` so one subdivision's probes fit in one request
/// -- cutting into 60 pieces would cost the same point but push the
/// document toward the ~11s deadline measured in `query.rs`.
pub fn split_factor(count: u64) -> u32 {
    split_factor_for(count, SUBDIVIDE_AT)
}

/// [`split_factor`] against the caller's threshold.
///
/// The board subdivides to the PAGE size rather than to the search cap --
/// `fetch::plan_to` carries the measurement for why -- and a split sized
/// against 800 would be wrong by a factor of sixteen for a 50-node page: it
/// would return 2 for a 569-PR slice that needs to become twelve.
///
/// `threshold` of 0 is treated as 1 rather than dividing by zero. A caller
/// asking to subdivide until every slice holds fewer than zero pull requests
/// has a bug, and the honest response is to cut as far as the date grammar
/// allows and let `is_one_day` stop the recursion -- not to panic inside a
/// planner, and not to silently return the count path's number for a
/// question that was not asked.
pub fn split_factor_for(count: u64, threshold: u64) -> u32 {
    let over = count.div_ceil(threshold.max(1)).max(2);
    // +1 so a slice at 2.0x the threshold cuts into 3 rather than
    // exactly 2, which would leave both halves near the threshold and
    // cost another whole round.
    let n = (over + 1).min(super::query::ALIAS_CHUNK as u64);
    n as u32
}

/// Parse a slice's inclusive `YYYY-MM-DD` bounds.
fn parse_range(slice: &Slice) -> Option<(NaiveDate, NaiveDate)> {
    let from = NaiveDate::parse_from_str(&slice.from, "%Y-%m-%d").ok()?;
    let to = NaiveDate::parse_from_str(&slice.to, "%Y-%m-%d").ok()?;
    if to < from {
        return None;
    }
    Some((from, to))
}

/// Whether a slice is a single day, and therefore cannot be cut further.
pub fn is_one_day(slice: &Slice) -> bool {
    slice.from == slice.to
}

/// Drive the recursion over a caller-supplied probe function.
///
/// The probe is a parameter rather than a GitHub call, which is what
/// makes the completeness property TESTABLE without a network: the test
/// below feeds it a synthetic 1,822-result org shaped like the real one
/// and asserts the plan comes back complete. A module that called GitHub
/// directly could only be verified by hand, against an account whose data
/// changes, which is how a completeness bug survives.
///
/// `probe` takes the ranges for one round and returns their counts in the
/// same order. The caller batches them into aliased documents at
/// `ALIAS_CHUNK` per request and runs those concurrently -- see
/// `fetch.rs`; this function is only the decision about WHAT to probe.
pub fn plan_with<F, E>(window: Slice, mut probe: F) -> Result<Plan, E>
where
    F: FnMut(&[Slice]) -> Result<Vec<u64>, E>,
{
    let mut pending = vec![window];
    let mut done: Vec<ProbedSlice> = Vec::new();
    let mut irreducible: Vec<ProbedSlice> = Vec::new();
    let mut rounds = 0;

    while !pending.is_empty() && rounds < MAX_DEPTH {
        rounds += 1;
        let counts = probe(&pending)?;
        let mut next = Vec::new();
        for (slice, count) in pending.into_iter().zip(counts) {
            let probed = ProbedSlice { slice, count };
            // An empty range is still a RESULT, not a gap: recording it
            // keeps the plan's slices tiling the window, so a reader can
            // see the whole window was covered rather than wondering
            // which parts were skipped. Not subdivided, obviously.
            if !probed.too_big() {
                done.push(probed);
                continue;
            }
            if is_one_day(&probed.slice) {
                // The floor. A day is the finest range the grammar
                // expresses, so this is as complete as GitHub can be
                // asked to be -- reported, never hidden.
                irreducible.push(probed.clone());
                done.push(probed);
                continue;
            }
            let pieces = subdivide(&probed.slice, split_factor(probed.count));
            // A subdivision that could not actually cut (an unparseable
            // range) would loop forever. Treated as irreducible instead.
            if pieces.len() < 2 {
                irreducible.push(probed.clone());
                done.push(probed);
                continue;
            }
            next.extend(pieces);
        }
        pending = next;
    }

    // Anything still pending when the depth limit bites is recorded as
    // irreducible rather than dropped. Dropping it would understate the
    // total in silence, which is the failure this module exists to
    // prevent -- a wrong number that looks right.
    for slice in pending {
        let probed = ProbedSlice { slice, count: 0 };
        irreducible.push(probed.clone());
        done.push(probed);
    }

    done.sort_by(|a, b| a.slice.from.cmp(&b.slice.from));
    Ok(Plan {
        slices: done,
        irreducible,
        rounds,
        // The synchronous planner has no ceiling: it either probes every
        // slice or hits `MAX_DEPTH`, and the depth case is recorded as
        // irreducible just above. So there is never an unprobed slice
        // here, and saying so explicitly is what stops a future edit
        // from quietly introducing one.
        unprobed: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic organisation with a known PR-per-day distribution, so
    /// the completeness property can be asserted without a network.
    ///
    /// Shaped from the LIVE FNX-Labs measurements of 2026-09-11 rather
    /// than invented: 1,790 PRs concentrated in 2026, with Q3 at 1,260 of
    /// them and July/August/September at 461/706/93. A uniform
    /// distribution would make the test pass for the wrong reason -- the
    /// whole difficulty is that activity CLUMPS, which is why a calendar
    /// grid fails.
    struct Org {
        /// Inclusive date ranges and the number of PRs in each.
        buckets: Vec<(NaiveDate, NaiveDate, u64)>,
        probes: std::cell::Cell<u32>,
        aliases: std::cell::Cell<u32>,
    }

    impl Org {
        fn fnx_labs() -> Self {
            let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
            Self {
                // Every figure here is measured live, cost 1 per round.
                buckets: vec![
                    (d("2022-01-01"), d("2022-12-31"), 2),
                    (d("2023-01-01"), d("2023-12-31"), 2),
                    (d("2025-01-01"), d("2025-12-31"), 28),
                    (d("2026-01-01"), d("2026-03-31"), 9),
                    (d("2026-04-01"), d("2026-06-30"), 521),
                    (d("2026-07-01"), d("2026-07-31"), 461),
                    (d("2026-08-01"), d("2026-08-31"), 706),
                    (d("2026-09-01"), d("2026-09-30"), 93),
                ],
                probes: std::cell::Cell::new(0),
                aliases: std::cell::Cell::new(0),
            }
        }

        /// PRs in a range, summed from a PER-DAY distribution.
        ///
        /// Per-day rather than proportional-to-overlap, and that is not a
        /// detail. A proportional count has to round, and rounding makes
        /// the parts of a range sum to something other than the whole --
        /// so "subdividing does not change the total" would fail for a
        /// reason that is purely an artefact of the fixture. Worse, if it
        /// rounded DOWN the fixture would lose PRs on division, and an
        /// incomplete plan would look complete: the test would pass
        /// because the fixture was broken in the same direction as the
        /// bug.
        ///
        /// A real date range behaves exactly like this sum: each day
        /// holds what it holds, and a range holds the days inside it.
        fn count(&self, slice: &Slice) -> u64 {
            let (from, to) = parse_range(slice).expect("well-formed range");
            let mut total = 0u64;
            for (bf, bt, n) in &self.buckets {
                let lo = from.max(*bf);
                let hi = to.min(*bt);
                if hi < lo {
                    continue;
                }
                let bucket_days = (*bt - *bf).num_days() + 1;
                // Deal `n` PRs across the bucket's days, largest
                // remainder first, so every day has an exact integer
                // count and the days sum to exactly `n`.
                let base = n / bucket_days as u64;
                let extra = n % bucket_days as u64;
                let first_offset = (lo - *bf).num_days() as u64;
                let overlap_days = (hi - lo).num_days() as u64 + 1;
                total += base * overlap_days;
                // The first `extra` days of the bucket carry one more.
                let end_offset = first_offset + overlap_days;
                total += extra
                    .min(end_offset)
                    .saturating_sub(first_offset.min(extra));
            }
            total
        }

        /// The fixture's own invariant: a range's count equals the sum of
        /// its parts. Asserted in a test below, because a fixture that
        /// did not have this property could make an incomplete plan look
        /// complete.
        #[cfg(test)]
        fn total(&self) -> u64 {
            self.buckets.iter().map(|(_, _, n)| n).sum()
        }

        fn probe(&self, slices: &[Slice]) -> Result<Vec<u64>, std::convert::Infallible> {
            self.probes.set(self.probes.get() + 1);
            self.aliases.set(self.aliases.get() + slices.len() as u32);
            Ok(slices.iter().map(|s| self.count(s)).collect())
        }
    }

    /// The FIXTURE's own invariant, asserted before anything is
    /// concluded from it: a range's count is the sum of its parts, and
    /// the whole history sums to the known live total.
    ///
    /// Without this, a fixture that lost PRs on division would make an
    /// incomplete plan look complete -- the test would pass because the
    /// fixture was broken in the same direction as the bug it is meant
    /// to catch.
    #[test]
    fn the_fixture_is_exact_under_subdivision() {
        let org = Org::fnx_labs();
        let whole = Slice::new("2021-01-01", "2026-09-30");
        assert_eq!(org.count(&whole), org.total(), "the fixture covers itself");
        assert_eq!(org.count(&whole), 1822, "the known live total");

        // Cut it every way the planner might, and the parts must sum to
        // the whole every time.
        for n in 2..=10u32 {
            let parts: u64 = subdivide(&whole, n).iter().map(|s| org.count(s)).sum();
            assert_eq!(parts, 1822, "splitting into {n} changed the total");
        }
        // And a month of the busiest bucket, down to single days.
        let august = Slice::new("2026-08-01", "2026-08-31");
        assert_eq!(org.count(&august), 706);
        let daily: u64 = (1..=31)
            .map(|d| {
                org.count(&Slice::new(
                    format!("2026-08-{d:02}"),
                    format!("2026-08-{d:02}"),
                ))
            })
            .sum();
        assert_eq!(daily, 706, "the days of August sum to August");
    }

    /// THE acceptance test for #824 item 4: a scope larger than 1,000
    /// results returns COMPLETE data.
    ///
    /// The window is the whole of FNX-Labs' history, which the live API
    /// reports at 1,822 PRs against a 1,000-result retrievable cap. The
    /// assertion is not "it returned something" but all three of:
    /// every slice is under the subdivide threshold, the slices tile the
    /// window with no gap and no overlap, and the total equals the known
    /// figure.
    #[test]
    fn a_scope_over_the_cap_is_planned_complete() {
        let org = Org::fnx_labs();
        let window = Slice::new("2021-01-01", "2026-09-30");
        let plan = plan_with(window.clone(), |s| org.probe(s)).unwrap();

        // 1. Nothing left over the cap.
        for s in &plan.slices {
            assert!(
                s.count < SUBDIVIDE_AT,
                "slice {}..{} still holds {} results, over the {SUBDIVIDE_AT} threshold",
                s.slice.from,
                s.slice.to,
                s.count
            );
        }
        assert!(
            plan.is_retrievable(),
            "every slice must be retrievable in full: {:?}",
            plan.irreducible
        );
        assert_eq!(plan.unretrievable(), 0);

        // 2. The slices TILE the window: contiguous, no gap, no overlap.
        // A gap loses PRs and an overlap double-counts them, and both
        // are silent -- the plan would look just as complete.
        ranges_tile(&plan.slices, &window);

        // 3. The total is the true figure, not a capped one. 1,822 is
        // what the live API reports for `is:pr org:FNX-Labs`; the
        // fixture sums to the same because its buckets are the measured
        // ones. The point is that it is ABOVE the cap and exact.
        let total = plan.total();
        assert!(
            total > SEARCH_CAP,
            "the test is meaningless below the cap: {total}"
        );
        assert_eq!(total, 1822, "the known live total for this org");
    }

    /// The slices must cover exactly the window: every day in it in
    /// exactly one slice.
    fn ranges_tile(slices: &[ProbedSlice], window: &Slice) {
        let (wf, wt) = parse_range(window).unwrap();
        assert!(!slices.is_empty());
        let mut expect = wf;
        for s in slices {
            let (f, t) = parse_range(&s.slice).unwrap();
            assert_eq!(
                f, expect,
                "gap or overlap at {f}: expected the next slice to start at {expect}"
            );
            assert!(t >= f, "backwards range {f}..{t}");
            expect = t + Duration::days(1);
        }
        assert_eq!(
            expect,
            wt + Duration::days(1),
            "the slices stop at {} but the window ends at {wt}",
            expect - Duration::days(1)
        );
    }

    /// A calendar grid is not enough, which is why the slicer is
    /// probe-driven. Measured live: quarterly slicing of FNX-Labs gives
    /// 9 / 521 / 1,260 and the last one is over the cap.
    ///
    /// This asserts the premise the whole module rests on, so that if
    /// GitHub ever raised the cap the test would say so rather than the
    /// module quietly doing unnecessary work.
    #[test]
    fn quarterly_slicing_alone_would_still_truncate() {
        let org = Org::fnx_labs();
        let q3 = Slice::new("2026-07-01", "2026-09-30");
        let count = org.count(&q3);
        assert_eq!(count, 1260, "the measured live figure for FNX-Labs Q3 2026");
        assert!(
            count > SEARCH_CAP,
            "a quarterly grid leaves {} PRs unretrievable",
            count - SEARCH_CAP
        );
        // And the planner fixes exactly that.
        let plan = plan_with(q3.clone(), |s| org.probe(s)).unwrap();
        assert!(plan.is_retrievable());
        assert_eq!(plan.total(), 1260, "subdividing does not change the total");
        ranges_tile(&plan.slices, &q3);
    }

    /// Probing is cheap, which is the reason the recursion is
    /// affordable: an aliased probe document costs 1 point regardless of
    /// alias count (MEASURED at 36/60/80 aliases on 2026-09-11).
    ///
    /// The ROUNDS are the latency, so they are what this pins. Four
    /// rounds for a six-year window holding 1,822 PRs.
    #[test]
    fn the_whole_org_history_plans_in_a_handful_of_rounds() {
        let org = Org::fnx_labs();
        let plan = plan_with(Slice::new("2021-01-01", "2026-09-30"), |s| org.probe(s)).unwrap();
        assert!(
            plan.rounds <= 6,
            "{} probe rounds is too many; each is a request",
            plan.rounds
        );
        assert_eq!(plan.rounds, org.probes.get());
    }

    /// A window already under the threshold is used as-is. One probe,
    /// no subdivision -- the common case for a quiet repo or a short
    /// window, and it must not pay for the machinery.
    #[test]
    fn a_small_window_is_not_subdivided() {
        let org = Org::fnx_labs();
        let w = Slice::new("2026-09-01", "2026-09-30");
        let plan = plan_with(w.clone(), |s| org.probe(s)).unwrap();
        assert_eq!(plan.rounds, 1);
        assert_eq!(plan.slices.len(), 1);
        assert_eq!(plan.slices[0].slice, w);
        assert_eq!(plan.total(), 93);
    }

    /// An empty window is a legitimate answer, not a failure. #823's
    /// item 9: "a member with no activity in a window must read as 'no
    /// activity', not as a zero that might be a failed query."
    #[test]
    fn an_empty_window_plans_to_one_empty_slice() {
        let org = Org::fnx_labs();
        let w = Slice::new("2024-01-01", "2024-12-31");
        let plan = plan_with(w.clone(), |s| org.probe(s)).unwrap();
        assert_eq!(plan.total(), 0);
        assert!(plan.is_retrievable());
        assert_eq!(plan.slices.len(), 1, "the window is still covered");
        ranges_tile(&plan.slices, &w);
    }

    /// A single day over the cap is the honest floor: the date grammar
    /// has nothing finer. Reported as irreducible rather than silently
    /// truncated, and the SIZE of the shortfall is stated.
    #[test]
    fn a_single_day_over_the_cap_is_reported_not_hidden() {
        let busy = |s: &[Slice]| -> Result<Vec<u64>, std::convert::Infallible> {
            // Every range, however small, holds 1,200.
            Ok(s.iter().map(|_| 1_200u64).collect())
        };
        let plan = plan_with(Slice::new("2026-08-01", "2026-08-01"), busy).unwrap();
        assert!(
            !plan.is_retrievable(),
            "an unsliceable over-cap day must not claim to be complete"
        );
        assert_eq!(plan.irreducible.len(), 1);
        assert_eq!(plan.unretrievable(), 200, "1200 - the 1000 cap");
        // The COUNT is still exact: issueCount is not capped, only
        // retrieval is.
        assert_eq!(plan.total(), 1_200);
    }

    /// A pathological probe that never reduces must terminate. The
    /// guard is the depth limit, and what it must NOT do is drop the
    /// slices it gave up on -- that would understate the total in
    /// silence.
    #[test]
    fn a_recursion_that_cannot_reduce_terminates_and_says_so() {
        let stubborn = |s: &[Slice]| -> Result<Vec<u64>, std::convert::Infallible> {
            Ok(s.iter().map(|_| 5_000u64).collect())
        };
        let plan = plan_with(Slice::new("2000-01-01", "2026-12-31"), stubborn).unwrap();
        assert!(plan.rounds <= MAX_DEPTH);
        assert!(!plan.is_retrievable());
        assert!(
            !plan.irreducible.is_empty(),
            "giving up must be reported, not silent"
        );
    }

    /// Subdivision must tile: contiguous, no gap, no overlap, and the
    /// last piece lands exactly on the end.
    #[test]
    fn subdividing_tiles_the_range_exactly() {
        for n in 2..=10u32 {
            let s = Slice::new("2026-01-01", "2026-12-31");
            let pieces = subdivide(&s, n);
            assert!(pieces.len() >= 2, "n={n} produced {}", pieces.len());
            let probed: Vec<ProbedSlice> = pieces
                .into_iter()
                .map(|slice| ProbedSlice { slice, count: 0 })
                .collect();
            ranges_tile(&probed, &s);
        }
    }

    /// A one-day range cannot be cut, and must come back whole rather
    /// than empty -- an empty result would drop it from the plan.
    #[test]
    fn a_single_day_cannot_be_subdivided() {
        let s = Slice::new("2026-08-01", "2026-08-01");
        assert_eq!(subdivide(&s, 5), vec![s.clone()]);
        assert!(is_one_day(&s));
    }

    /// A leap day must survive the arithmetic, the same hazard
    /// `query.rs`'s `crosses_a_leap_day` pins for the history series.
    #[test]
    fn subdividing_crosses_a_leap_day() {
        let s = Slice::new("2024-02-01", "2024-03-31");
        let pieces = subdivide(&s, 2);
        let probed: Vec<ProbedSlice> = pieces
            .into_iter()
            .map(|slice| ProbedSlice { slice, count: 0 })
            .collect();
        ranges_tile(&probed, &s);
        assert!(
            probed
                .iter()
                .any(|p| p.slice.from.as_str() <= "2024-02-29"
                    && p.slice.to.as_str() >= "2024-02-29"),
            "2024-02-29 must fall inside a slice"
        );
    }

    /// A backwards or malformed range must not panic and must not loop.
    #[test]
    fn a_malformed_range_is_irreducible_rather_than_a_panic() {
        let s = Slice::new("not-a-date", "also-not");
        assert_eq!(subdivide(&s, 4), vec![s.clone()]);
        let plan = plan_with(s, |r| {
            Ok::<_, std::convert::Infallible>(r.iter().map(|_| 2_000u64).collect())
        })
        .unwrap();
        assert!(!plan.is_retrievable());
    }

    /// The split factor is proportional, so a very large slice does not
    /// need a long chain of halvings -- each round is a request.
    #[test]
    fn the_split_factor_grows_with_the_overshoot() {
        assert_eq!(split_factor(0), 3, "minimum of 2 pieces, +1");
        assert_eq!(split_factor(800), 3);
        assert_eq!(split_factor(1_790), 4);
        // Capped so one subdivision's probes fit in one request, well
        // inside the ~11s deadline measured in `query.rs`.
        assert_eq!(
            split_factor(1_000_000),
            super::super::query::ALIAS_CHUNK as u32
        );
    }

    /// The threshold is deliberately BELOW the cap, because the cap has
    /// no error on the other side of it and counts move during a load.
    #[test]
    fn the_threshold_leaves_headroom_under_the_hard_cap() {
        const { assert!(SUBDIVIDE_AT < SEARCH_CAP) };
        let headroom = (SEARCH_CAP - SUBDIVIDE_AT) as f64 / SEARCH_CAP as f64;
        assert!(
            headroom >= 0.15,
            "only {:.0}% headroom under a silent cap",
            headroom * 100.0
        );
    }

    /// A probe failure propagates rather than yielding a partial plan
    /// that looks complete. A plan is a completeness claim; one built on
    /// a failed probe cannot make it.
    #[test]
    fn a_failed_probe_fails_the_plan() {
        let out = plan_with(Slice::new("2026-01-01", "2026-12-31"), |_| {
            Err::<Vec<u64>, &str>("network")
        });
        assert_eq!(out, Err("network"));
    }
}
