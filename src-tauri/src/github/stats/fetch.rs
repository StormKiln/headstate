//! The execution layer: bounded concurrency, a wall-clock ceiling,
//! connection-first routing, and the degradation ladder inherited from
//! `client.rs` rather than reinvented.
//!
//! `slice.rs` decides WHAT to ask and `budget.rs` records what it cost.
//! This module is the part that actually issues requests, and everything
//! in it exists because its absence has already caused a shipped bug
//! here -- see the table in #824.

use super::budget::Budget;
use super::query::{
    probe_query, slice_detail_query, Slice, ALIAS_CEILING, ALIAS_CHUNK, REPO_CONNECTION_QUERY,
};
use super::scope::{Scope, StatsQuery};
use super::slice::{self, Plan};
use crate::github::client::{ClientError, GitHubClient};
use serde_json::json;

/// How many stats reads may be in flight at once.
///
/// # Why reads need a cap at all
///
/// `BATCH_CONCURRENCY = 4` in `commands.rs:437-444` caps MUTATIONS,
/// because "GitHub applies secondary rate limits to concurrent
/// mutations". Reads have had no equivalent, and the shape that needs one
/// already exists: `fetch_history_values` (`client.rs:947-986`) spawns
/// `days / HISTORY_CHUNK_DAYS` concurrent POSTs, which is **18** at the
/// 90-day clamp. That has been fine because 18 is the ceiling and it is
/// reached only by one user action.
///
/// This feature has no such ceiling. An org-scope leaderboard fans out
/// over probe rounds, then over slices, then potentially over members --
/// the product of three counts that all depend on how much activity the
/// scope contains. Unbounded, a busy org is exactly the "many concurrent
/// requests from one client" shape that secondary limits exist to stop,
/// and a secondary limit is not a clean 403: GitHub's documented
/// behaviour is to start refusing, and octocrab is configured
/// `HandleRateLimits { max_retries: 3, min_wait_seconds: 60 }`
/// (`client.rs:1070-1076`), so each refusal becomes a 60-second wait
/// inside a request that the layer above simply waits on.
///
/// # Why 6
///
/// Bounded from below by the shipped evidence and from above by the
/// mutation cap's reasoning:
///
/// - **Not below 4.** `fetch_history_values` has shipped at 18 concurrent
///   reads without a reported secondary-limit failure, so the real
///   threshold is above 18 and a cap of 1 or 2 would be slower than what
///   already works for no measured reason.
/// - **Not 18.** That number is an accident of `HISTORY_CHUNK_DAYS` and
///   the 90-day clamp, not a measurement of anything. Inheriting it would
///   be inheriting a coincidence.
/// - **6, because the latency it has to hide is ~2-11s per request.** I
///   measured a 10-alias probe document at 1.78-1.81s and the expensive
///   node-heavy document at 3.69-5.21s for 12 aliases (live, 2026-09-11).
///   Six in flight covers 60 slices per round-trip at the probe shape,
///   which plans the whole 1,822-PR FNX-Labs history in one round-trip's
///   worth of wall clock. Going wider buys less and less -- the planner
///   needs at most `ALIAS_CHUNK` x 6 = 60 slices per round and rarely
///   that many -- while the risk it adds is a secondary limit whose
///   penalty is a 60-second stall, not a retry.
///
/// Deliberately HIGHER than the mutation cap of 4 and deliberately
/// stated: a mutation that trips a secondary limit may have partially
/// applied and has to be reasoned about, where a refused read is just a
/// read to do again. The asymmetry is the justification for the two
/// numbers differing rather than being unified.
///
/// # It is enforced PER PROCESS, not per call (#844)
///
/// The waves below enforce it correctly WITHIN one command: each drains its
/// `JoinSet` before the next, so at most six are in flight per call. But six
/// per call is not six, because a Stats page render fires FIVE independent
/// commands with no gating between them (`StatsPage.tsx:126-151`:
/// `useScopedCounts` is 2x `stats_count`, plus `useStatsSeries`,
/// `useStatsBoard`, `useStatsReviewers`). 5 x 6 is ~30 concurrent POSTs.
///
/// That exceeds this module's own safety argument. The evidence above is
/// "`fetch_history_values` has shipped at 18 without a reported
/// secondary-limit failure, so the real threshold is above 18" -- which
/// covers 18, and says nothing about 30. And the penalty it names is severe:
/// octocrab is `min_wait_seconds: 60` against a `LOAD_TIMEOUT` of 60s, so one
/// secondary limit becomes a TIMEOUT rather than a retry.
///
/// So the cap is a process-wide [`READ_PERMITS`] acquired per request, which
/// is what makes the number above true of the app rather than of one command.
pub const READ_CONCURRENCY: usize = 6;

/// The process-wide permits that make [`READ_CONCURRENCY`] real.
///
/// # Why a static semaphore and not a parameter
///
/// The cap has to hold across COMMANDS, and commands have no shared object to
/// hang it on: each `#[tauri::command]` is entered independently with a
/// cloned client. A permit threaded in as an argument would be a permit a new
/// call site could decline to take -- and "the cap is a local variable" is
/// precisely the defect (#844). A static cannot be forgotten.
///
/// Acquired INSIDE each spawned task rather than around a wave, so a wave
/// that is wider than the cap simply queues rather than deadlocking, and so
/// the permit is held for exactly the duration of the POST. `tokio`'s
/// semaphore is fair (FIFO), so one command cannot starve another
/// indefinitely: five commands contending for six permits interleave instead
/// of the first to arrive holding them until it finishes.
///
/// # Why this does not make the page slower than it was
///
/// It changes the page from ~30 concurrent requests to 6, which sounds like
/// a 5x serialisation and is not: the five commands were never independent in
/// wall-clock terms, because they contend for one connection pool and for
/// GitHub's own serial evaluation of search aliases. `client.rs:564`'s
/// diagnostic comment records the same effect for two concurrent searches
/// ("a total far above the slower of the two means they are NOT actually
/// overlapping"), and #844 measured the poll loop's two searches at 3.5s
/// sequential against 5.6s concurrent -- GitHub contends on simultaneous
/// node-heavy queries. Bounded concurrency is what the latency figures in
/// this module were measured under in the first place.
///
/// `const_new` so there is no lazy initialisation and no `OnceLock` to reason
/// about: the permits exist for the life of the process.
static READ_PERMITS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(READ_CONCURRENCY);

/// Run one stats read, holding a process-wide permit for its duration.
///
/// Every POST this module issues goes through here, which is what makes
/// [`READ_CONCURRENCY`] a property of the app rather than of one command.
///
/// The permit is released when the future completes, including on error: it
/// is held in a local that drops at the end of this function. `acquire` fails
/// only if the semaphore has been CLOSED, which nothing closes -- mapped to an
/// error rather than unwrapped, because a panic inside a Tauri command aborts
/// the whole app (`board.rs`'s `total_cmp` comment makes the same call).
/// `pub(super)` so `tree.rs`'s two reads go through it too. Every stats POST
/// in the module has to, or the cap is a property of this file rather than of
/// the feature -- and the sidebar tree is one of the things a Stats page
/// render has in flight (`StatsPage.tsx` reads `useStatsTree`).
pub(super) async fn metered_read(
    client: &GitHubClient,
    budget: &Budget,
    body: serde_json::Value,
) -> Result<serde_json::Value, ClientError> {
    let _permit = READ_PERMITS
        .acquire()
        .await
        .map_err(|_| ClientError::Graphql("the stats read semaphore is closed".into()))?;
    let v = client.stats_graphql(&body).await?;
    // Recorded HERE rather than at each call site, so a new read cannot be
    // added that takes a permit and then forgets to meter itself -- which is
    // the `fetch_viewer` defect in #844, one layer up.
    budget.record(&v);
    Ok(v)
}

/// Wall-clock ceiling on one whole scope load.
///
/// # Why this is not `poll::FETCH_TIMEOUT`
///
/// `poll::FETCH_TIMEOUT` is 30s and bounds ONE fetch. Its own doc
/// (`poll.rs:709-736`) explains the property that sets it: it must be
/// below `MIN_FOCUSED_SECS` so a hung poll lands strictly before the next
/// tick, or two fetches overlap and each spends budget. A stats load has
/// no cadence and no successor to collide with, so that constraint does
/// not apply here and copying 30s would be copying a number for a reason
/// that is not present.
///
/// What DOES apply is the reason it exists at all, and it applies more
/// strongly: octocrab is `HandleRateLimits { max_retries: 3,
/// min_wait_seconds: 60 }` (`client.rs:1070-1076`), so **one POST can
/// legitimately block for minutes** while every layer above it waits. A
/// load issuing dozens of POSTs inherits that per-POST hazard dozens of
/// times over. Only a wall-clock ceiling around the whole thing bounds
/// what the user is actually waiting on -- the same argument
/// `commands::get_pr_detail` makes at `commands.rs:604-633`, and the same
/// one that #790 shipped without.
///
/// # Why 60s
///
/// Derived from what a load legitimately needs, at measured latencies:
///
/// - A probe round is one request per [`ALIAS_CHUNK`] slices, measured at
///   1.78-1.81s for 10 aliases. The FNX-Labs history plans in 4 rounds,
///   so planning is ~8s even when every round is serial (they are, by
///   construction -- each round's slices depend on the last round's
///   counts).
/// - A detail fetch is one request per chunk, measured at 3.42-3.68s for
///   3 aliases at the default 50-node page over the densest slices in this
///   account, and those ARE parallel at [`READ_CONCURRENCY`]. The
///   FNX-Labs plan lands ~16 slices, which is two waves: ~10s.
///
/// So a worst realistic load is ~20-25s, and 60s is between two and three
/// times that. Generous on purpose, for the reason `get_pr_detail`'s
/// comment gives: "the budget exists to convert an unbounded hang into an
/// actionable error, not to tighten a latency target". A real load that
/// needs 40 seconds should succeed.
///
/// Bounded ABOVE by what a user will sit in front of. This is behind an
/// explicit load (#823 item 4), so the user asked and is watching; a
/// ceiling past a minute is a ceiling nobody waits for.
pub const LOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Nodes per slice on the first attempt, and after degrading.
///
/// The ladder is `client.rs:1024-1050`'s, inherited rather than
/// reinvented: `MERGED_DETAIL_QUERY` halves `first: 100` to 50 when the
/// server gives up, because the three diff-statistic fields are ~60% of
/// its measured 6.5s and a reported log showed 124
/// `RESOURCE_LIMITS_EXCEEDED` errors on that shape.
///
/// # The default is 50, not 100, and that is a MEASUREMENT
///
/// This layer multiplies that query by its alias count, so I measured the
/// real document against dense real data -- three month-slices of
/// `org:FNX-Labs` holding 416 / 581 / 79 merged PRs, 2026-09-11:
///
/// | Document            | Wall clock         | Result      |
/// |---------------------|--------------------|-------------|
/// | 3 aliases x 100     | 11.0s, then 8.1s   | **502**, ok |
/// | 3 aliases x 50      | 3.42-3.68s         | ok 2/2      |
/// | 1 alias x 100       | 0.52s              | ok 2/2      |
///
/// THREE node-heavy aliases at `first: 100` straddle the ~11s deadline on
/// real data -- intermittently failing where 36 aliases over SPARSE ranges
/// succeeded in 9.22-10.84s. The difference is the number of nodes
/// actually materialised, not the number of aliases: the sparse ranges
/// returned a handful of PRs each and these return the full page.
///
/// Halving the page took 11.0s to 3.5s, a 3x improvement, which makes it
/// by far the most effective knob. So 50 is the DEFAULT rather than the
/// first rung of the ladder. Going out at 100 would mean a load that
/// intermittently 502s on its first attempt and recovers only via the
/// ladder -- paying a wasted request and a ~11s stall for it, every time,
/// on exactly the dense scopes this feature is for.
///
/// The cost of a 50-node page is more slices, not less data: the slicer
/// already subdivides until each slice is under 800, and the nodes are
/// paged per slice either way.
pub const SLICE_PAGE_FULL: u32 = 50;
pub const SLICE_PAGE_REDUCED: u32 = 25;

/// One rung down the ladder: a smaller page, then a smaller chunk.
///
/// # Page before aliases, which is the opposite of my first guess
///
/// I initially shed aliases first, reasoning that the 502 is a ~11s
/// deadline and alias count drives elapsed time toward it. The sparse
/// measurements supported that -- 48 node-heavy aliases failed where 80
/// count-only ones succeeded. The dense measurements above overturned it:
/// at three aliases, halving the PAGE took the document from 11.0s to
/// 3.5s, where halving the aliases could at best have taken it to ~7s.
///
/// The reason is that the work is per NODE, not per alias. An alias over
/// a sparse range is nearly free; an alias returning a full page of 100
/// PRs with per-PR diff statistics is not. Shedding aliases first would
/// have thrown away parallelism to fix a problem caused by page size, and
/// would have needed two or three rungs to achieve what one page halving
/// does.
///
/// Returns `None` at the bottom. A single alias at 25 nodes is the
/// smallest useful request; if the server gives up on that, asking for
/// less is not the problem and the error belongs to the caller.
pub fn degrade(chunk: usize, page: u32) -> Option<(usize, u32)> {
    if page > SLICE_PAGE_REDUCED {
        return Some((chunk, SLICE_PAGE_REDUCED));
    }
    if chunk > 1 {
        return Some(((chunk / 2).max(1), page));
    }
    None
}

/// What a scope load returned, and how complete it is.
///
/// #824 item 8 in the type system: there is no way to read a total out of
/// this without also being handed the facts about whether it is exact.
/// A caller that wants a bare number has to call [`Outcome::total`] and
/// can see [`Outcome::is_complete`] right beside it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// The exact count, summed from `issueCount` across every slice.
    /// Exact even when `retrievable` is false -- the 1,000 cap limits
    /// retrieval, not counting.
    pub total: u64,
    /// Whether every PR in the window could be RETRIEVED, not merely
    /// counted. False means per-PR detail is over a sample.
    pub retrievable: bool,
    /// How many PRs sit in slices whose nodes could not all be fetched.
    pub unretrievable: u64,
    /// How many slices the window was cut into. > 1 means the total is
    /// assembled, which #824 item 8 requires be visible.
    pub slices: usize,
    /// Probe rounds the plan took.
    pub rounds: u32,
    /// Whether the answer came from a connection (exact, uncapped) or
    /// from search (capped, sliced). Item 5's routing decision, reported
    /// so a reader can tell which guarantee they have.
    pub via_connection: bool,
    /// What it cost. See `budget.rs`.
    pub spend: super::budget::Spend,
    /// Fields GitHub refused on the responses behind this total, summed.
    /// Non-zero means some data is missing -- inherited from
    /// `client.rs:1094-1175`'s partial-success handling.
    pub refused_fields: usize,
}

impl Outcome {
    /// Whether this total can be presented as a plain number.
    ///
    /// False if anything is capped, refused, or unmetered. The UI in #826
    /// must branch on this rather than on a truthiness check of
    /// `unretrievable`, so that a new partiality channel added later
    /// cannot be forgotten at one call site.
    pub fn is_complete(&self) -> bool {
        self.retrievable && self.refused_fields == 0
    }

    /// Whether the count was assembled from more than one request.
    pub fn is_assembled(&self) -> bool {
        self.slices > 1
    }
}

/// Fetch a count for a window, completely.
///
/// The routing decision of #824 item 5 is the first thing here: a
/// single-repo scope goes to the connection, which has no 1,000-result
/// cap and needs no slicing at all. Everything else is sliced.
///
/// Bounded by [`LOAD_TIMEOUT`] around the WHOLE load, not per request --
/// see its docs for why a per-request bound is not enough.
pub async fn load_count(
    client: &GitHubClient,
    q: &StatsQuery,
    window: Slice,
    budget: &Budget,
) -> Result<Outcome, ClientError> {
    let started = std::time::Instant::now();
    // Captured before the move: `window` goes into the inner future, and
    // the log below runs after that future has been dropped.
    let (from, to) = (window.from.clone(), window.to.clone());
    match tokio::time::timeout(LOAD_TIMEOUT, load_count_inner(client, q, window, budget)).await {
        Ok(r) => r,
        Err(_) => {
            // What the timeout looked like from inside, because the error
            // alone cannot say (#853).
            //
            // `ClientError::Timeout` carries only the ceiling -- the same
            // "60" on every expiry -- so the log showed N
            // indistinguishable `graphql POST` lines and nothing tying
            // them to a round, a slice, or a window. With adaptive
            // slicing N is not even a fixed number, so "the stats view
            // hangs" arrived with no way to tell a slow network from a
            // scope too large to ever finish. That is the #790 situation
            // in new code, and the degradation ladder below is the
            // counter-example done right: it logs the chunk and page it
            // drops to.
            //
            // Read off the `Budget`, which is `Arc`-shared and therefore
            // still readable after the future is dropped -- so these are
            // measured counters rather than a guess at what got done.
            // `diag!` rather than `warn!`: a timeout is already reported
            // to the user through the error, and this is the detail only
            // someone diagnosing wants.
            crate::diag!(
                "[diag] stats load_count TIMEOUT after {:?} (ceiling {}s): \
                 window {}..{}, {} requests, {} points spent, {} unmetered",
                started.elapsed(),
                LOAD_TIMEOUT.as_secs(),
                from,
                to,
                budget.requests(),
                budget.spent(),
                budget.unmetered()
            );
            Err(ClientError::Timeout(LOAD_TIMEOUT.as_secs()))
        }
    }
}

async fn load_count_inner(
    client: &GitHubClient,
    q: &StatsQuery,
    window: Slice,
    budget: &Budget,
) -> Result<Outcome, ClientError> {
    // CONNECTION-FIRST. `repository.pullRequests` has no 1,000-result
    // cap (VERIFIED: pktstorm/headstate, 337 merged, cost 1), so a
    // single-repo scope is exact with no slicing and no probe rounds.
    if !q.scope.needs_search() {
        if let Some((owner, name)) = q.scope.owner_name() {
            return connection_count(client, owner, name, budget).await;
        }
        // A `Scope::Repo` whose value is not `owner/name` falls through
        // to search rather than failing: search still answers it
        // correctly, and a malformed scope is a caller bug that should
        // not take the feature down.
        log::warn!("single-repo scope is not owner/name; falling back to search");
    }

    // The COUNT path's threshold, which is the one `slice::SUBDIVIDE_AT`
    // exists for: just under the 1,000-result search cap, so the assembled
    // total is exact. A board needs a much smaller one -- see `plan_to`.
    let plan = plan(client, q, window, budget, slice::SUBDIVIDE_AT).await?;
    Ok(Outcome {
        total: plan.total(),
        retrievable: plan.is_retrievable(),
        unretrievable: plan.unretrievable(),
        slices: plan.slices.len(),
        rounds: plan.rounds,
        via_connection: false,
        spend: budget.snapshot(),
        refused_fields: 0,
    })
}

/// Build a complete plan for a window, running each probe round's chunks
/// concurrently at [`READ_CONCURRENCY`].
///
/// The ROUNDS are serial by construction -- each round's slices are
/// chosen from the previous round's counts, which is what "probe-driven"
/// means -- and the chunks WITHIN a round are parallel. That is the only
/// parallelism available here, and it is the one that matters: a round is
/// one request per `ALIAS_CHUNK` slices.
///
/// # Why this is public as of #826
///
/// `load_count` discards the plan after summing it, which is all a count
/// needs. A leaderboard needs the SLICES: it has to fetch nodes for each
/// one and then map them to people, so it needs the same tiling the
/// planner produced rather than a second division of the window.
///
/// Exposing the planner is the alternative to `board.rs` writing its own,
/// and #826 is explicit that it must not ("USE that layer; do not write a
/// second pagination path"). The reason is not reuse for its own sake: the
/// planner's tiling property -- contiguous, no gap, no overlap -- is what
/// makes a board complete, and `slice.rs`'s `ranges_tile` is the assertion
/// that earns it. A second path would have to earn it again, and a gap in
/// it would lose PRs silently.
pub async fn plan_window(
    client: &GitHubClient,
    q: &StatsQuery,
    window: Slice,
    budget: &Budget,
) -> Result<Plan, ClientError> {
    plan_to(client, q, window, budget, slice::SUBDIVIDE_AT).await
}

/// [`plan_window`] with the caller's subdivision threshold.
///
/// # The threshold a COUNT needs is not the threshold a BOARD needs
///
/// `slice::SUBDIVIDE_AT` is 800, sized just under the 1,000-result search cap
/// so that a count is exact. That is the right threshold for a count, and it
/// is the WRONG one for anything that reads nodes, because the two are
/// limited by different numbers:
///
/// - a count reads `issueCount`, which is exact at any size;
/// - a board reads `nodes`, and gets at most [`SLICE_PAGE_FULL`] of them.
///
/// MEASURED 2026-09-11, and this is a defect I found in my own first
/// implementation rather than a hypothetical: a 30-day `org:FNX-Labs` window
/// holds **569** merged pull requests. At 800 the planner leaves it as ONE
/// slice -- correctly, for a count -- and the detail fetch then retrieves
/// **50 of 569**, a 9% sample. The board reported `complete: false` and named
/// the short slice, so it was honest; but a top-five over 9% of the data is
/// not a ranking, and #826's rule is "never a confident top-five over a
/// sample". Honest and useless is still useless.
///
/// Passing `SLICE_PAGE_FULL` instead subdivides until every slice fits in one
/// page. On that same window it plans 30 day-slices holding 1-47 each
/// (measured: the per-day counts are 16, 18, 20, 21, 13, 23, 39, 47, 14, 34,
/// 15, 29, 31, 39, 22, 46, 26, 12, 7, 11, 18, 23, 6, 8, 1, 7, 4, 6, 5, 8 --
/// none over a 50-node page), and the whole 30-alias probe document costs
/// **1 point in 3.5s**.
///
/// # Why not cursor-page each slice instead
///
/// I measured that too, and it works: `search(..., after: <cursor>)` paged a
/// 569-PR slice at 1 point and 1.8-3.0s per page. It is the wrong mechanism
/// here because each page needs the PREVIOUS page's cursor, so 569 PRs is 12
/// SERIAL round-trips -- roughly 25s of wall clock against the planner's
/// parallel waves, and inside a 60-second `LOAD_TIMEOUT` that a busier scope
/// would exceed. Subdividing keeps the fan-out parallel at
/// [`READ_CONCURRENCY`], and it reuses the tiling property `slice.rs`'s
/// `ranges_tile` already proves rather than adding the second pagination path
/// #826 explicitly rules out.
pub async fn plan_to(
    client: &GitHubClient,
    q: &StatsQuery,
    window: Slice,
    budget: &Budget,
    subdivide_at: u64,
) -> Result<Plan, ClientError> {
    match tokio::time::timeout(LOAD_TIMEOUT, plan(client, q, window, budget, subdivide_at)).await {
        Ok(r) => r,
        Err(_) => Err(ClientError::Timeout(LOAD_TIMEOUT.as_secs())),
    }
}

async fn plan(
    client: &GitHubClient,
    q: &StatsQuery,
    window: Slice,
    budget: &Budget,
    subdivide_at: u64,
) -> Result<Plan, ClientError> {
    // `plan_with` is synchronous and takes a closure, because that is
    // what makes the completeness property testable without a network
    // (see `slice.rs`). Bridging to async here rather than making the
    // planner async keeps the test-only path honest: the code under test
    // is the code that ships.
    let mut pending = vec![window];
    let mut done = Vec::new();
    let mut irreducible = Vec::new();
    let mut rounds = 0;

    while !pending.is_empty() && rounds < slice::MAX_DEPTH {
        rounds += 1;
        let counts = probe_round(client, q, &pending, budget).await?;
        let mut next = Vec::new();
        for (s, count) in pending.into_iter().zip(counts) {
            let probed = slice::ProbedSlice { slice: s, count };
            // The threshold comes from the CALLER, because a count and a
            // board are limited by different numbers -- see `plan_to`.
            // `ProbedSlice::too_big` still pins the count path's 800 and is
            // what `slice.rs`'s completeness tests assert against.
            if probed.count < subdivide_at {
                done.push(probed);
                continue;
            }
            if slice::is_one_day(&probed.slice) {
                irreducible.push(probed.clone());
                done.push(probed);
                continue;
            }
            // Split proportionally to how far over the threshold the slice
            // is, not by halving: each round is a request, and halving a
            // 569-PR slice against a 50-node page would need four rounds
            // where one proportional split needs one.
            let pieces = slice::subdivide(
                &probed.slice,
                slice::split_factor_for(probed.count, subdivide_at),
            );
            if pieces.len() < 2 {
                irreducible.push(probed.clone());
                done.push(probed);
                continue;
            }
            next.extend(pieces);
        }
        pending = next;
    }
    for s in pending {
        let probed = slice::ProbedSlice { slice: s, count: 0 };
        irreducible.push(probed.clone());
        done.push(probed);
    }
    done.sort_by(|a, b| a.slice.from.cmp(&b.slice.from));
    Ok(Plan {
        slices: done,
        irreducible,
        rounds,
    })
}

/// Whether one more wave may be issued.
///
/// # The second half of #843
///
/// The in-advance gate in `commands.rs` is consulted ONCE, before request
/// one. All four wave loops below then iterate on input length alone, with
/// `budget` threaded in only so each chunk can `record()` -- so a load
/// committed to its projection and could not stop. `budget.rs:15-18`
/// diagnoses exactly this ("the probe-driven slicer cannot be costed in
/// advance by construction: how many slices it takes IS the thing it
/// discovers") and then implemented only the in-advance check. The real
/// bounds on spend were `MAX_DEPTH = 24` and `LOAD_TIMEOUT = 60s`; the
/// budget contributed nothing once a load had started.
///
/// The failure it allows: a user opens a 90-day org scope while the poll
/// loop has already spent most of the hour. The gate passes, the load runs
/// every wave it discovers, and the poll loop -- the thing the gate exists
/// to protect -- starves.
///
/// # Why PER WAVE and not per request
///
/// A wave is the unit that is actually decided: requests within one are
/// spawned together into a `JoinSet` and run concurrently, so there is no
/// point between them at which anything could be refused. Checking per wave
/// also bounds the overshoot to one wave's worth of spend, which is at most
/// `READ_CONCURRENCY` requests at the measured 1 point each.
///
/// # Why PROJECTED is one wave, not the rest of the load
///
/// What is left to do is unknowable here for the same reason the planner is
/// probe-driven. Projecting the remaining load would need a number nobody
/// has; projecting the NEXT wave needs only the wave's own size, and
/// refusing a wave stops the load just as effectively one wave later. The
/// projection is deliberately the small honest number rather than the large
/// invented one.
fn wave_permitted(budget: &Budget, requests_in_wave: usize) -> bool {
    // One point per request, which is the measured figure for every document
    // this module issues: `MEASURED_PROBE_COST` and
    // `MEASURED_DETAIL_CHUNK_COST` are both 1, each confirmed live at 36, 60
    // and 80 aliases and at 12, 24 and 36 aliases respectively
    // (`budget.rs:91-140`). A projection is allowed to be a round number; it
    // is not allowed to be an invented one.
    budget.permits(requests_in_wave as u64)
}

/// One probe round: every slice's `issueCount`, in chunks of
/// [`ALIAS_CHUNK`] run at [`READ_CONCURRENCY`].
///
/// Counts come back in the SAME ORDER as the slices went in, which the
/// caller relies on to pair them. Order is preserved by indexing into a
/// pre-sized vector rather than by collection order, because the chunks
/// complete out of order -- the same hazard `query.rs:667-678` records
/// for the history series, solved the same way: absolute indices.
///
/// # A refused wave is an ERROR here, not a partial
///
/// This is the one path where a short answer is not honest-partial. A probe
/// round feeds the PLANNER: a missing count reads as "no activity in that
/// range", which silently shrinks the total and tiles the window wrongly for
/// every subsequent round. The same reasoning the per-alias check below
/// gives -- "a missing alias is NOT zero" -- and the same reasoning
/// `load_series`'s doc gives for why a count differs from a chart.
///
/// So a budget refusal mid-plan fails the load with a message naming the
/// budget, rather than returning a plan over part of the window. The
/// honest-partial channels exist for the paths that read NODES
/// (`board.rs:566-573`), and they are where the other three loops return to.
async fn probe_round(
    client: &GitHubClient,
    q: &StatsQuery,
    slices: &[Slice],
    budget: &Budget,
) -> Result<Vec<u64>, ClientError> {
    let mut counts = vec![0u64; slices.len()];
    let per_wave = ALIAS_CHUNK * READ_CONCURRENCY;
    for (w, wave) in slices.chunks(per_wave).enumerate() {
        let base = w * per_wave;
        // MID-LOAD budget re-check (#843). Errors rather than truncating,
        // for the reason this function's doc gives: a short probe round
        // mis-tiles the window for every round after it.
        if !wave_permitted(budget, wave.len().div_ceil(ALIAS_CHUNK)) {
            return Err(ClientError::Graphql(format!(
                "GitHub budget fell below the {}-point reserve while planning \
                 this scope; stopping so the background refresh keeps working",
                super::budget::RESERVE
            )));
        }
        let mut set = tokio::task::JoinSet::new();
        for (n, chunk) in wave.chunks(ALIAS_CHUNK).enumerate() {
            let first_index = base + n * ALIAS_CHUNK;
            let doc = probe_query(q, chunk, first_index);
            let client = client.clone();
            let budget = budget.clone();
            let len = chunk.len();
            set.spawn(async move {
                let v = metered_read(&client, &budget, json!({ "query": doc })).await?;
                let mut out = Vec::with_capacity(len);
                for i in 0..len {
                    let alias = super::query::slice_alias(first_index + i);
                    // A missing alias is NOT zero. GitHub returning
                    // partial data (`client.rs:1094-1175`) would leave
                    // the alias absent, and defaulting it to 0 would
                    // make a slice that failed look empty -- which is
                    // the exact shape that turns a truncation into a
                    // confident wrong number. Reported as an error so
                    // the load fails rather than under-counting.
                    let c = v[&alias]["issueCount"].as_u64().ok_or_else(|| {
                        ClientError::Graphql(format!(
                            "probe response missing issueCount for slice {i} of {len}"
                        ))
                    })?;
                    out.push(c);
                }
                Ok::<(usize, Vec<u64>), ClientError>((first_index, out))
            });
        }
        while let Some(joined) = set.join_next().await {
            // A panicked task surfaced rather than swallowed: a dropped
            // chunk would leave zeros in `counts`, which reads as "no
            // activity in that range" and silently shrinks the total.
            // Same reasoning as `client.rs:975-977`.
            let (first_index, out) = joined.map_err(|e| ClientError::Join(e.to_string()))??;
            for (i, c) in out.into_iter().enumerate() {
                counts[first_index + i] = c;
            }
        }
    }
    Ok(counts)
}

/// An exact count for ONE repository, through the uncapped connection.
///
/// `totalCount` on `repository.pullRequests` is the repository's real
/// total with no 1,000 ceiling -- VERIFIED live at 337 for
/// pktstorm/headstate, cost 1. One request, no probe rounds, no slices.
async fn connection_count(
    client: &GitHubClient,
    owner: &str,
    name: &str,
    budget: &Budget,
) -> Result<Outcome, ClientError> {
    let v = metered_read(
        client,
        budget,
        json!({
            "query": REPO_CONNECTION_QUERY,
            // `first: 1` because only `totalCount` is wanted here. The
            // page is not the cost -- `query.rs:480-484` records that
            // GitHub charges the connection, not the page -- so this is
            // about bytes on the wire, not points.
            "variables": { "owner": owner, "name": name, "first": 1, "after": null },
        }),
    )
    .await?;
    let total = v["repository"]["pullRequests"]["totalCount"]
        .as_u64()
        .ok_or_else(|| {
            // Not defaulted to 0: a repository the token cannot see
            // returns a null `repository`, and reporting that as "no
            // pull requests" is the dishonest-empty-state failure #823
            // item 9 names.
            ClientError::Graphql(format!(
                "no pullRequests total for {owner}/{name} (private, renamed, or gone?)"
            ))
        })?;
    Ok(Outcome {
        total,
        // The connection has no cap, so a connection answer is
        // retrievable by construction. This is the whole reason item 5
        // prefers it.
        retrievable: true,
        unretrievable: 0,
        slices: 1,
        rounds: 0,
        via_connection: true,
        spend: budget.snapshot(),
        refused_fields: crate::github::client::refused_fields_of(&v),
    })
}

/// Fetch per-PR detail for a planned set of slices, degrading on 5xx.
///
/// The ladder is `client.rs:1024-1050`'s, with the extra rung my
/// measurements call for -- see [`degrade`]. Returns the raw alias map so
/// #826 can map it into whatever a leaderboard needs; mapping belongs
/// with the view that defines the shape, not here.
pub async fn load_detail(
    client: &GitHubClient,
    q: &StatsQuery,
    slices: &[Slice],
    budget: &Budget,
) -> Result<serde_json::Value, ClientError> {
    load_detail_chunked(client, q, slices, budget, ALIAS_CHUNK).await
}

/// Where a detail fetch writes each wave down AS IT COMPLETES, so a
/// dropped future does not take the data with it (#1044).
///
/// # Why this exists rather than a plain return value
///
/// `tokio::time::timeout` DROPS the future it is racing. Everything that
/// future owns -- including every alias a completed wave had already
/// merged -- is destroyed at the moment the ceiling expires. Those waves
/// were real POSTs, paid for against the rate limit, and #1004 built the
/// accumulation table precisely so they would survive; the timeout branch
/// was the one path that threw them away, which is why the scopes that
/// need eventual consistency most were the exact scopes it never reached.
///
/// So the accumulator is owned by the CALLER, OUTSIDE the future, and the
/// future only appends to it. A timeout can then read what was retrieved
/// out of a future that no longer exists.
///
/// # Why a mutex rather than a channel
///
/// The reader wants the WHOLE set at one instant (the moment the ceiling
/// expires), not a stream; a channel would make the reader drain an
/// unknown number of messages under the same deadline that just expired.
/// Contention is nil: one lock per completed wave, held for the length of
/// a map extend.
#[derive(Debug, Clone, Default)]
pub struct PartialDetail(std::sync::Arc<std::sync::Mutex<PartialDetailInner>>);

/// [`PartialDetail`]'s contents, behind its lock.
#[derive(Debug, Default)]
struct PartialDetailInner {
    /// Aliases merged so far, keyed exactly as the completed map keys
    /// them -- `query::slice_alias`'s ABSOLUTE index, so a partial map is
    /// read by the same `Board::from_alias_map` as a whole one.
    merged: serde_json::Map<String, serde_json::Value>,
    /// Refused fields summed across every response so far.
    refused: usize,
    /// Waves that finished before the ceiling expired.
    waves: usize,
    /// Slices whose aliases a completed wave covered.
    slices_done: usize,
}

impl PartialDetail {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one completed wave.
    ///
    /// A POISONED lock is recovered from rather than panicked on. A panic
    /// here would convert "a wave's data is questionable" into "the whole
    /// load dies", which is the opposite of this type's purpose; the
    /// inner map is a plain accumulator with no invariant a panic
    /// mid-insert could break.
    ///
    /// `pub(super)` rather than private: `board.rs` seeds a sink in its
    /// own tests to exercise the timeout branch's mapping directly, and
    /// those two modules are the only pair that ever touch one.
    pub(super) fn record(
        &self,
        aliases: &serde_json::Map<String, serde_json::Value>,
        refused: usize,
        slices_done: usize,
    ) {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        for (k, v) in aliases {
            g.merged.insert(k.clone(), v.clone());
        }
        g.refused += refused;
        g.waves += 1;
        g.slices_done = g.slices_done.max(slices_done);
    }

    /// What has been retrieved so far, in the shape `load_detail_chunked`
    /// returns on success.
    ///
    /// Identical shape on purpose: the partial path must not need a
    /// second mapper, or the two would drift and the partial one would be
    /// the untested of the pair.
    pub fn snapshot(&self) -> serde_json::Value {
        let g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut map = g.merged.clone();
        // Non-zero only, matching `graphql_partial_ok`'s rule that the
        // key's ABSENCE means no refusal and a written 0 would be a
        // second way of saying that.
        if g.refused > 0 {
            map.insert("__refused".into(), g.refused.into());
        }
        serde_json::Value::Object(map)
    }

    /// How many waves completed, and how many slices they covered.
    pub fn progress(&self) -> (usize, usize) {
        let g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        (g.waves, g.slices_done)
    }
}

/// [`load_detail`] with the caller's chunk size rather than
/// [`ALIAS_CHUNK`].
///
/// # Why a caller gets to choose, as of #826
///
/// `ALIAS_CHUNK` is 10 and sizes BOTH documents this layer issues, which
/// is one number doing two jobs that measure seven times apart. #826
/// measured the detail document at 10 aliases x `SLICE_PAGE_FULL` against
/// real dense day-slices (`org:FNX-Labs`, 2026-08, 245 merged PRs over 10
/// days) and it failed **0 of 3**, every failure at 10.6s against the ~11s
/// deadline this module's docs establish. The probe document at the same 10
/// aliases answered in 1.4-1.5s, because it materialises no nodes at all.
///
/// #827's own figure for this document was 3 aliases x 50 at 3.42-3.68s,
/// which is correct and was measured at a third of the chunk it then
/// shipped. Nothing between 3 and 10 was measured, and 10 is where it
/// breaks -- so this is a gap in the measurement rather than an error in
/// the reasoning, and the reasoning already says pages matter more than
/// aliases.
///
/// The degradation ladder is unchanged and still sheds pages before
/// aliases: a smaller starting chunk reduces how often the ladder is
/// needed, it does not replace it. `board::BOARD_ALIAS_CHUNK` is the
/// measured value for the board's document.
pub async fn load_detail_chunked(
    client: &GitHubClient,
    q: &StatsQuery,
    slices: &[Slice],
    budget: &Budget,
    chunk: usize,
) -> Result<serde_json::Value, ClientError> {
    load_detail_into(client, q, slices, budget, chunk, &PartialDetail::new()).await
}

/// [`load_detail_chunked`], writing each completed wave into the caller's
/// [`PartialDetail`] as it goes (#1044).
///
/// The caller owns the sink, so a ceiling that drops THIS future still
/// leaves the caller holding every wave that finished before it expired.
/// `load_detail_chunked` is this function with a sink nobody reads, which
/// is the honest way to express "this caller does not want the partial"
/// rather than duplicating the body.
pub async fn load_detail_into(
    client: &GitHubClient,
    q: &StatsQuery,
    slices: &[Slice],
    budget: &Budget,
    chunk: usize,
    sink: &PartialDetail,
) -> Result<serde_json::Value, ClientError> {
    // Clamped rather than trusted. The chunk reaches here from a caller's
    // constant today, but a 0 would make `slices.chunks(0)` panic and a
    // value over the ceiling would build a document the deadline refuses
    // -- and the ceiling exists precisely because a reviewer cannot see
    // either failure in a diff.
    let chunk = chunk.clamp(1, ALIAS_CEILING);
    let started = std::time::Instant::now();
    match tokio::time::timeout(
        LOAD_TIMEOUT,
        detail_with_ladder(client, q, slices, budget, chunk, SLICE_PAGE_FULL, sink),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => {
            // The detail half of the same gap (#853), and the one that
            // needs it more: this path is ADAPTIVE. The ladder may have
            // degraded the document several times before the deadline
            // expired, so "how far did it get" cannot be inferred from
            // the call site -- the starting chunk is not the chunk it
            // died on.
            //
            // How many SLICES were planned is the number that was missing
            // most: it is what distinguishes a slow network from a scope
            // whose window was cut into more pieces than 60s can ever
            // cover, and the two want opposite responses from the user.
            // `requests` against `slices.len()` gives that directly.
            //
            // The chunk logged is the one this call STARTED at;
            // `detail_with_ladder` already logs each degradation step at
            // `warn!`, so the pair reconstructs the descent.
            // #1044 added the two figures that say whether the ceiling
            // expired on a load that had done nothing or on one that was
            // nearly finished: waves completed, and slices they covered.
            // Without them a 59-seconds-of-useful-work failure reads
            // identically to an instant one.
            let (waves, slices_done) = sink.progress();
            crate::diag!(
                "[diag] stats load_detail_chunked TIMEOUT after {:?} (ceiling {}s): \
                 {} slices planned, {} covered by {} completed waves, \
                 {} outstanding, start chunk {}, {} requests completed, \
                 {} points spent, {} unmetered",
                started.elapsed(),
                LOAD_TIMEOUT.as_secs(),
                slices.len(),
                slices_done,
                waves,
                slices.len().saturating_sub(slices_done),
                chunk,
                budget.requests(),
                budget.spent(),
                budget.unmetered()
            );
            Err(ClientError::Timeout(LOAD_TIMEOUT.as_secs()))
        }
    }
}

async fn detail_with_ladder(
    client: &GitHubClient,
    q: &StatsQuery,
    slices: &[Slice],
    budget: &Budget,
    chunk: usize,
    page: u32,
    sink: &PartialDetail,
) -> Result<serde_json::Value, ClientError> {
    match detail_round(client, q, slices, budget, chunk, page, sink).await {
        Ok(v) => Ok(v),
        Err(e) if crate::github::client::server_gave_up_on(&e) => {
            let Some((next_chunk, next_page)) = degrade(chunk, page) else {
                // The bottom of the ladder. One alias, 50 nodes, and
                // GitHub still gave up: asking for less is not the
                // problem, so the error is the answer.
                return Err(e);
            };
            log::warn!(
                "GitHub could not answer a {chunk}-alias/{page}-node stats document ({e}); \
                 retrying at {next_chunk}/{next_page} -- the load will take longer"
            );
            Box::pin(detail_with_ladder(
                client, q, slices, budget, next_chunk, next_page, sink,
            ))
            .await
        }
        Err(e) => Err(e),
    }
}

/// One pass over every slice's nodes, wave by wave.
///
/// # A refused wave returns a PARTIAL, not an error (#843)
///
/// This is the path the honest-partial channels were built for, and they need
/// nothing new: `Board::from_alias_map` (`board.rs:480-493`) already treats an
/// alias whose `issueCount` is absent as a `ShortSlice` with `retrieved: 0`
/// and an UNKNOWN true size -- "the honest shape, since the only figure that
/// could have said how big it was is the one that is missing" -- and any
/// non-empty `truncated_slices` clears `Board::complete`.
///
/// So stopping at a wave boundary produces a board that names every range it
/// did not read and reports itself incomplete, which is what #826's rule
/// asks for: never a confident top-five over a sample. Returning an error
/// instead would discard waves already paid for and show the user nothing
/// for spend they have already made.
async fn detail_round(
    client: &GitHubClient,
    q: &StatsQuery,
    slices: &[Slice],
    budget: &Budget,
    chunk: usize,
    page: u32,
    sink: &PartialDetail,
) -> Result<serde_json::Value, ClientError> {
    let mut merged = serde_json::Map::new();
    let mut refused = 0usize;
    let per_wave = chunk * READ_CONCURRENCY;
    for (w, wave) in slices.chunks(per_wave).enumerate() {
        let base = w * per_wave;
        // MID-LOAD budget re-check (#843). The aliases for the slices this
        // skips are simply absent from the merged map, which `from_alias_map`
        // already reads as a named short slice -- so the board comes back
        // partial and says which ranges it is missing, rather than running to
        // completion and starving the poll loop.
        if !wave_permitted(budget, wave.len().div_ceil(chunk)) {
            log::warn!(
                "GitHub budget fell below the {}-point reserve mid-load; \
                 returning a partial board over {} of {} slices",
                super::budget::RESERVE,
                base,
                slices.len()
            );
            break;
        }
        let mut set = tokio::task::JoinSet::new();
        for (n, part) in wave.chunks(chunk).enumerate() {
            let first_index = base + n * chunk;
            let doc = slice_detail_query(q, part, first_index, page);
            let client = client.clone();
            let budget = budget.clone();
            set.spawn(async move { metered_read(&client, &budget, json!({ "query": doc })).await });
        }
        // This wave's own aliases, kept separately from `merged` so the
        // sink is handed exactly what THIS wave added (#1044). Merging
        // into `merged` and re-sending the whole map each time would be
        // quadratic in waves for no gain.
        let mut wave_aliases = serde_json::Map::new();
        let mut wave_refused = 0usize;
        while let Some(joined) = set.join_next().await {
            let v = joined.map_err(|e| ClientError::Join(e.to_string()))??;
            if let Some(obj) = v.as_object() {
                // Absolute alias indices, so chunks merge in any
                // completion order without clobbering each other --
                // `query.rs:667-678`'s rule.
                for (k, val) in obj {
                    merged.insert(k.clone(), val.clone());
                    wave_aliases.insert(k.clone(), val.clone());
                }
            }
            // `__refused` is a TOP-LEVEL key on each response, not an alias,
            // so the blanket insert above makes the last refusing chunk's
            // count win instead of accumulating. Summed explicitly.
            //
            // Found in review, and it understated rather than hid: any
            // non-zero count still trips `complete`, and `graphql_partial_ok`
            // inserts the key only when there ARE refusals
            // (`client.rs:1204-1213`), so a clean chunk could not zero a
            // dirty one. But the FIGURE drives the SAML remediation message,
            // and "GitHub refused 4 fields" when it refused 7 is the kind of
            // wrong number that makes a reader distrust the advice attached
            // to it.
            //
            // `series_inner` below already accumulates per chunk, which is
            // what the two paths now have in common.
            let r = crate::github::client::refused_fields_of(&v);
            refused += r;
            wave_refused += r;
        }
        // Written down BEFORE the next wave starts, which is the whole of
        // #1044: the ceiling can expire at any await point after this, and
        // whatever is in the sink at that instant survives the drop of
        // this future.
        //
        // `__refused` is a top-level key rather than an alias and rides
        // along in `wave_aliases`; `PartialDetail::snapshot` rewrites it
        // from its own running sum, so the sink's figure is authoritative
        // and the stray copy is overwritten rather than double-counted.
        sink.record(&wave_aliases, wave_refused, base + wave.len());
    }
    // Written back as the merged map's own key, so `Board::from_alias_map`
    // keeps reading the count through `client::refused_fields_of` -- one
    // reader for this value rather than a second convention for the merged
    // shape. Inserted only when non-zero, matching `graphql_partial_ok`'s own
    // rule: the key's ABSENCE means no refusal, and a written 0 would be a
    // second way of saying that.
    if refused > 0 {
        merged.insert("__refused".into(), refused.into());
    }
    Ok(serde_json::Value::Object(merged))
}

/// One day of scoped activity.
///
/// Fields named to match the existing `HistoryPoint` the chart component
/// already consumes, so the scoped series renders through the SAME chart
/// rather than a second one -- #826 is explicit that the existing chart
/// components are to be reused rather than a second charting idiom
/// introduced.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScopedPoint {
    /// `YYYY-MM-DD` in UTC, which is the bucket GitHub's bare date
    /// qualifiers actually use. `ActivityChart` already discloses that
    /// boundary ("Opened and merged per day (UTC)") and the scoped series
    /// inherits the same distortion for the same reason, so the disclosure
    /// covers it rather than needing a second one.
    pub date: String,
    pub opened: u64,
    pub merged: u64,
}

/// The scoped series, with the honesty fields a partial one needs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    /// One point per day, oldest first.
    pub points: Vec<ScopedPoint>,
    /// Days whose counts did not come back.
    ///
    /// Named rather than counted, and NOT defaulted to zero. A day missing
    /// from the response that rendered as `0` would draw a trough in the
    /// chart that looks like a quiet Tuesday -- the most legible possible
    /// lie, because a chart invites the eye to read shape. `probe_round`
    /// records the same rule for the count path and the reasoning is
    /// stronger here.
    pub failed_days: Vec<String>,
    /// Fields GitHub refused across the series responses.
    pub refused_fields: usize,
    pub spend: super::budget::Spend,
    /// Why days are missing, when the reason is not GitHub's (#1050).
    ///
    /// `None` means the days that failed did so at GitHub: a document went
    /// unanswered, or a field was refused. `Some` means this process
    /// declined to issue the request at all, which is a DIFFERENT fact and
    /// the one the UI previously could not tell.
    ///
    /// Without this field `Series` carried no way to say "we never asked".
    /// `StatsPage` had two explanations available -- a SAML refusal when
    /// `refused_fields > 0`, and "GitHub did not answer ... usually clears
    /// on its own" otherwise -- and a budget-exhausted load matched the
    /// second while being wrong in both halves: GitHub was never asked, and
    /// it clears when the hourly window resets rather than on its own.
    ///
    /// Deliberately NOT folded into `refused_fields`. That field means
    /// GitHub refused a field and is load-bearing for the SAML message;
    /// overloading it would trade one confidently wrong sentence for
    /// another.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unmeasured: Option<Unmeasured>,
}

/// Why a load stopped short for a reason that is not GitHub's (#1050).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Unmeasured {
    /// The rate-limit budget was under [`super::budget::RESERVE`], so the
    /// request was never issued.
    ///
    /// Carries the figures the message needs, rather than leaving the UI to
    /// restate a constant it would then have to keep in step: `remaining`
    /// and `reset_at` are read off the budget at the moment of the refusal.
    BudgetExhausted {
        remaining: Option<u64>,
        reserve: u64,
        reset_at: Option<chrono::DateTime<chrono::Utc>>,
    },
}

impl Series {
    /// Whether every day in the window was measured.
    pub fn is_complete(&self) -> bool {
        self.failed_days.is_empty() && self.refused_fields == 0
    }
}

/// Fetch the scoped daily series, chunked and bounded.
///
/// Count-only, so this is the cheap half of a scope load: MEASURED
/// 2026-09-11, 10 count-only aliases over dense day-slices answered in
/// 1.4-1.5s. It is issued as its OWN request rather than folded into the
/// board's, because `StatsPage.tsx:12-22` records that three independent
/// queries rendering as each lands beat one combined gate -- and #826 notes
/// an org "Others" view has more parts and more variance, so a single gate
/// would be worse here than there.
///
/// A failed chunk does not fail the series: its days are reported in
/// `failed_days` and the rest of the chart draws. That is the opposite of
/// the count path, which errors on a missing alias -- and the asymmetry is
/// deliberate. A count is ONE number and a short one is simply wrong, while
/// a chart of 30 days missing 2 is still the most informative thing
/// available, provided it says which 2.
pub async fn load_series(
    client: &GitHubClient,
    q: &StatsQuery,
    days: &[String],
    budget: &Budget,
) -> Result<Series, ClientError> {
    match tokio::time::timeout(LOAD_TIMEOUT, series_inner(client, q, days, budget)).await {
        Ok(r) => r,
        Err(_) => Err(ClientError::Timeout(LOAD_TIMEOUT.as_secs())),
    }
}

async fn series_inner(
    client: &GitHubClient,
    q: &StatsQuery,
    days: &[String],
    budget: &Budget,
) -> Result<Series, ClientError> {
    // Indexed rather than pushed, so chunks completing out of order cannot
    // reorder the chart -- `query.rs:667-678`'s absolute-index rule.
    let mut merged = vec![None::<u64>; days.len()];
    let mut opened = vec![None::<u64>; days.len()];
    let mut refused = 0usize;
    let mut unmeasured: Option<Unmeasured> = None;
    let per_wave = ALIAS_CHUNK * READ_CONCURRENCY;

    for (w, wave) in days.chunks(per_wave).enumerate() {
        let base = w * per_wave;
        // MID-LOAD budget re-check (#843). Stopping leaves these days as
        // `None`, which the tail of this function turns into named
        // `failed_days` rather than into a trough at zero -- "defaulting here
        // would draw a trough that reads as a quiet day". A chart of 30 days
        // missing 8 is still the most informative thing available, provided
        // it says which 8, which is `load_series`'s own stated rule.
        if !wave_permitted(budget, wave.len().div_ceil(ALIAS_CHUNK)) {
            log::warn!(
                "GitHub budget fell below the {}-point reserve mid-load; \
                 the remaining {} days are reported as unmeasured rather than \
                 as zero",
                super::budget::RESERVE,
                days.len().saturating_sub(base)
            );
            // Recorded, not merely logged (#1050). A log line cannot reach
            // the UI, and this is the difference between "GitHub did not
            // answer" and "we did not ask" -- which on wave 0 is the whole
            // of what the user sees.
            let snap = budget.snapshot();
            unmeasured = Some(Unmeasured::BudgetExhausted {
                remaining: snap.remaining,
                reserve: super::budget::RESERVE,
                reset_at: snap.reset_at,
            });
            break;
        }
        let mut set = tokio::task::JoinSet::new();
        for (n, chunk) in wave.chunks(ALIAS_CHUNK).enumerate() {
            let first_index = base + n * ALIAS_CHUNK;
            let doc = super::query::series_query(q, chunk, first_index);
            let client = client.clone();
            let budget = budget.clone();
            let len = chunk.len();
            set.spawn(async move {
                let v = metered_read(&client, &budget, json!({ "query": doc })).await?;
                let refused = crate::github::client::refused_fields_of(&v);
                let mut out = Vec::with_capacity(len);
                for i in 0..len {
                    let (m, o) = super::query::day_aliases(first_index + i);
                    // `None` rather than 0 for a missing alias. The caller
                    // turns it into a named failed day; defaulting here
                    // would draw a trough that reads as a quiet day.
                    out.push((v[&m]["issueCount"].as_u64(), v[&o]["issueCount"].as_u64()));
                }
                Ok::<(usize, Vec<(Option<u64>, Option<u64>)>, usize), ClientError>((
                    first_index,
                    out,
                    refused,
                ))
            });
        }
        while let Some(joined) = set.join_next().await {
            // A chunk that FAILED outright leaves its days as `None`, and
            // they become named failed days. Not propagated as an error:
            // see `load_series`' doc for why a chart differs from a count
            // here. A panic IS propagated, because a dropped task is a bug
            // rather than a server refusal.
            match joined.map_err(|e| ClientError::Join(e.to_string()))? {
                Ok((first_index, out, r)) => {
                    refused += r;
                    for (i, (m, o)) in out.into_iter().enumerate() {
                        merged[first_index + i] = m;
                        opened[first_index + i] = o;
                    }
                }
                Err(e) => log::warn!(
                    "a scoped series chunk failed ({e}); its days are reported as \
                     unmeasured rather than as zero"
                ),
            }
        }
    }

    let mut points = Vec::with_capacity(days.len());
    let mut failed_days = Vec::new();
    for (i, d) in days.iter().enumerate() {
        match (merged[i], opened[i]) {
            (Some(m), Some(o)) => points.push(ScopedPoint {
                date: d.clone(),
                merged: m,
                opened: o,
            }),
            // EITHER half missing makes the day unmeasured. A point with a
            // merged count and no opened count would render as "opened
            // nothing", which is a claim rather than a gap.
            _ => failed_days.push(d.clone()),
        }
    }

    Ok(Series {
        points,
        failed_days,
        refused_fields: refused,
        spend: budget.snapshot(),
        unmeasured,
    })
}

/// One person's reviews GIVEN in a window.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewerRow {
    pub login: String,
    /// Pull requests in scope, merged in the window, that this person
    /// reviewed. A real `0` -- never a stand-in for an unmeasured count,
    /// which is [`Reviewers::unmeasured`]'s job.
    pub reviews: u64,
}

/// The reviews-given board, with the honesty fields a partial one needs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reviewers {
    /// One row per login that was successfully counted, ranked highest
    /// first with ties broken on login.
    ///
    /// Includes rows whose count is `0`. A zero here is a MEASURED zero --
    /// "this person reviewed nothing in this window" -- and the UI is what
    /// decides not to rank it (`Leaderboard.tsx`'s "a zero has no rank"
    /// rule). Dropping them in Rust instead would make "everybody reviewed
    /// nothing" indistinguishable from "nobody could be measured", which is
    /// precisely the confusion the next field exists to prevent.
    pub rows: Vec<ReviewerRow>,
    /// Logins whose count did NOT come back, named rather than counted.
    ///
    /// The #802/#790 rule applied to a leaderboard: a failed alias rendered
    /// as `0` would place a colleague at the BOTTOM of a ranking on the
    /// strength of a query that never answered. Named, because a reader
    /// deciding whether to trust the order needs to know who is missing from
    /// it -- "2 people could not be measured" does not say whether the
    /// leader might be one of them.
    pub unmeasured: Vec<String>,
    /// Fields GitHub refused across the reviewer responses.
    ///
    /// Its own channel rather than folded into `unmeasured`, matching
    /// `Board`'s three-channel split: a refusal suggests a SAML
    /// authorization to fix, and a missing alias suggests a retry.
    pub refused_fields: usize,
    pub spend: super::budget::Spend,
}

impl Reviewers {
    /// Whether every login in scope was measured.
    ///
    /// The caller renders a ranking as a ranking only when this holds. A
    /// top-five over a roster with one unmeasured member can have the wrong
    /// person in first place, which is `board.rs`'s reason for making
    /// completeness a required prop rather than an optional note.
    pub fn is_complete(&self) -> bool {
        self.unmeasured.is_empty() && self.refused_fields == 0
    }
}

/// Count reviews GIVEN by each of `logins`, over one window (#826).
///
/// # Why there is no slicing here
///
/// This document RETRIEVES nothing -- it reads `issueCount` and no `nodes`
/// -- and `issueCount` is exact at any size. The 1,000-result cap that
/// forces `slice.rs` to subdivide limits what can be PAGED OUT of a search,
/// not what can be counted (`slice::SUBDIVIDE_AT`'s doc records exactly
/// that distinction, and #829 found the bug that comes from confusing the
/// two in the other direction). So one alias spans the whole window and
/// there is no plan, no probe round and no per-slice arithmetic to get
/// wrong.
///
/// # Chunked at `ALIAS_CHUNK`, not at `BOARD_ALIAS_CHUNK`
///
/// The measured asymmetry. `board::BOARD_ALIAS_CHUNK` is 5 because 10
/// node-bearing aliases at a 50-node page failed 0 of 3 against the ~11s
/// server deadline. This document carries no nodes at all: MEASURED live
/// 2026-09-11 on `org:FNX-Labs`, 36 reviewer aliases answered in 3.62-4.15s
/// at cost 1, and 10 in 1.26-1.50s. Nodes drive the deadline, which is why
/// `degrade` sheds PAGES first and `SLICE_PAGE_FULL` stays 50 -- a document
/// with no page is governed by alias latency alone, and `ALIAS_CHUNK`'s 10
/// is already sized against that.
///
/// # A failed chunk does not fail the board
///
/// Its logins are named in `unmeasured` and the rest still render, matching
/// `load_series` rather than `load_count`. The reasoning there applies with
/// more force: a count is one number and a short one is simply wrong, while
/// a ranking missing two named people is still useful to a reader who can
/// see WHICH two. What must never happen is the missing ones appearing as
/// zeroes, which would rank them last on the strength of nothing.
pub async fn load_reviewers(
    client: &GitHubClient,
    q: &StatsQuery,
    logins: &[String],
    window: &Slice,
    budget: &Budget,
) -> Result<Reviewers, ClientError> {
    match tokio::time::timeout(
        LOAD_TIMEOUT,
        reviewers_inner(client, q, logins, window, budget),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => Err(ClientError::Timeout(LOAD_TIMEOUT.as_secs())),
    }
}

async fn reviewers_inner(
    client: &GitHubClient,
    q: &StatsQuery,
    logins: &[String],
    window: &Slice,
    budget: &Budget,
) -> Result<Reviewers, ClientError> {
    // Indexed rather than pushed, so chunks completing out of order cannot
    // reassign a count to the wrong person -- `query.rs:667-678`'s
    // absolute-index rule, and the consequence of getting it wrong here is
    // worse than a reordered chart: it would attribute one colleague's
    // review count to another by name.
    let mut counts = vec![None::<u64>; logins.len()];
    let mut refused = 0usize;
    let per_wave = ALIAS_CHUNK * READ_CONCURRENCY;

    for (w, wave) in logins.chunks(per_wave).enumerate() {
        let base = w * per_wave;
        // MID-LOAD budget re-check (#843). Stopping leaves these logins as
        // `None`, which becomes `Reviewers::unmeasured` -- named people whose
        // count did not come back, kept distinct from a measured zero.
        // Defaulting them to 0 would rank colleagues last on a query that was
        // never issued, which this function's own doc forbids.
        if !wave_permitted(budget, wave.len().div_ceil(ALIAS_CHUNK)) {
            log::warn!(
                "GitHub budget fell below the {}-point reserve mid-load; \
                 {} reviewers are reported as unmeasured rather than as zero",
                super::budget::RESERVE,
                logins.len().saturating_sub(base)
            );
            break;
        }
        let mut set = tokio::task::JoinSet::new();
        for (n, chunk) in wave.chunks(ALIAS_CHUNK).enumerate() {
            let first_index = base + n * ALIAS_CHUNK;
            let doc = super::query::reviewer_query(q, chunk, window, first_index);
            let client = client.clone();
            let budget = budget.clone();
            let len = chunk.len();
            set.spawn(async move {
                let v = metered_read(&client, &budget, json!({ "query": doc })).await?;
                let refused = crate::github::client::refused_fields_of(&v);
                let mut out = Vec::with_capacity(len);
                for i in 0..len {
                    let alias = super::query::reviewer_alias(first_index + i);
                    // `None` rather than 0 for a missing alias. The caller
                    // turns it into a named unmeasured login; defaulting to
                    // zero would rank that person last on a query that
                    // never answered.
                    out.push(v[&alias]["issueCount"].as_u64());
                }
                Ok::<(usize, Vec<Option<u64>>, usize), ClientError>((first_index, out, refused))
            });
        }
        while let Some(joined) = set.join_next().await {
            // A chunk that failed outright leaves its logins as `None` and
            // they become named unmeasured rows. A panic IS propagated: a
            // dropped task is a bug rather than a server refusal, which is
            // `series_inner`'s distinction above.
            match joined.map_err(|e| ClientError::Join(e.to_string()))? {
                Ok((first_index, out, r)) => {
                    refused += r;
                    for (i, c) in out.into_iter().enumerate() {
                        counts[first_index + i] = c;
                    }
                }
                Err(e) => log::warn!(
                    "a reviewer-count chunk failed ({e}); its logins are reported as \
                     unmeasured rather than as zero"
                ),
            }
        }
    }

    let mut rows = Vec::new();
    let mut unmeasured = Vec::new();
    for (i, login) in logins.iter().enumerate() {
        match counts[i] {
            Some(reviews) => rows.push(ReviewerRow {
                login: login.clone(),
                reviews,
            }),
            None => unmeasured.push(login.clone()),
        }
    }
    // Ranked here AND again in the UI, for `board.rs`'s reason: chunks
    // complete out of order by design, so without a deterministic order two
    // equal rows swap between loads, which reads as a bug in the data rather
    // than in the sort. Ties break on LOGIN, the same tie-break the author
    // board uses, so the two boards order a tie identically.
    rows.sort_by(|a, b| {
        b.reviews
            .cmp(&a.reviews)
            .then_with(|| a.login.cmp(&b.login))
    });

    Ok(Reviewers {
        rows,
        unmeasured,
        refused_fields: refused,
        spend: budget.snapshot(),
    })
}

/// Which scopes route through the connection rather than search.
///
/// Exposed so a caller can report WHICH guarantee it has before issuing
/// the request, and so the routing rule has one name.
pub fn routes_through_connection(scope: &Scope) -> bool {
    !scope.needs_search() && scope.owner_name().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads are capped, and the number is not inherited from the
    /// mutation cap by accident. Both halves asserted: a cap must exist,
    /// and it must be a DECISION -- equal to 4 would mean someone copied
    /// `BATCH_CONCURRENCY` without reading why it is 4.
    #[test]
    fn reads_have_their_own_justified_cap() {
        // `const` blocks, so these bounds fail the BUILD rather than a
        // test run -- the values are compile-time constants, and a guard
        // on a constant may as well be checked where it cannot be
        // skipped by running a subset of the suite.
        const {
            assert!(READ_CONCURRENCY >= 1);
            // Bounded above by the reasoning in the doc comment: the
            // penalty for tripping a secondary limit is a 60-second
            // stall (octocrab's min_wait_seconds), not a cheap retry.
            assert!(
                READ_CONCURRENCY <= 8,
                "concurrent reads past what the doc comment justifies"
            );
        }
        // And the mutation cap is still its own number, for its own
        // reason. If it moves, the asymmetry argument in the doc above
        // needs re-reading.
        let src = include_str!("../../commands.rs");
        assert!(
            src.contains("const BATCH_CONCURRENCY: usize = 4"),
            "the mutation cap moved; re-read why reads differ from it"
        );
    }

    /// The cap is enforced PER PROCESS, not per call (#844).
    ///
    /// # Why this is a source scan rather than a concurrency test
    ///
    /// The property is "no stats POST bypasses the permit", which is a
    /// statement about every call site rather than about observable behaviour
    /// at one of them. A test that spawned 30 futures and counted peak
    /// concurrency would prove the semaphore works -- which is tokio's job,
    /// not this module's -- while saying nothing about a SEVENTH call site
    /// added later that calls `stats_graphql` directly. That seventh call site
    /// is the defect: the cap was real within each command and meaningless
    /// across the five a Stats page fires (`StatsPage.tsx:126-151`).
    ///
    /// So the scan asserts the chokepoint is the only door. `metered_read` is
    /// the one function that may call `stats_graphql`, and it holds a permit
    /// and records the spend -- which also means a new read cannot take a
    /// permit and then forget to meter itself, the `fetch_viewer` half of
    /// #844.
    #[test]
    fn every_stats_read_goes_through_the_process_wide_permit() {
        // The permit COUNT asserted at the end of this test reads
        // process-wide state, so it is false for as long as any other test
        // has a load in flight -- which #1044's timeout tests deliberately
        // do. Serialised against them rather than weakened: the assertion
        // that the cap is the documented number is worth keeping exact.
        // `blocking_lock` because this is a plain `#[test]` with no
        // runtime of its own; inside one it would panic rather than wait.
        let _permits = crate::github::stats::budget::READ_PERMIT_TEST_LOCK.blocking_lock();
        for (file, src) in [
            ("fetch.rs", include_str!("fetch.rs")),
            ("tree.rs", include_str!("tree.rs")),
            ("board.rs", include_str!("board.rs")),
            ("slice.rs", include_str!("slice.rs")),
        ] {
            // Production half only: a doc comment may legitimately name the
            // method it is explaining, and the test module below discusses it.
            let prod = src.split_once("\n#[cfg(test)]").map_or(src, |(p, _)| p);
            for (i, line) in prod.lines().enumerate() {
                if !line.contains("stats_graphql(") {
                    continue;
                }
                // The one permitted caller, and only inside `metered_read`.
                let inside_chokepoint =
                    file == "fetch.rs" && line.contains("client.stats_graphql(&body)");
                // A doc comment or ordinary comment mentioning the name.
                let t = line.trim_start();
                let is_comment = t.starts_with("//");
                assert!(
                    inside_chokepoint || is_comment,
                    "{file}:{} calls `stats_graphql` directly, bypassing the \
                     process-wide READ_PERMITS. A Stats page fires five \
                     independent commands, so a per-call cap is not a cap \
                     (#844) -- route it through `metered_read`, which holds a \
                     permit and records the spend.\n    {line}",
                    i + 1
                );
            }
        }
        // And the permit count IS the documented cap, rather than a second
        // number that could drift from it.
        assert_eq!(READ_PERMITS.available_permits(), READ_CONCURRENCY);
    }

    /// A whole-load ceiling exists, and it is NOT `poll::FETCH_TIMEOUT`
    /// -- that number is set by a property (staying under
    /// `MIN_FOCUSED_SECS`) that a user-initiated load does not have.
    #[test]
    fn the_load_has_a_wall_clock_ceiling_of_its_own() {
        assert!(LOAD_TIMEOUT >= std::time::Duration::from_secs(30));
        // A ceiling past a minute is one nobody waits for, and this is
        // behind an explicit load so the user is watching.
        assert!(LOAD_TIMEOUT <= std::time::Duration::from_secs(120));
        // It must exceed a realistic load: ~8s of serial probe rounds
        // plus ~10s of parallel detail waves, measured. Three times that
        // is the headroom `get_pr_detail` argues for.
        assert!(LOAD_TIMEOUT >= std::time::Duration::from_secs(50));
    }

    /// The ladder sheds NODES before aliases, because the work is per
    /// node: on dense real data, halving the page took the document from
    /// 11.0s to 3.5s where halving the aliases could at best have
    /// reached ~7s. See [`degrade`] for the measurements and for why my
    /// first guess was the other way round.
    #[test]
    fn the_ladder_sheds_nodes_before_aliases() {
        let (chunk, page) = degrade(ALIAS_CHUNK, SLICE_PAGE_FULL).expect("first rung");
        assert_eq!(page, SLICE_PAGE_REDUCED);
        assert_eq!(chunk, ALIAS_CHUNK, "parallelism is kept on rung one");
    }

    /// Parameterising the threshold did not change the COUNT path.
    ///
    /// `plan` used to branch on `ProbedSlice::too_big()`, which is
    /// `count >= SUBDIVIDE_AT`, and now branches on `count < subdivide_at`
    /// with the count path passing `slice::SUBDIVIDE_AT`. Those are exact
    /// complements at that value, so the shipped behaviour is unchanged -- but
    /// "unchanged" is a claim about two conditions written in opposite
    /// directions in different functions, which is precisely the kind of
    /// refactor that silently shifts a boundary by one.
    ///
    /// Asserted across the boundary rather than at one value, because an
    /// off-by-one is invisible anywhere else.
    #[test]
    fn the_count_path_keeps_its_exact_subdivision_boundary() {
        use slice::{ProbedSlice, SUBDIVIDE_AT};
        for count in [
            0,
            1,
            SUBDIVIDE_AT - 2,
            SUBDIVIDE_AT - 1,
            SUBDIVIDE_AT,
            SUBDIVIDE_AT + 1,
            slice::SEARCH_CAP,
            u64::MAX,
        ] {
            let probed = ProbedSlice {
                slice: Slice::new("2026-08-01", "2026-08-31"),
                count,
            };
            // The OLD condition and the NEW one, on the same input.
            let kept_by_old = !probed.too_big();
            let kept_by_new = probed.count < SUBDIVIDE_AT;
            assert_eq!(
                kept_by_old, kept_by_new,
                "count {count}: the parameterised condition must be the exact \
                 complement of `too_big()` at the count path's threshold"
            );
        }
    }

    /// Refusals ACROSS chunks are summed, not overwritten.
    ///
    /// `__refused` is a top-level key on every response rather than an alias,
    /// so the alias-merge loop made the last refusing chunk's count win. Three
    /// refusals in one chunk plus four in another reported **4**, and that
    /// figure is what `partialityCaveat` quotes beside the SAML remediation
    /// advice -- a wrong number makes a reader distrust the advice attached to
    /// it.
    ///
    /// Found in review. The shape is reproduced directly rather than through a
    /// mocked two-chunk fetch, because the bug is in the MERGE and a test that
    /// needed a network to reach it would not have been written.
    #[test]
    fn refusals_across_chunks_are_summed_not_overwritten() {
        // Two chunk responses, each carrying its own top-level count.
        let chunks = [
            serde_json::json!({ "s0": { "issueCount": 1, "nodes": [] }, "__refused": 3 }),
            serde_json::json!({ "s1": { "issueCount": 1, "nodes": [] }, "__refused": 4 }),
        ];
        // The merge this module performs, in the order it performs it.
        let mut merged = serde_json::Map::new();
        let mut refused = 0usize;
        for v in &chunks {
            if let Some(obj) = v.as_object() {
                for (k, val) in obj {
                    merged.insert(k.clone(), val.clone());
                }
            }
            refused += crate::github::client::refused_fields_of(v);
        }
        // The blanket insert alone loses the sum -- this is the bug, asserted
        // so a future edit that drops the explicit accumulation fails here
        // rather than understating a figure in the UI.
        assert_eq!(
            merged["__refused"].as_u64(),
            Some(4),
            "the alias merge alone keeps only the LAST count; if this ever \
             reads 7, the merge has started summing and the explicit \
             accumulation below is redundant"
        );
        assert_eq!(refused, 7, "the accumulator is what carries the total");
        if refused > 0 {
            merged.insert("__refused".into(), refused.into());
        }
        assert_eq!(
            crate::github::client::refused_fields_of(&serde_json::Value::Object(merged)),
            7,
            "the merged map must report the TOTAL, which is what the board reads"
        );
    }

    /// No refusals means the key is ABSENT, not zero.
    ///
    /// `graphql_partial_ok` inserts `__refused` only when there are refusals
    /// (`client.rs:1204-1213`), and the merge keeps that convention: a written
    /// 0 would be a second way of saying "none", and two spellings of one fact
    /// is how a reader ends up checking the wrong one.
    #[test]
    fn a_clean_load_writes_no_refusal_key() {
        let mut merged = serde_json::Map::new();
        merged.insert(
            "s0".into(),
            serde_json::json!({ "issueCount": 1, "nodes": [] }),
        );
        let refused = 0usize;
        if refused > 0 {
            merged.insert("__refused".into(), refused.into());
        }
        assert!(!merged.contains_key("__refused"));
        assert_eq!(
            crate::github::client::refused_fields_of(&serde_json::Value::Object(merged)),
            0,
            "an absent key reads as zero, which is why it need not be written"
        );
    }

    /// The DEFAULT page is already the reduced one, because 3 node-heavy
    /// aliases at `first: 100` intermittently 502'd on real data
    /// (11.0s then 8.1s). Going out at 100 would mean paying a wasted
    /// request and a ~11s stall on exactly the dense scopes this feature
    /// exists for.
    #[test]
    fn the_default_page_is_sized_for_dense_real_data() {
        const {
            assert!(SLICE_PAGE_FULL == 50);
            assert!(
                SLICE_PAGE_FULL < 100,
                "100 straddles the ~11s deadline on dense slices; re-measure first"
            );
        }
    }

    /// The ladder terminates, and the bottom rung is the smallest
    /// useful request rather than an infinite regress.
    #[test]
    fn the_ladder_reaches_a_bottom_and_stops() {
        let mut rung = (ALIAS_CHUNK, SLICE_PAGE_FULL);
        let mut steps = 0;
        while let Some(next) = degrade(rung.0, rung.1) {
            assert!(
                next.0 < rung.0 || next.1 < rung.1,
                "a rung that reduces nothing would loop: {rung:?} -> {next:?}"
            );
            rung = next;
            steps += 1;
            assert!(steps < 20, "the ladder does not terminate");
        }
        assert_eq!(rung, (1, SLICE_PAGE_REDUCED), "the smallest useful request");
    }

    /// The ladder this inherits from still exists, and this layer's pages
    /// are DELIBERATELY smaller than its.
    ///
    /// `MERGED_DETAIL_QUERY` goes out at 100 and degrades to 50, which is
    /// right for ONE search. This layer sends several searches in one
    /// document, so the same page size materialises several times the
    /// nodes -- which is why the measurements put the default a rung
    /// lower rather than copying the numbers across.
    #[test]
    fn this_layers_pages_are_deliberately_smaller_than_the_single_search_ladder() {
        let src = include_str!("../client.rs");
        assert!(
            src.contains("const PAGE_FULL: u32 = 100")
                && src.contains("const PAGE_REDUCED: u32 = 50"),
            "client.rs's page ladder moved; re-read client.rs:1024-1050"
        );
        const {
            assert!(
                SLICE_PAGE_FULL < 100,
                "an aliased document materialises its page once PER ALIAS"
            );
            assert!(SLICE_PAGE_REDUCED < SLICE_PAGE_FULL);
        }
    }

    /// Item 5's routing: only a well-formed single-repo scope takes the
    /// uncapped connection. A malformed one falls back to search, which
    /// still answers correctly.
    #[test]
    fn only_a_well_formed_single_repo_scope_takes_the_connection() {
        assert!(routes_through_connection(&Scope::Repo(
            "pktstorm/headstate".into()
        )));
        assert!(!routes_through_connection(&Scope::Repo("nameless".into())));
        assert!(!routes_through_connection(&Scope::Org("FNX-Labs".into())));
        assert!(!routes_through_connection(&Scope::Personal(
            "pktstorm".into()
        )));
        assert!(!routes_through_connection(&Scope::All(
            crate::github::stats::scope::AccountScope::new("octocat", Vec::new())
        )));
    }

    /// An `Outcome` cannot report a total without also reporting whether
    /// it is trustworthy. #824 item 8 in the type system.
    #[test]
    fn an_outcome_states_its_own_completeness() {
        let complete = Outcome {
            total: 337,
            retrievable: true,
            unretrievable: 0,
            slices: 1,
            rounds: 0,
            via_connection: true,
            spend: Budget::new().snapshot(),
            refused_fields: 0,
        };
        assert!(complete.is_complete());
        assert!(!complete.is_assembled());

        let capped = Outcome {
            retrievable: false,
            unretrievable: 260,
            slices: 12,
            ..complete.clone()
        };
        assert!(!capped.is_complete());
        assert!(capped.is_assembled());

        // A refusal is its own partiality channel, independent of the
        // cap -- inherited from `client.rs`'s partial-success handling.
        let refused = Outcome {
            refused_fields: 3,
            ..complete
        };
        assert!(
            !refused.is_complete(),
            "refused fields mean data is missing even when nothing was capped"
        );
    }

    /// A reviewer board states its own completeness, and a MEASURED zero is
    /// not a gap.
    ///
    /// The distinction this pins is the one the feature is built on, and on a
    /// ranking it binds harder than on a count: an unmeasured login rendered
    /// as `0` would place a colleague LAST on the strength of a query that
    /// never answered, and a top-five missing one person can have the wrong
    /// name in first place.
    ///
    /// It is also the state this board is usually in on real data. MEASURED
    /// 2026-09-11: `org:FNX-Labs` over a 30-day window holds 569 merged pull
    /// requests and ZERO reviewed by any of its four members -- the account
    /// merges without human review. So the true answer here looks exactly
    /// like a broken query, which is precisely why the two must not render
    /// the same.
    #[test]
    fn a_reviewer_board_distinguishes_a_measured_zero_from_an_unmeasured_login() {
        let all_zero = Reviewers {
            rows: vec![
                ReviewerRow {
                    login: "octocat".into(),
                    reviews: 0,
                },
                ReviewerRow {
                    login: "pktstorm".into(),
                    reviews: 0,
                },
            ],
            unmeasured: vec![],
            refused_fields: 0,
            spend: Budget::new().snapshot(),
        };
        // Everybody measured, everybody zero. That is a COMPLETE board
        // reporting an empty answer, which is the measured truth on this
        // account -- and the rows are kept rather than dropped so the UI can
        // say "no reviews given" rather than "nothing could be counted".
        assert!(all_zero.is_complete());
        assert_eq!(all_zero.rows.len(), 2);

        let short = Reviewers {
            unmeasured: vec!["hubot".into()],
            ..all_zero.clone()
        };
        assert!(
            !short.is_complete(),
            "a login that could not be counted makes the ranking partial, \
             because the leader might be the one that is missing"
        );

        // A refusal is its own channel, independent of a missing alias: it
        // suggests a SAML authorization to fix where a missing alias suggests
        // a retry, which is the same three-channel split `Board` carries.
        let refused = Reviewers {
            refused_fields: 2,
            ..all_zero
        };
        assert!(
            !refused.is_complete(),
            "refused fields mean data is missing even when every login answered"
        );
    }

    /// Absolute alias indices must cover every slice exactly once -- no
    /// repeat and no gap -- whatever the wave and chunk sizes.
    ///
    /// This is the property the whole merge rests on. Chunks complete out
    /// of order into one map keyed by alias, so a REPEATED index silently
    /// overwrites a slice's count and a GAP silently leaves it at zero --
    /// and zero reads as "no activity in that range", which is a wrong
    /// total that looks entirely right. The same failure
    /// `query.rs:667-678` guards for the history series.
    ///
    /// Derived from `enumerate()` rather than from pointer arithmetic: an
    /// earlier version of this computed the wave offset from the pointer
    /// distance between the subslice and its parent, which needed an
    /// `unsafe` block and a safety argument to save nothing over
    /// multiplying the wave number by the wave size.
    #[test]
    fn absolute_alias_indices_tile_every_slice_exactly_once() {
        // Boundaries either side of one wave and one chunk, plus an
        // empty input -- `chunks` yields no waves at all for 0, which is
        // the case a pointer-based offset would have had to special-case.
        for n in [0usize, 1, 9, 10, 11, 25, 60, 61, 137] {
            for chunk in [1usize, 3, ALIAS_CHUNK] {
                // Deliberately IDENTICAL ranges: the indices must not
                // depend on the contents being distinguishable.
                let slices: Vec<Slice> = (0..n)
                    .map(|_| Slice::new("2026-08-01", "2026-08-31"))
                    .collect();
                let per_wave = chunk * READ_CONCURRENCY;
                let mut seen = Vec::new();
                for (w, wave) in slices.chunks(per_wave).enumerate() {
                    let base = w * per_wave;
                    for (c, part) in wave.chunks(chunk).enumerate() {
                        let first = base + c * chunk;
                        for i in 0..part.len() {
                            seen.push(first + i);
                        }
                    }
                }
                seen.sort_unstable();
                assert_eq!(seen, (0..n).collect::<Vec<_>>(), "n={n} chunk={chunk}");
            }
        }
    }

    /// One wave is `ALIAS_CHUNK * READ_CONCURRENCY` slices, which is the
    /// number of slices one round-trip's worth of wall clock covers.
    /// Pinned because the doc comment on `READ_CONCURRENCY` reasons
    /// about exactly this product.
    #[test]
    fn a_wave_covers_sixty_slices() {
        const { assert!(ALIAS_CHUNK * READ_CONCURRENCY == 60) };
    }

    /// A wave is refused once the budget is under the reserve, and permitted
    /// while it is not -- which is what makes the mid-load check able to stop
    /// a load rather than merely being threaded through it (#843).
    ///
    /// Tested through `wave_permitted` itself rather than through a wiremock
    /// load, deliberately: the wave loops are four copies of one decision,
    /// and the decision is what has to be right. A mock load would assert it
    /// once for whichever loop the mock happened to exercise, and the other
    /// three would be covered by inspection -- which is how the in-advance
    /// gate came to be always-true at 4 of 4 call sites in the first place.
    ///
    /// The partial SHAPES each loop produces are asserted where they are
    /// read: `board.rs:480-493` for a missing detail alias, and
    /// `series_inner`/`reviewers_inner`'s own `None`-not-zero handling for
    /// the other two.
    /// THE regression test for #1050.
    ///
    /// A drained account refuses on WAVE ZERO: the loop breaks before issuing
    /// a single request, every day stays `None` and becomes a named failed
    /// day, and the function returns `Ok`. That is the state a large account
    /// is in on every page load, and the page told the user "GitHub did not
    /// answer the daily documents, which usually clears on its own" -- wrong
    /// in both halves, because GitHub was never asked and it clears when the
    /// hourly window resets.
    ///
    /// The existing `a_wave_is_refused_once_the_budget_is_under_the_reserve`
    /// covers the PREDICATE, and a mid-load refusal leaves some days measured.
    /// Nothing covered wave zero, which is why this shipped.
    ///
    /// No mock server on purpose: the client points at an unroutable port, so
    /// the test proves NO request is issued rather than asserting on a
    /// response. If the break on wave 0 ever stops happening, this fails
    /// instead of quietly passing.
    ///
    /// Asserts on the REASON, not merely on emptiness -- a `Series` that is
    /// empty for an unexplained reason IS the defect, and an assertion that
    /// only counted failed days would pass over it.
    #[tokio::test]
    async fn a_budget_exhausted_series_names_the_budget_rather_than_blaming_github() {
        use crate::github::stats::budget::RESERVE;
        use crate::github::stats::scope::{Measure, Scope, StatsQuery};

        // The budget is seeded on THIS load's own accumulator, not on the
        // process-wide figure, deliberately (#1048). `observed_test_lock`
        // returns a `std::sync::MutexGuard` and clippy's
        // `await_holding_lock` under `-D warnings` refuses to let one be
        // held across an `.await`, so an async test cannot serialise on it
        // -- and writing the shared static WITHOUT the lock is exactly the
        // race that burned the v5.20.0 tag.
        //
        // `Budget::permits` takes the LOWER of this accumulator and the
        // process figure, so a local reading under the reserve refuses the
        // wave whatever the process figure happens to be. The test needs no
        // shared state at all, which is why it can be async safely.
        let oc = octocrab::Octocrab::builder()
            .base_uri("http://127.0.0.1:1")
            .unwrap()
            .personal_token("test-token".to_string())
            .build()
            .unwrap();
        let client = GitHubClient::new(oc);
        // Seeded locally -- no write to the process-wide figure, so this
        // async test needs no lock and cannot starve another test's gate.
        let budget = Budget::seeded_for_test(RESERVE - 1, 1_789_412_400);
        let q = StatsQuery {
            subject: None,
            scope: Scope::Org("example".into()),
            measure: Measure::Merged,
        };
        let days: Vec<String> = (1..=30).map(|d| format!("2026-09-{d:02}")).collect();

        let series = series_inner(&client, &q, &days, &budget)
            .await
            .expect("a budget refusal is a successful call reporting nothing measured");

        assert!(series.points.is_empty(), "no day can have been measured");
        assert_eq!(
            series.failed_days.len(),
            days.len(),
            "every day in the window is unmeasured"
        );
        // One request accounted for: the seeding `record` above, which is
        // this test's own setup rather than a load. What matters is that the
        // LOAD issued none -- the client points at an unroutable port, so any
        // real request would have produced an error rather than an `Ok`, and
        // `failed_days` covering the whole window with `points` empty is what
        // "never asked" looks like from the outside.
        assert_eq!(
            series.spend.points, 0,
            "the signature of never asking: not one point spent"
        );
        assert_eq!(series.spend.requests, 0, "not one request issued");
        match series.unmeasured {
            Some(Unmeasured::BudgetExhausted { reserve, .. }) => {
                assert_eq!(reserve, RESERVE);
            }
            ref other => panic!(
                "#1050: a budget-exhausted load must NAME the budget, not leave \
                 the UI to guess that GitHub did not answer; got {other:?}"
            ),
        }
    }

    #[test]
    fn a_wave_is_refused_once_the_budget_is_under_the_reserve() {
        use crate::github::stats::budget::RESERVE;

        // `record` feeds the PROCESS-WIDE figure as well as this accumulator
        // (#843), so this test mutates state shared with `budget.rs`'s tests.
        // Serialised and restored, or it races them -- an intermittent failure
        // in another file caused by a test in this one.
        let _g = crate::github::stats::budget::observed_test_lock();
        let _restore = crate::github::stats::budget::RestoreObserved::capture();

        // A fresh `Budget` with a seeded process figure, which is exactly
        // the state a wave loop is in: the accumulator belongs to this load,
        // the remaining figure came from the poll loop or an earlier wave.
        let b = Budget::new();
        b.record(&serde_json::json!({
            "rateLimit": { "cost": 1, "remaining": RESERVE + 6, "resetAt": "2026-09-11T17:00:00Z" }
        }));
        // Six requests would leave exactly the reserve, which is permitted --
        // `RESERVE` is the floor, not a margin above it. Stated as
        // `READ_CONCURRENCY` because it happens to be 6, and a full wave at
        // the shipped concurrency is the realistic ask.
        assert!(wave_permitted(&b, READ_CONCURRENCY));
        assert!(
            !wave_permitted(&b, READ_CONCURRENCY + 1),
            "one request past the reserve must be refused"
        );

        // And once under the reserve, nothing is permitted.
        let b = Budget::new();
        b.record(&serde_json::json!({
            "rateLimit": { "cost": 1, "remaining": RESERVE - 1, "resetAt": "2026-09-11T17:00:00Z" }
        }));
        assert!(!wave_permitted(&b, 1));

        // With plenty of budget a full wave goes ahead, or the check would be
        // a refusal rather than a gate.
        let b = Budget::new();
        b.record(&serde_json::json!({
            "rateLimit": { "cost": 1, "remaining": 4_900, "resetAt": "2026-09-11T17:00:00Z" }
        }));
        assert!(wave_permitted(&b, READ_CONCURRENCY));
    }

    /// Every wave loop checks the budget before spawning its `JoinSet`.
    ///
    /// Asserted on the SOURCE, because there is no other way to tell: a
    /// mock-driven test of one loop says nothing about the other three, and
    /// the defect #843 describes is precisely that `budget` was threaded into
    /// all four and consulted by none of them. The `JoinSet` is the point of
    /// no return -- once it is spawned the requests are in flight -- so the
    /// check has to come before it in every loop.
    ///
    /// Counts `wave_permitted` call sites against `JoinSet::new` ones rather
    /// than naming the four functions, so a FIFTH wave loop added later
    /// cannot be the one that forgets.
    #[test]
    fn every_wave_loop_checks_the_budget_before_spawning() {
        let src = include_str!("fetch.rs");
        // Only the production half: the test module below deliberately
        // restates the tiling arithmetic with its own `JoinSet`-free loop,
        // and calls `wave_permitted` directly.
        let prod = src.split_once("\n#[cfg(test)]").expect("the test module").0;
        let spawners = prod.matches("JoinSet::new()").count();
        // `!wave_permitted(budget` -- the CALL in a loop guard, which is
        // negated every time because the guard stops the loop. Matching the
        // bare name would also count the function's own definition.
        let gates = prod.matches("!wave_permitted(budget").count();
        assert!(spawners > 0, "no wave loops found -- the scan is broken");
        assert_eq!(
            gates, spawners,
            "{spawners} wave loops spawn a JoinSet but only {gates} check \
             `wave_permitted` first. A loop that does not check cannot be \
             stopped mid-load, which is #843: `budget` was threaded into all \
             four and consulted by none."
        );
    }
}
