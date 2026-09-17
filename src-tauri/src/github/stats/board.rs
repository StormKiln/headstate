//! Per-author aggregates for one scope and window: the numbers behind
//! #826's Mine and Others views, and the three leaderboards.
//!
//! `fetch.rs` issues the requests and returns the raw alias map; this
//! module is the part that turns that map into people. The split is
//! deliberate and was left that way by #827 on purpose -- "`load_detail`
//! returns the raw alias map rather than a leaderboard struct, because
//! #826 defines that shape and this layer should not pre-empt it".
//!
//! # What a board is, and what it is NOT
//!
//! One row per author who appears in the window, carrying the four
//! measures a scope page shows: pull requests, lines changed, files
//! touched, reviews received. From those rows the UI takes two views --
//! the viewer's own row ("Mine") and everybody else's ("Others") -- plus
//! three rankings.
//!
//! It is not a census of the organisation. An author appears because they
//! merged or opened something in the window; a member with no activity is
//! **absent**, and that absence is the honest answer rather than a zero
//! row. #826 states the rule: "a member with no activity in the window
//! reads as 'no activity', not a zero that might be a failed query". The
//! difference matters because a zero and a refusal look identical once
//! they are both rendered as `0`, and two features in this repo have
//! already shipped that confusion (#802, #790).
//!
//! # Completeness, and where it can fail
//!
//! A leaderboard is a RANKING, which makes it far less forgiving of
//! missing data than a count is. A total that is 5% short is a slightly
//! wrong number; a top-five that is 5% short can have the wrong person in
//! first place. So [`Board`] carries three distinct partiality channels,
//! none of which collapses into another:
//!
//! | Field | What it means | Why it is its own field |
//! |---|---|---|
//! | [`Board::complete`] | every PR in the window was retrieved | a ranking over a sample is not a ranking |
//! | [`Board::truncated_slices`] | some slice returned fewer nodes than its own `issueCount` | names WHICH part is short, and by how much |
//! | [`Board::refused_fields`] | GitHub refused fields on a response | a refusal is not an absence |
//!
//! The third is the one #827 left open. Its `Outcome::refused_fields` is
//! `0` on the sliced path because "per-slice attribution of refusals needs
//! the detail-fetch mapping that #826 owns" -- this module is that
//! mapping, so the count is wired here rather than left at zero. See
//! [`Board::from_alias_map`].
//!
//! What it is NOT is per-slice attribution, and the difference is worth
//! stating rather than letting a reader assume the stronger property.
//! `graphql_partial_ok` folds a response's `errors` array into a single
//! `__refused` COUNT on `data` (`client.rs:1204-1213`), discarding the
//! `path` that says which field was refused. So a board can say how many
//! fields were refused across the load; it cannot say which author's
//! additions are short. Attributing a refusal to a slice would need
//! `graphql_partial_ok` to carry the paths through, which is a change to
//! 80 lines of hard-won behaviour that every other query in the app
//! depends on -- and the UI consequence is the same either way, since a
//! board with any refusal is presented as incomplete rather than as a
//! ranking with a footnote.
//!
//! # Measurements behind the document this reads
//!
//! Mine, live API, `gh api graphql`, 2026-09-11, `org:FNX-Labs`.
//!
//! ## `reviews { totalCount }` is FREE, and the epic's premise was wrong
//!
//! #823 and #826 both record that `reviews { totalCount }` is "a
//! connection, priced per search", and #826 settled the review leaderboard
//! as "proceed, reassess if it bites" on that basis. **It does not cost a
//! point.** What is priced is a connection that PAGES -- one carrying a
//! `first:` argument -- not one read for `totalCount` alone:
//!
//! | Nested field on each PR node | 3 searches | 6 searches | 15 searches |
//! |---|---|---|---|
//! | scalars only (the control) | 1 | 1 | 1 |
//! | `reviews { totalCount }` | 1 | 1 | 1 |
//! | `reviews(first: 1) { totalCount }` | 1 | **2** | **2** |
//! | `reviews(first: 1) { nodes { state } }` | 1 | **2** | **2** |
//! | `labels { totalCount }` | 1 | 1 | - |
//! | `labels(first: 10) { nodes { name } }` | 1 | **2** | - |
//! | `reviewThreads { totalCount }` | 1 | 1 | - |
//!
//! The same field is free without `first:` and priced with it, for both
//! `reviews` and `labels` -- so the rule is the ARGUMENT, not the field.
//! That reconciles rather than contradicts `poll.rs:1503-1580`: every
//! connection in the cost list there is a paged one (`labels(first:`,
//! `reviewThreads(first:`, `contexts(first:`), so its measurements were
//! right about the queries it was measuring, and the generalisation to
//! "connections are priced per search" is what was too broad.
//!
//! The practical consequence: `reviews { totalCount }` rides along on this
//! document at no cost, so the reviewer leaderboard #826 asked for needs
//! no retreat plan. A test pins the spelling, because the difference
//! between the free form and the priced one is four characters.
//!
//! ## The document must be SMALLER than #827's chunk default
//!
//! `ALIAS_CHUNK` is 10 and `SLICE_PAGE_FULL` is 50, and that product does
//! not survive real dense data. Against day-slices of `org:FNX-Labs` in
//! 2026-08 (245 merged PRs across 10 days), three runs per cell:
//!
//! | Document | Succeeded | Wall clock |
//! |---|---|---|
//! | 10 aliases x 50 | **0 of 3** | 10.6 / 10.6 / 10.6s -- all 502 |
//! | 10 aliases x 25 | 3 of 3 | 6.6-9.6s |
//! | 5 aliases x 50 | 3 of 3 | 4.2-4.9s |
//! | 5 aliases x 25 | 3 of 3 | 4.1-4.3s |
//! | 3 aliases x 50 | 3 of 3 | 2.3-3.7s |
//!
//! Every failure landed at 10.6s, which is #827's measured ~11s server
//! deadline exactly. Its own figure for this document was "3 aliases x 50:
//! 3.42-3.68s, ok 2/2" -- correct, and measured at a THIRD of the chunk
//! size it then shipped. Nothing between 3 and 10 was measured, and 10 is
//! where it breaks.
//!
//! I confirmed the cause is node volume rather than this date range or the
//! added `reviews` field: the identical 10 aliases count-only answered in
//! 1.4-1.5s, 10 x 50 carrying only scalars still straddled the deadline
//! (7.0-10.6s, 2 of 3), and 10 x 50 WITH `reviews` passed 3 of 3 at
//! 7.7-8.5s on a re-run. At 7-10s against an 11s wall the outcome is a
//! coin toss, which is why [`BOARD_ALIAS_CHUNK`] is 5 rather than 10.
//!
//! That is a narrowing of #827's constants for this document only, not a
//! correction of its reasoning -- its ladder already sheds pages before
//! aliases for exactly the reason these numbers show.

use std::collections::HashMap;

use super::budget::Budget;
use super::query::Slice;
use super::scope::{Scope, StatsQuery, Subject};
use crate::github::client::{ClientError, GitHubClient};
use crate::store::pr_history::StoredPr;

/// Searches per detail document, for the BOARD's document specifically.
///
/// `query::ALIAS_CHUNK` is 10, which measured 0 of 3 against real dense
/// day-slices -- see the module docs for the table. 5 is the widest cell
/// that succeeded 3 of 3 at the full 50-node page, at 4.2-4.9s against an
/// ~11s deadline: roughly half the budget, which is the margin a document
/// whose density the caller cannot predict actually needs.
///
/// Deliberately a SEPARATE constant rather than a change to
/// `ALIAS_CHUNK`, and the asymmetry is the point. `ALIAS_CHUNK` also sizes
/// the PROBE document, which is count-only and measured at 1.4-1.5s for
/// the same 10 aliases -- seven times faster, because it materialises no
/// nodes at all. Halving the probe chunk would double the planner's round
/// count for no measured reason, and the planner's rounds are serial by
/// construction.
pub const BOARD_ALIAS_CHUNK: usize = 5;

/// How many rows a leaderboard shows. #826 says top five.
///
/// Exported because the UI must not pick its own: a board cut to five in
/// Rust and re-cut to three in TypeScript would render a "top three" whose
/// fourth place was decided by a tie-break the UI never saw.
pub const TOP_N: usize = 5;

/// One person's activity in one scope and window.
///
/// Every field is a count over the PRs actually RETRIEVED, which is why
/// [`Board::complete`] sits beside the rows rather than inside them: a row
/// cannot say whether it is short, because a short row looks exactly like
/// a smaller one. That is the whole failure mode of a ranking.
///
/// No `Eq`, because `cycle_time_hours` holds `f64`. `PartialEq` is kept for
/// the tests, which compare whole rows deliberately -- a single-field
/// assertion passes while a figure is accumulated into the wrong bucket.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorRow {
    /// The GitHub login. The identity the search qualifier uses
    /// (`author:<login>`), so a row is checkable against GitHub's own UI.
    pub login: String,
    /// Pull requests by this author in the window, for the board's MEASURE.
    ///
    /// `Measure::Merged` counts merged ones and `Measure::Opened` counts
    /// opened ones -- they are not interchangeable, and a PR opened in July
    /// and merged in August belongs to July's opened count and August's merged
    /// count. The board is loaded with one measure, so this is a count of that
    /// one rather than of "pull requests" in general.
    ///
    /// Worth stating because it bounds what [`AuthorRow::cycle_time_hours`]
    /// can be. On the merged measure every counted pull request HAS merged, so
    /// the two differ only when a timestamp could not be parsed -- not because
    /// some are still open. A first version of the UI labelled that gap "still
    /// open, not counted", which described a mixed population this field never
    /// holds.
    pub prs: u64,
    /// Lines ADDED, summed. Raw `additions`, including generated files --
    /// see [`AuthorRow::lines_changed`] for why the label matters more
    /// than the number.
    pub additions: u64,
    /// Lines REMOVED, summed.
    pub deletions: u64,
    /// `changedFiles`, summed. A free scalar, and the honest companion to
    /// the line count: 40,000 lines across 3 files is a generated diff and
    /// 40,000 across 300 is a refactor, and only the pair distinguishes
    /// them.
    pub changed_files: u64,
    /// Reviews RECEIVED on this author's pull requests, summed.
    ///
    /// Received, not given, and the distinction is not a detail: this
    /// comes off `reviews { totalCount }` on a PR the author WROTE, so it
    /// measures how much review their work attracted. A leaderboard of
    /// reviews GIVEN would need `reviewed-by:<login>` -- one search per
    /// person, which is a different and much more expensive question. The
    /// field name and the UI label both have to say "received" or the
    /// board means the opposite of what a reader assumes.
    pub reviews_received: u64,
    /// Hours from open to merge, for each of this author's MERGED pull
    /// requests, sorted ascending.
    ///
    /// Kept as the distribution rather than reduced to a mean, for the
    /// reason `MergedDetail` already records: a median and a tail say
    /// different things, and an average cycle time is dominated by the one
    /// pull request somebody left open over a holiday. Sorted here so the
    /// UI's `percentile()` can index it directly, which is the contract
    /// `MergedDetail::cycle_time_hours` already has.
    ///
    /// SHORTER than `prs` whenever a pull request has no usable merge time,
    /// which the mapper drops rather than guessing at:
    ///
    /// - `mergedAt` is null -- an open pull request. A cycle time computed
    ///   against "now" would report it as slow rather than as unfinished, and
    ///   it gets slower every second it stays open.
    /// - the timestamps do not parse, or the merge precedes the creation. A
    ///   negative duration sorts to the TOP of an ascending distribution and
    ///   drags a median below zero, and clamping it to 0 would be a
    ///   measurement claim about a value nobody can explain.
    ///
    /// On the **merged** measure only the second case is reachable, because
    /// `is:merged` means every node has a `mergedAt`. That bounds what a UI
    /// may say about the gap: it is a timestamp problem, not unfinished work.
    /// A first version of `CycleTime` labelled it "still open, not counted",
    /// which on a merged-only board called a merged pull request unfinished.
    ///
    /// Either way the length is NOT `prs`, and nothing may divide one by the
    /// other.
    pub cycle_time_hours: Vec<f64>,
}

impl AuthorRow {
    fn new(login: String) -> Self {
        Self {
            login,
            prs: 0,
            additions: 0,
            deletions: 0,
            changed_files: 0,
            reviews_received: 0,
            cycle_time_hours: Vec::new(),
        }
    }

    /// Additions plus deletions.
    ///
    /// The metric #823 settled on, and it is GAMEABLE -- one checked-in
    /// generated file outranks a month of real work. The settled
    /// mitigation is the label, not a fix: the UI must say "lines changed,
    /// including generated files". That decision is recorded in #823 as
    /// accepted with its consequence, so this method exists to make the
    /// number available in exactly one place rather than have three call
    /// sites each add two fields and each label it differently.
    pub fn lines_changed(&self) -> u64 {
        self.additions.saturating_add(self.deletions)
    }
}

/// A slice whose nodes could not all be retrieved.
///
/// Carried as a STRUCT rather than a count, because "3 slices were short"
/// does not tell a user whether the board is missing four PRs or four
/// hundred. The window is named so the gap is locatable: a reader can ask
/// GitHub the same question for that range and see what is missing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortSlice {
    pub from: String,
    pub to: String,
    /// What GitHub said the slice holds.
    pub issue_count: u64,
    /// How many nodes actually came back.
    pub retrieved: u64,
}

impl ShortSlice {
    /// PRs this slice is missing from the board.
    pub fn missing(&self) -> u64 {
        self.issue_count.saturating_sub(self.retrieved)
    }
}

/// Per-author aggregates for one scope and window, with every way they
/// could be wrong stated alongside them.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    /// One row per author who appears, in NO guaranteed order -- the UI
    /// ranks by whichever measure its chart is about, and a board that
    /// arrived pre-sorted by one of them would invite a caller to render
    /// "top by lines" from an order that is actually by PR count.
    pub rows: Vec<AuthorRow>,
    /// Pull requests GitHub says the window holds, from `issueCount`.
    ///
    /// Exact even when the rows are short -- the 1,000 cap limits
    /// retrieval, not counting (`slice.rs:52`). So this is the number a
    /// "based on N of M" label quotes, and comparing it against
    /// [`Board::retrieved`] is how the UI knows to draw that label at all.
    ///
    /// # Why it is `Option` (#1092)
    ///
    /// `None` means NOBODY HAS MEASURED IT. A board assembled from stored
    /// rows whose window has no ledger coverage holds pull requests and
    /// has no denominator for them, and a 0 there would render as "400 of
    /// 0" or -- worse, because it reads as reassuring -- "400 of 400,
    /// complete". Absent is not zero, and a denominator is the one field
    /// where the lie is invisible: every ratio built from it looks
    /// plausible.
    ///
    /// The window in which this is `None` is short by design: the
    /// backfill's first tick probes the whole horizon for ~1 point and
    /// makes it `Some`. That is exactly why defaulting it would be
    /// dangerous rather than harmless -- the shortest window would be the
    /// most confidently wrong one.
    pub total: Option<u64>,
    /// Pull requests actually aggregated into the rows.
    pub retrieved: u64,
    /// Whether every PR in the window made it into a row.
    ///
    /// The flag a ranking must branch on. Computed from all three
    /// partiality channels rather than from `retrieved == total`, so a
    /// refusal that happened not to shorten the node list still reads as
    /// incomplete.
    pub complete: bool,
    /// Slices that came back short, with their sizes.
    pub truncated_slices: Vec<ShortSlice>,
    /// Fields GitHub refused across the detail responses.
    ///
    /// #827 left `Outcome::refused_fields` at 0 on the sliced path and
    /// flagged it rather than silently zeroing it, because attribution
    /// needs this mapping. It is counted here, per response, summed.
    pub refused_fields: usize,
    /// Slices the window was cut into. > 1 means assembled.
    pub slices: usize,
    /// Probe rounds the plan took.
    pub rounds: u32,
    /// What the whole load cost.
    pub spend: super::budget::Spend,
    /// The slowest merged pull requests in scope, slowest first.
    ///
    /// Kept on the board rather than derived per author, because the
    /// interesting outlier is usually the scope's and not one person's --
    /// and because the list is short and bounded by [`OUTLIERS`] either way.
    pub slowest: Vec<BoardPr>,
    /// The largest merged pull requests in scope by lines changed.
    pub largest: Vec<BoardPr>,
    /// Merged pull requests per repository, most first.
    ///
    /// The scoped counterpart to `MergedDetail::repo_counts`. Over a single
    /// repository scope this is one row, which is correct rather than
    /// useless: it confirms the scope.
    pub repo_counts: Vec<RepoCount>,
    /// Pull requests held in `store::pr_history` for this window, across
    /// every load (#1004).
    ///
    /// The fourth partiality channel, and the one that distinguishes
    /// CONVERGING from STUCK. `retrieved` says what THIS load managed;
    /// this says what the answer is assembled from. Today's caveat reads
    /// identically whether the next load will help or not, and the
    /// difference between "1,419 of 2,942" twice and "1,419 then 2,219"
    /// is the whole of what the reporter asked for.
    ///
    /// Equal to `retrieved` on a board built from one fetch with nothing
    /// stored, so a small account's numbers are unchanged.
    pub accumulated: u64,
    /// Whether accumulation is in play for this board at all.
    ///
    /// False for a board assembled from a single fetch with no storage
    /// behind it -- a single-repo connection scope, or a load whose
    /// database write failed. The UI must not promise "this improves on
    /// the next load" when nothing is being written down, which would be
    /// #841's fail-open in a new costume: a claim we cannot keep,
    /// presented as a fact.
    pub accumulating: bool,
    /// Days of the window the ledger has actually retrieved (#1092).
    ///
    /// The figure a pull request count cannot give. "40% of the pull
    /// requests" is compatible with 40% of every day and with 100% of 40%
    /// of the days, and only the second tells a reader WHICH PART of the
    /// chart to trust -- the part they are usually looking at.
    ///
    /// A day is counted here because a settled ledger row covers it, never
    /// because it held no pull requests. An uncovered day is uncovered.
    pub days_covered: usize,
    /// Days in the window.
    ///
    /// The denominator of "34 of 90 days measured". 0 when the window's
    /// bounds could not be read, which renders as visibly wrong rather
    /// than invisibly wrong.
    pub days_total: usize,
}

/// How many outliers each list holds.
///
/// Five, matching both [`TOP_N`] and `MergedDetail`'s own outlier lists --
/// so the page has one idea of "a few of the most extreme" rather than three
/// numbers a reader has to notice differ.
pub const OUTLIERS: usize = 5;

/// One pull request, enough to name and open it.
///
/// Mirrors `MergedPr`, which `Outliers` already renders, so the scoped
/// outliers go through the SAME component rather than a second one. #826 asks
/// for reuse of the existing components and this is the type that makes it
/// possible.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardPr {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// `owner/name`.
    pub repo: String,
    pub author: String,
    pub cycle_time_hours: f64,
    /// Additions plus deletions -- the same gameable measure, labelled the
    /// same way wherever it appears.
    pub size: u64,
}

/// Merged pull requests in one repository.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepoCount {
    pub repo: String,
    pub merged: u64,
}

impl Board {
    /// How many PRs are missing from the rows, or `None` when the total
    /// has not been measured.
    ///
    /// `None` rather than 0, because the two mean opposite things and 0 is
    /// the reassuring one: "nothing is missing" is exactly what a reader
    /// would conclude, and it is the one conclusion an unmeasured
    /// denominator cannot support. A caller that wants a number must
    /// decide what to say about the unknown case rather than being handed
    /// a plausible default -- which is the whole reason [`Board::total`]
    /// is an `Option`.
    pub fn missing(&self) -> Option<u64> {
        self.total.map(|t| t.saturating_sub(self.retrieved))
    }

    /// Whether the board was assembled from more than one request.
    pub fn is_assembled(&self) -> bool {
        self.slices > 1
    }

    /// The row for one login, or `None` when that person has no activity
    /// in the window.
    ///
    /// `None` rather than a zero row, deliberately. "No activity" and
    /// "zero" are the same number and different facts, and the caller has
    /// to be able to tell them apart to honour #826's empty-means-empty
    /// rule -- a zero row would let the UI render a confident `0` for a
    /// person whose data simply is not here.
    pub fn row_for(&self, login: &str) -> Option<&AuthorRow> {
        self.rows.iter().find(|r| r.login == login)
    }

    /// Every row except one login's -- the "Others" view's population.
    ///
    /// Takes the login rather than a `Subject`, because the viewer's login
    /// is resolved by the time a board is rendered and `Subject::Viewer`
    /// would make this method unable to answer.
    pub fn others(&self, login: &str) -> impl Iterator<Item = &AuthorRow> {
        let login = login.to_string();
        self.rows.iter().filter(move |r| r.login != login)
    }

    /// The top rows by one measure, highest first, at most [`TOP_N`].
    ///
    /// Ties break on LOGIN, ascending, rather than being left to the
    /// sort's stability. Two people with the same line count would
    /// otherwise swap places between loads depending on which slice
    /// happened to land first -- a leaderboard that reorders on refresh
    /// with no data change reads as broken, and the arrival order of
    /// concurrent requests is genuinely nondeterministic here
    /// (`fetch.rs`'s waves complete out of order by design).
    pub fn top_by<F>(&self, measure: F) -> Vec<&AuthorRow>
    where
        F: Fn(&AuthorRow) -> u64,
    {
        let mut rows: Vec<&AuthorRow> = self.rows.iter().filter(|r| measure(r) > 0).collect();
        rows.sort_by(|a, b| {
            measure(b)
                .cmp(&measure(a))
                .then_with(|| a.login.cmp(&b.login))
        });
        rows.truncate(TOP_N);
        rows
    }

    /// Build a board from `fetch::load_detail`'s alias map.
    ///
    /// # Why the slices go in alongside the map
    ///
    /// The map is keyed by ALIAS (`s0`, `s4`, ...), and an alias index is
    /// meaningless without the slice it was assigned to. The truncation
    /// report needs the date range, so the caller passes the same slice
    /// list it passed to `load_detail`, in the same order -- the absolute
    /// indexing rule `query.rs:667-678` records, applied one layer up.
    ///
    /// # A missing alias is not an empty slice
    ///
    /// Exactly the rule `fetch::probe_round` states for the probe: a slice
    /// whose alias is absent from the map did not come back, and counting
    /// it as zero PRs "would make a slice that failed look empty -- which
    /// is the exact shape that turns a truncation into a confident wrong
    /// number". Here the consequence is worse than a wrong total, because
    /// a ranking built over a silently-dropped slice can put the wrong
    /// person first. So an absent alias is recorded as a short slice of
    /// unknown size rather than skipped.
    pub fn from_alias_map(
        map: &serde_json::Value,
        slices: &[Slice],
        rounds: u32,
        spend: super::budget::Spend,
    ) -> Self {
        let mut by_login: HashMap<String, AuthorRow> = HashMap::new();
        let mut by_repo: HashMap<String, u64> = HashMap::new();
        // Every merged PR, so the outliers can be chosen after the whole
        // board is known. Kept whole rather than maintaining two
        // five-element heaps: a window is bounded by the slice plan and the
        // cap, so this is thousands of small structs at worst, and a heap
        // would be three times the code for a saving nobody would measure.
        let mut prs: Vec<BoardPr> = Vec::new();
        let mut total = 0u64;
        let mut retrieved = 0u64;
        let mut truncated = Vec::new();
        let refused_fields = crate::github::client::refused_fields_of(map);

        for (i, slice) in slices.iter().enumerate() {
            let alias = super::query::slice_alias(i);
            let entry = &map[&alias];
            // `issueCount` absent means this alias did not answer. The
            // slice is reported short with a zero retrieved count and an
            // UNKNOWN true size -- the honest shape, since the only
            // figure that could have said how big it was is the one that
            // is missing.
            let Some(issue_count) = entry["issueCount"].as_u64() else {
                truncated.push(ShortSlice {
                    from: slice.from.clone(),
                    to: slice.to.clone(),
                    issue_count: 0,
                    retrieved: 0,
                });
                continue;
            };
            total = total.saturating_add(issue_count);

            let nodes = entry["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            let mut got = 0u64;
            for n in nodes {
                // A PR whose author is gone from GitHub resolves
                // `author` to null. Attributed to a single named bucket
                // rather than dropped: dropping it would make the rows
                // sum to less than `retrieved` and put the board's own
                // arithmetic out, and inventing a per-PR placeholder
                // would scatter one absence across many rows.
                let login = n["author"]["login"].as_str().unwrap_or(GHOST);
                let row = by_login
                    .entry(login.to_string())
                    .or_insert_with(|| AuthorRow::new(login.to_string()));
                row.prs = row.prs.saturating_add(1);
                row.additions = row
                    .additions
                    .saturating_add(n["additions"].as_u64().unwrap_or(0));
                row.deletions = row
                    .deletions
                    .saturating_add(n["deletions"].as_u64().unwrap_or(0));
                row.changed_files = row
                    .changed_files
                    .saturating_add(n["changedFiles"].as_u64().unwrap_or(0));
                row.reviews_received = row
                    .reviews_received
                    .saturating_add(n["reviews"]["totalCount"].as_u64().unwrap_or(0));

                // Cycle time only for a MERGED pull request. An open one has
                // a null `mergedAt`, and measuring it against "now" would
                // report unfinished work as slow -- which is the one reading
                // that turns an honest figure into an accusation. So an
                // unmerged PR contributes to `prs` and to the line counts
                // and to nothing else, and `cycle_time_hours` is therefore
                // shorter than `prs`: the type's doc says so, because a
                // caller dividing one by the other would be wrong.
                let hours = cycle_hours(n);
                if let Some(h) = hours {
                    row.cycle_time_hours.push(h);
                }

                let repo = n["repository"]["nameWithOwner"].as_str().unwrap_or("");
                if !repo.is_empty() && hours.is_some() {
                    // Merged only, matching `MergedDetail::repo_counts`'s
                    // own meaning. A repo count that mixed open and merged
                    // would not be comparable with the merged total above it.
                    *by_repo.entry(repo.to_string()).or_insert(0) += 1;
                }
                // Outlier candidates are merged pull requests: "slowest to
                // merge" is undefined for one that has not merged, and
                // "largest" beside it would then be drawn from a different
                // population than the list next to it.
                if let Some(h) = hours {
                    prs.push(BoardPr {
                        number: n["number"].as_u64().unwrap_or(0),
                        title: n["title"].as_str().unwrap_or("").to_string(),
                        url: n["url"].as_str().unwrap_or("").to_string(),
                        repo: repo.to_string(),
                        author: login.to_string(),
                        cycle_time_hours: h,
                        size: n["additions"].as_u64().unwrap_or(0)
                            + n["deletions"].as_u64().unwrap_or(0),
                    });
                }
                got += 1;
            }
            retrieved = retrieved.saturating_add(got);

            // Short means GitHub's own count for this slice exceeds what
            // it returned for it. Checked per slice rather than on the
            // window total, because the window total can match by
            // accident while two slices are short and long respectively
            // -- and the board would then be confidently wrong.
            if got < issue_count {
                truncated.push(ShortSlice {
                    from: slice.from.clone(),
                    to: slice.to.clone(),
                    issue_count,
                    retrieved: got,
                });
            }
        }

        let mut rows: Vec<AuthorRow> = by_login.into_values().collect();
        // Sorted by LOGIN, not by any measure. The board is ranked by the
        // caller per chart, and a stable order here is what makes the
        // serialized value comparable between loads and in tests.
        rows.sort_by(|a, b| a.login.cmp(&b.login));
        for r in &mut rows {
            // Ascending, which is the contract `percentile()` on the UI side
            // indexes against -- `MergedDetail::cycle_time_hours` documents
            // the same requirement. `total_cmp` rather than `partial_cmp`:
            // these are durations and cannot be NaN, but `partial_cmp`
            // would need an `unwrap` whose failure mode is a panic in a
            // mapper, and `total_cmp` has no failure mode at all.
            r.cycle_time_hours.sort_by(f64::total_cmp);
        }

        // Outliers picked AFTER the whole window is mapped, so they are the
        // scope's extremes rather than the last slice's. Ties break on the
        // repo and number so two equally slow pull requests do not swap
        // places between loads -- `fetch.rs`'s waves complete out of order
        // by design, so without a tie-break the list would reshuffle on
        // refresh with no data change.
        let mut slowest = prs.clone();
        slowest.sort_by(|a, b| {
            b.cycle_time_hours
                .total_cmp(&a.cycle_time_hours)
                .then_with(|| a.repo.cmp(&b.repo))
                .then_with(|| a.number.cmp(&b.number))
        });
        slowest.truncate(OUTLIERS);
        let mut largest = prs;
        largest.sort_by(|a, b| {
            b.size
                .cmp(&a.size)
                .then_with(|| a.repo.cmp(&b.repo))
                .then_with(|| a.number.cmp(&b.number))
        });
        largest.truncate(OUTLIERS);

        let mut repo_counts: Vec<RepoCount> = by_repo
            .into_iter()
            .map(|(repo, merged)| RepoCount { repo, merged })
            .collect();
        repo_counts.sort_by(|a, b| b.merged.cmp(&a.merged).then_with(|| a.repo.cmp(&b.repo)));

        Self {
            rows,
            // `Some`, and measured: this board came from a fetch, so
            // GitHub's own `issueCount` for every slice is in hand. The
            // `None` case belongs to a board assembled from storage whose
            // window nothing has probed -- see `from_stored`.
            total: Some(total),
            retrieved,
            // All three channels, not just the node count. A refusal can
            // leave the node list the right LENGTH while blanking fields
            // inside it, and `retrieved == total` would then read as
            // complete over data GitHub admitted it did not give.
            complete: truncated.is_empty() && refused_fields == 0 && retrieved == total,
            truncated_slices: truncated,
            refused_fields,
            slices: slices.len(),
            rounds,
            spend,
            slowest,
            largest,
            repo_counts,
            // A board straight off one fetch has nothing stored behind it
            // yet. `commands::stats_board` overwrites both once it has
            // written the pages down and re-assembled -- and if that write
            // fails, these stay honest rather than promising a convergence
            // that is not happening.
            accumulated: retrieved,
            accumulating: false,
            // A board from one fetch knows the slices it planned, and a
            // planned slice IS a measured day. The ledger figure replaces
            // this the moment storage is consulted (`from_stored`).
            days_covered: day_span(slices),
            days_total: day_span(slices),
        }
    }

    /// Every pull request this load retrieved, for accumulation.
    ///
    /// The outlier lists are TRUNCATED to [`OUTLIERS`], so they are not the
    /// population -- writing them down would store five pull requests per
    /// load and converge on nothing. This is the full retrieved set, which
    /// is what `store::pr_history` needs.
    ///
    /// Returned from the alias map rather than kept on [`Board`] because a
    /// `Board` is serialised into `stats_cache`'s payload, and carrying
    /// every pull request there would put the whole corpus in the answer
    /// cache as well as in the table beneath it.
    ///
    /// # The day a row is filed under (#1092)
    ///
    /// Every row carries the DATE its slice ranged over, which is what
    /// makes a stored pull request evidence about a day rather than about
    /// a question. Read from the node's own `mergedAt`, because that is
    /// the date GitHub itself matched against the `merged:` qualifier.
    ///
    /// When a node carries no readable date the row is filed under the
    /// slice's own `from`. That is exact for the day-sized slices a board
    /// plans (`SLICE_PAGE_FULL` subdivides to the date grammar's floor on
    /// any busy scope) and, for a wider slice, is a date the row genuinely
    /// falls on or after -- so the row lands inside the range that
    /// retrieved it and can never be attributed to a day outside it. The
    /// alternative, dropping the row, would lose a pull request that was
    /// paid for.
    pub fn retrieved_prs(map: &serde_json::Value, slices: &[Slice]) -> Vec<StoredPr> {
        let mut out = Vec::new();
        for (i, slice) in slices.iter().enumerate() {
            let alias = super::query::slice_alias(i);
            let entry = &map[&alias];
            let nodes = entry["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            for n in nodes {
                let repo = n["repository"]["nameWithOwner"].as_str().unwrap_or("");
                // A node with no repository cannot be keyed, and a row
                // under an empty repo would collide with every other such
                // node on `(repo, number)`. Skipped rather than stored
                // under a placeholder: it stays absent from the table, so
                // the window keeps reading as incomplete, which is the
                // honest direction.
                if repo.is_empty() {
                    continue;
                }
                // Merged only, matching the population the outlier lists
                // and `repo_counts` are drawn from. `cycle_hours` is what
                // distinguishes a merged pull request from an open one.
                let Some(hours) = cycle_hours(n) else {
                    continue;
                };
                let additions = n["additions"].as_u64().unwrap_or(0);
                let deletions = n["deletions"].as_u64().unwrap_or(0);
                out.push(StoredPr {
                    repo: repo.to_string(),
                    number: n["number"].as_u64().unwrap_or(0),
                    merged_at: merged_day(n).unwrap_or_else(|| slice.from.clone()),
                    title: n["title"].as_str().unwrap_or("").to_string(),
                    url: n["url"].as_str().unwrap_or("").to_string(),
                    author: n["author"]["login"].as_str().unwrap_or(GHOST).to_string(),
                    cycle_time_hours: hours,
                    size: additions + deletions,
                    additions,
                    deletions,
                    changed_files: n["changedFiles"].as_u64().unwrap_or(0),
                    reviews_received: n["reviews"]["totalCount"].as_u64().unwrap_or(0),
                });
            }
        }
        out
    }

    /// Re-assemble a board from everything stored for a window (#1004).
    ///
    /// This is where accumulation becomes an ANSWER. `from_alias_map`
    /// builds a board from one fetch; this builds one from the union of
    /// every fetch, so two partial loads produce a board containing both
    /// their pull requests rather than only the later one's.
    ///
    /// # Completeness is computed against the WINDOW, not the fetch
    ///
    /// `total` stays GitHub's `issueCount` for the window -- exact even
    /// when retrieval is short, which is what makes the shortfall
    /// detectable at all (`slice.rs:52`). `complete` becomes "every pull
    /// request in this window is in the table", which is #1004's
    /// definition and a STRICT EXTENSION of the existing flag rather than
    /// a replacement: a single complete load still satisfies it, because
    /// its rows are all stored.
    ///
    /// The refusal and truncation channels are kept from the fetch that
    /// produced them. They do not become false merely because rows
    /// accumulated -- a refused field is still refused, and #824's rule is
    /// that partiality shrinks rather than becoming invisible.
    /// # The denominator comes from the LEDGER (#1092)
    ///
    /// `coverage` is what the `pr_slice` ledger says about this window:
    /// which days were retrieved, and GitHub's exact total across them.
    /// It is the only honest source for a board assembled from storage,
    /// because the fetch this board is paired with may have covered a
    /// fraction of the window -- or, for a board served entirely from
    /// accumulated rows, none of it.
    ///
    /// A `Coverage::total` of `None` passes straight through to
    /// `Board::total`. That is the case the whole `Option` exists for:
    /// rows on disk and nothing that ever measured how many there should
    /// be. Defaulting it to the fetched total would look right and be
    /// wrong whenever the fetch covered less than the window.
    pub fn from_stored(
        stored: &[StoredPr],
        fetched: &Board,
        coverage: &crate::store::pr_slice::Coverage,
        days_total: usize,
    ) -> Self {
        let mut by_login: HashMap<String, AuthorRow> = HashMap::new();
        let mut by_repo: HashMap<String, u64> = HashMap::new();
        let mut prs: Vec<BoardPr> = Vec::with_capacity(stored.len());

        for s in stored {
            let row = by_login
                .entry(s.author.clone())
                .or_insert_with(|| AuthorRow::new(s.author.clone()));
            row.prs = row.prs.saturating_add(1);
            row.additions = row.additions.saturating_add(s.additions);
            row.deletions = row.deletions.saturating_add(s.deletions);
            row.changed_files = row.changed_files.saturating_add(s.changed_files);
            row.reviews_received = row.reviews_received.saturating_add(s.reviews_received);
            row.cycle_time_hours.push(s.cycle_time_hours);
            *by_repo.entry(s.repo.clone()).or_insert(0) += 1;
            prs.push(s.to_board_pr());
        }

        let mut rows: Vec<AuthorRow> = by_login.into_values().collect();
        rows.sort_by(|a, b| a.login.cmp(&b.login));
        for r in &mut rows {
            r.cycle_time_hours.sort_by(f64::total_cmp);
        }

        // The same tie-breaks `from_alias_map` uses, for the same reason:
        // without them the lists reshuffle between loads with no data
        // change.
        let mut slowest = prs.clone();
        slowest.sort_by(|a, b| {
            b.cycle_time_hours
                .total_cmp(&a.cycle_time_hours)
                .then_with(|| a.repo.cmp(&b.repo))
                .then_with(|| a.number.cmp(&b.number))
        });
        slowest.truncate(OUTLIERS);
        let mut largest = prs;
        largest.sort_by(|a, b| {
            b.size
                .cmp(&a.size)
                .then_with(|| a.repo.cmp(&b.repo))
                .then_with(|| a.number.cmp(&b.number))
        });
        largest.truncate(OUTLIERS);

        let mut repo_counts: Vec<RepoCount> = by_repo
            .into_iter()
            .map(|(repo, merged)| RepoCount { repo, merged })
            .collect();
        repo_counts.sort_by(|a, b| b.merged.cmp(&a.merged).then_with(|| a.repo.cmp(&b.repo)));

        let accumulated = stored.len() as u64;
        // The ledger's figure when it has one; otherwise the fetch's,
        // which is exact for the window the fetch actually planned. Both
        // can be `None`, and a `None` survives to the UI rather than being
        // filled in here.
        let total = coverage.total.or(fetched.total);
        Self {
            rows,
            total,
            // What THIS load retrieved is kept as it was. It is a fact
            // about the fetch, and overwriting it with the accumulated
            // figure would erase the distinction the caveat needs.
            retrieved: fetched.retrieved,
            // Computed against the WINDOW: every pull request GitHub says
            // is here is in the table. The fetch's own refusal channel
            // still counts -- a refused field is not cured by a row
            // arriving from somewhere else.
            // Completeness needs a denominator, a full day list, and no
            // refusals. An UNKNOWN total can never satisfy it: `None`
            // makes this false, which is the honest direction -- a board
            // that does not know how many pull requests it is missing is
            // not a board that can claim to be missing none.
            //
            // The day check is what makes this true to the window rather
            // than to the fetch: every pull request GitHub counted for the
            // days we measured can be present while whole days remain
            // unasked, and that board is not complete.
            complete: total.is_some_and(|t| accumulated >= t)
                && coverage.days_covered() >= days_total
                && !coverage.partial
                && fetched.refused_fields == 0,
            truncated_slices: fetched.truncated_slices.clone(),
            refused_fields: fetched.refused_fields,
            slices: fetched.slices,
            rounds: fetched.rounds,
            spend: fetched.spend.clone(),
            slowest,
            largest,
            repo_counts,
            accumulated,
            accumulating: true,
            days_covered: coverage.days_covered(),
            days_total,
        }
    }
}

/// Hours from open to merge, or `None` for a pull request that has not
/// merged.
///
/// `None` rather than a duration against "now", deliberately: an open pull
/// request measured that way gets slower every second it stays open and
/// would dominate a "slowest to merge" list while not having failed to merge
/// at all. The distinction is available only because `mergedAt` is null on
/// those, so it is read rather than inferred from a state field the document
/// does not carry.
///
/// A NEGATIVE result is also `None`. It should be impossible -- a merge
/// cannot precede its own creation -- but the two timestamps come from
/// GitHub independently, and a negative cycle time would sort to the TOP of
/// an ascending distribution and drag a median below zero. Dropped rather
/// than clamped to 0, because a clamped zero is a measurement claim and this
/// is a value nobody can explain.
fn cycle_hours(node: &serde_json::Value) -> Option<f64> {
    let created = node["createdAt"].as_str()?;
    let merged = node["mergedAt"].as_str()?;
    let c = chrono::DateTime::parse_from_rfc3339(created).ok()?;
    let m = chrono::DateTime::parse_from_rfc3339(merged).ok()?;
    let hours = (m - c).num_seconds() as f64 / 3600.0;
    (hours >= 0.0).then_some(hours)
}

/// How many distinct days a slice list spans.
///
/// The day figure for a board built from one fetch, where every planned
/// slice was fetched and is therefore measured. Deduplicated across slices
/// so an overlap cannot report more days than the calendar holds -- the
/// planner tiles without overlap, but a figure that could exceed its own
/// denominator is one a reader cannot act on, so it is made impossible
/// here rather than assumed upstream.
///
/// An unparseable bound contributes nothing, matching
/// `pr_slice::clamped_days`: a range this cannot read must not claim days
/// it cannot verify.
fn day_span(slices: &[Slice]) -> usize {
    let mut days = std::collections::BTreeSet::new();
    for s in slices {
        let (Some(from), Some(to)) = (
            chrono::NaiveDate::parse_from_str(&s.from, "%Y-%m-%d").ok(),
            chrono::NaiveDate::parse_from_str(&s.to, "%Y-%m-%d").ok(),
        ) else {
            continue;
        };
        let mut d = from;
        while d <= to {
            days.insert(d);
            d += chrono::Duration::days(1);
        }
    }
    days.len()
}

/// The `YYYY-MM-DD` day a node belongs to, for the slice ledger (#1092).
///
/// `mergedAt` is the date GitHub matched against the `merged:` qualifier,
/// so it is the day the retrieving slice ranged over -- taking the first
/// ten characters of the RFC 3339 timestamp rather than parsing and
/// re-formatting, because the wire format is fixed and a round trip
/// through a local timezone is exactly how a row lands on the wrong day.
///
/// `None` when the field is absent or too short to be a date. The caller
/// falls back to the slice's own start rather than dropping the row: a
/// pull request that was paid for should not be lost to an unreadable
/// field, and the slice bound is a date it provably falls on or after.
fn merged_day(node: &serde_json::Value) -> Option<String> {
    let merged = node["mergedAt"].as_str()?;
    (merged.len() >= 10).then(|| merged[..10].to_string())
}

/// The bucket for a pull request whose author no longer exists.
///
/// A literal rather than an `Option<String>` on the row, because the row's
/// `login` is what every chart labels and what `row_for` matches -- making
/// it optional would push a `None` case into the UI for a situation that
/// is rare, unactionable, and already honestly described by a name. The
/// leading space cannot occur in a GitHub login, so this can never collide
/// with a real person.
pub const GHOST: &str = "(deleted user)";

/// Load a board for one scope and window.
///
/// Reuses #827's layer end to end and adds no second pagination path: the
/// planner probes and subdivides (`slice.rs`), `load_detail_chunked`
/// fetches the nodes with its degradation ladder, and this module maps the
/// result. #826's instruction was explicit that the slicing layer is to be
/// used rather than reimplemented, and the reason is stronger than reuse:
/// the planner's tiling property (contiguous, no gap, no overlap) is what
/// makes a board complete, and it is asserted by `slice.rs`'s tests. A
/// second path would have to re-earn that.
///
/// The subject is `None` by construction. A board counts EVERYONE in the
/// scope and ranks them, so an `author:` qualifier would return a board
/// with one row -- `scope.rs` has a test named for exactly that mistake.
/// The viewer's own numbers come from `Board::row_for`, not from a
/// narrower query, which is also why Mine and Others cost ONE load
/// between them rather than two.
pub async fn load_board(
    client: &GitHubClient,
    scope: &Scope,
    measure: super::scope::Measure,
    window: Slice,
    budget: &Budget,
) -> Result<Board, ClientError> {
    // ONE ceiling around the WHOLE load, which is the rule
    // `fetch::LOAD_TIMEOUT`'s own doc states: "only a wall-clock ceiling
    // around the whole thing bounds what the user is actually waiting on".
    //
    // Without this the board had TWO sequential 60-second bounds -- the
    // planner's and the detail fetch's -- so it could legitimately run 120s
    // where `load_count` bounds itself at 60. Found in review. The inner
    // bounds are left in place: they are what the other callers of those
    // functions get, and the outer one is what makes this caller obey the same
    // contract as the count path.
    load_board_accumulating(client, scope, measure, window, budget)
        .await
        .map(|loaded| loaded.board)
}

/// A board plus the pull requests it retrieved, for accumulation (#1004).
///
/// `load_board` returns only the board, which is all the single-repo and
/// test callers want. `commands::stats_board` needs the retrieved set as
/// well so it can write the pages down -- including from a load that
/// later hits the cap, which is the entire point of #1004.
#[derive(Debug, Clone)]
pub struct LoadedBoard {
    pub board: Board,
    /// Every merged pull request this load retrieved, ready for
    /// `store::pr_history::put_many`.
    pub prs: Vec<StoredPr>,
}

/// [`load_board`], keeping the retrieved pull requests.
pub async fn load_board_accumulating(
    client: &GitHubClient,
    scope: &Scope,
    measure: super::scope::Measure,
    window: Slice,
    budget: &Budget,
) -> Result<LoadedBoard, ClientError> {
    load_board_within(
        client,
        scope,
        measure,
        window,
        budget,
        super::fetch::LOAD_TIMEOUT,
    )
    .await
}

/// [`load_board_accumulating`] against a caller-supplied ceiling.
///
/// # Why the ceiling is a parameter
///
/// So the RETENTION can be tested (#1044). The behaviour under test is
/// "the ceiling expired part-way through a fetch and the pull requests
/// already retrieved survived", and the shipped ceiling is 60 seconds --
/// a test that waited for it would take a minute, and one that faked the
/// clock would exercise a timer rather than the drop. With the ceiling as
/// an argument the test drives THIS function, the one that ships, against
/// a stalling server and a ceiling short enough to expire between two
/// waves.
///
/// `LOAD_TIMEOUT` itself is unchanged and is not raised: #1044 is
/// explicit that the defect is discarding the work, not the size of the
/// budget, and `fetch.rs`'s own doc derives the 60s from measurement.
/// There is exactly one production caller and it passes `LOAD_TIMEOUT`.
async fn load_board_within(
    client: &GitHubClient,
    scope: &Scope,
    measure: super::scope::Measure,
    window: Slice,
    budget: &Budget,
    ceiling: std::time::Duration,
) -> Result<LoadedBoard, ClientError> {
    // Both owned HERE, outside the future the ceiling may drop (#1044).
    // That placement is the fix: `tokio::time::timeout` destroys the
    // future and everything it owns, so anything the partial answer is
    // built from has to live on this side of the race.
    let progress = BoardProgress::default();
    let sink = super::fetch::PartialDetail::new();
    let started = std::time::Instant::now();
    match tokio::time::timeout(
        ceiling,
        board_inner(client, scope, measure, window, budget, &progress, &sink),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => partial_on_timeout(&progress, &sink, budget, started.elapsed(), ceiling),
    }
}

/// What the dropped future had got as far as, for the timeout branch.
///
/// Only the plan: the retrieved nodes live in `fetch::PartialDetail`,
/// which the fetch layer already owns for the same reason. This carries
/// what the MAPPER needs and the fetch layer does not know -- the slice
/// list the aliases are indexed against, and the probe rounds the plan
/// took.
#[derive(Debug, Clone, Default)]
struct BoardProgress(std::sync::Arc<std::sync::Mutex<Option<PlannedSlices>>>);

/// The plan a partial board is mapped against.
#[derive(Debug, Clone)]
struct PlannedSlices {
    slices: Vec<Slice>,
    rounds: u32,
}

impl BoardProgress {
    fn publish(&self, planned: PlannedSlices) {
        // Poisoning recovered from rather than panicked on, for
        // `PartialDetail::record`'s reason: this is an accumulator with no
        // invariant a mid-write panic could break, and dying here would
        // convert a partial answer into no answer.
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(planned);
    }

    fn get(&self) -> Option<PlannedSlices> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Build a board out of what a timed-out load had already retrieved.
///
/// # Partial is not nothing
///
/// The load that expires here has typically issued dozens of successful
/// POSTs, each paid for against the rate limit. #1004 built
/// `LoadedBoard::prs` so those would be written down "including from a
/// load that later hits the cap, which is the entire point of #1004", and
/// the timeout branch was the single path that dropped them instead. The
/// user-visible consequence was that eventual consistency never advanced
/// on exactly the scopes slow enough to need it: a fast scope did not need
/// accumulation and a slow one saved nothing, so no number of retries
/// converged.
///
/// # Why the board is still marked incomplete
///
/// `complete` is forced false whatever the numbers say. This codebase's
/// rule is only-low -> qualify, possibly-wrong -> suppress, and a board
/// assembled from an unknown fraction of its slices is only-low on the
/// per-author figures (every row is a floor) but possibly-WRONG on the
/// ranking between them. `complete = false` is what the UI branches on to
/// qualify the first and withhold the second, so it is set at the source
/// rather than left to a caller to infer.
///
/// # Why this is still an error when nothing was retrieved
///
/// A plan that never published, or one whose every slice is still
/// outstanding, has nothing to qualify. Returning an empty board would
/// present "we measured nobody" as a measurement, which is the
/// zero-for-absent confusion this module's header exists to forbid. So the
/// timeout error survives for exactly that case.
fn partial_on_timeout(
    progress: &BoardProgress,
    sink: &super::fetch::PartialDetail,
    budget: &Budget,
    elapsed: std::time::Duration,
    ceiling: std::time::Duration,
) -> Result<LoadedBoard, ClientError> {
    let secs = ceiling.as_secs();
    let (waves, slices_done) = sink.progress();
    let Some(planned) = progress.get() else {
        // The ceiling expired during PLANNING, so there are no slices to
        // index the aliases against and no detail request was ever issued.
        crate::diag!(
            "[diag] stats board TIMEOUT after {:?} (ceiling {}s): \
             expired while planning, nothing retrieved",
            elapsed,
            secs
        );
        return Err(ClientError::Timeout(secs));
    };
    let outstanding = planned.slices.len().saturating_sub(slices_done);
    let map = sink.snapshot();
    let prs = Board::retrieved_prs(&map, &planned.slices);
    if slices_done == 0 {
        crate::diag!(
            "[diag] stats board TIMEOUT after {:?} (ceiling {}s): \
             {} slices planned, 0 covered, {} outstanding, 0 pull requests kept",
            elapsed,
            secs,
            planned.slices.len(),
            outstanding
        );
        return Err(ClientError::Timeout(secs));
    }
    // The SAME mapper as the success path. An alias no wave reached is
    // simply absent from the snapshot, and `from_alias_map` already reads
    // an absent alias as a named short slice of unknown size -- the shape
    // #843 built for a refused wave. So a partial board arrives already
    // naming the ranges it did not read, and there is no second mapper to
    // keep in step with the first.
    let mut board = Board::from_alias_map(&map, &planned.slices, planned.rounds, budget.snapshot());
    // Unconditional, and NOT `&&`-ed with anything. A load that ran out of
    // wall clock is incomplete even if every alias it did reach came back
    // whole, because the aliases it never reached are the ones missing --
    // and `plan.is_retrievable()`, which the success path checks
    // separately, can only ever push the same way. One assignment rather
    // than two, so there is no reading of this function under which the
    // flag comes back true.
    board.complete = false;
    crate::diag!(
        "[diag] stats board TIMEOUT after {:?} (ceiling {}s): \
         {} slices planned, {} covered by {} completed waves, {} outstanding, \
         {} pull requests kept, {} short slices, {} points spent",
        elapsed,
        secs,
        planned.slices.len(),
        slices_done,
        waves,
        outstanding,
        prs.len(),
        board.truncated_slices.len(),
        budget.spent()
    );
    Ok(LoadedBoard { board, prs })
}

async fn board_inner(
    client: &GitHubClient,
    scope: &Scope,
    measure: super::scope::Measure,
    window: Slice,
    budget: &Budget,
    progress: &BoardProgress,
    sink: &super::fetch::PartialDetail,
) -> Result<LoadedBoard, ClientError> {
    let q = StatsQuery::new(None, scope.clone(), measure);
    // Subdivided to the PAGE, not to the search cap. This is the line that
    // makes a board a ranking rather than a sample, and it was a real defect
    // in my first implementation -- see `fetch::plan_to` for the measurement:
    // at the count path's threshold of 800, a 30-day org window of 569 merged
    // pull requests planned as ONE slice and retrieved 50 of them.
    let plan = super::fetch::plan_to(
        client,
        &q,
        window,
        budget,
        u64::from(super::fetch::SLICE_PAGE_FULL),
    )
    .await?;
    let slices: Vec<Slice> = plan.slices.iter().map(|s| s.slice.clone()).collect();
    // Published BEFORE the fetch, so a ceiling expiring mid-fetch can
    // still index the sink's aliases against the slices they name (#1044).
    progress.publish(PlannedSlices {
        slices: slices.clone(),
        rounds: plan.rounds,
    });
    let map = super::fetch::load_detail_into(client, &q, &slices, budget, BOARD_ALIAS_CHUNK, sink)
        .await?;
    let mut board = Board::from_alias_map(&map, &slices, plan.rounds, budget.snapshot());
    // An irreducible slice cannot be divided further, so its nodes are a
    // sample BY CONSTRUCTION -- before any request was made. Folded in here
    // rather than left to the node-count comparison in the mapper, which
    // would catch it only if the response also happened to come back short.
    //
    // This is reachable on the board's threshold in a way it is not on the
    // count's: a SINGLE DAY holding more than one page of pull requests
    // cannot be cut (GitHub's search grammar has no sub-day range), so a very
    // busy day is a genuine partial. On the measured window the busiest day
    // held 47 against a 50-node page, so there is headroom -- but a busier
    // org will reach it, and then the board says so rather than quietly
    // ranking over part of a day.
    if !plan.is_retrievable() {
        board.complete = false;
    }
    // Harvested from the SAME map the board was built from, so the rows
    // written down and the board returned cannot describe different
    // fetches.
    let prs = Board::retrieved_prs(&map, &slices);
    Ok(LoadedBoard { board, prs })
}

/// Whether a login is the ghost bucket rather than a person.
///
/// Exists so the UI can decline to link it to a GitHub profile without
/// duplicating the literal.
pub fn is_ghost(login: &str) -> bool {
    login == GHOST
}

/// The viewer's login, for splitting a board into Mine and Others.
///
/// A thin wrapper over `Subject::cache_key`, which already resolves
/// `Subject::Viewer` against a fetched login. Here so a caller holding a
/// `Subject` does not have to know that the cache-key method is also the
/// "who is this, really" method.
pub fn resolve_login(subject: &Subject, viewer: &str) -> String {
    subject.cache_key(viewer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A spend for a mapper test, which measures no cost.
    ///
    /// Built here rather than by deriving `Default` on `Spend` itself,
    /// deliberately: a derived default is a zero-point spend, and
    /// `Spend::is_exact` would report it as an exact zero. That is a
    /// measurement claim -- "this load cost nothing" -- available to
    /// production code by accident. `unmetered: 1` marks it as a floor
    /// instead, which is the honest reading of a spend nobody measured.
    fn unmeasured() -> super::super::budget::Spend {
        super::super::budget::Spend {
            points: 0,
            requests: 0,
            unmetered: 1,
            remaining: None,
            reset_at: None,
        }
    }

    /// A client pointed at a mock server, matching `client.rs`'s own
    /// test helper rather than a second way of building one.
    async fn mock_client(server: &wiremock::MockServer) -> GitHubClient {
        let oc = octocrab::Octocrab::builder()
            .base_uri(server.uri())
            .unwrap()
            .personal_token("test-token".to_string())
            .build()
            .unwrap();
        GitHubClient::new(oc)
    }

    /// Every slice alias a request's document actually selects.
    ///
    /// Read off the DOCUMENT rather than assumed from the call, because
    /// the point of these tests is that the layer under test decides how
    /// to chunk -- a fixture that hard-coded the aliases would keep
    /// passing if the chunking changed underneath it.
    fn aliases_in(doc: &str) -> Vec<String> {
        doc.lines()
            .filter_map(|l| {
                l.trim()
                    .split(": search(")
                    .next()
                    .filter(|_| l.contains(": search("))
            })
            .map(|a| a.trim().to_string())
            .collect()
    }

    /// A probe answer: a count over the subdivision threshold for every
    /// alias asked for, so the planner keeps cutting until it reaches
    /// single days it cannot cut.
    fn probe_body(aliases: &[String]) -> serde_json::Value {
        let mut data = serde_json::Map::new();
        for a in aliases {
            data.insert(a.clone(), json!({ "issueCount": 5000 }));
        }
        json!({ "data": data })
    }

    /// A detail answer: ONE merged pull request per alias, numbered by the
    /// alias so the retained set can be told apart from a duplicate.
    ///
    /// `issueCount` matches the node count, so a slice that answers is not
    /// ALSO reported short -- the assertion about `truncated_slices` in the
    /// timeout test would otherwise pass on the answered slices and say
    /// nothing about the stalled ones.
    fn detail_body(aliases: &[String]) -> serde_json::Value {
        let mut data = serde_json::Map::new();
        for a in aliases {
            let n: u64 = a.trim_start_matches('s').parse().unwrap_or(0);
            data.insert(
                a.clone(),
                json!({
                    "issueCount": 1,
                    "nodes": [{
                        "number": n + 1,
                        "title": format!("pull request {n}"),
                        "url": format!("https://github.com/acme/repo/pull/{}", n + 1),
                        "repository": { "nameWithOwner": "acme/repo" },
                        "author": { "login": "alice" },
                        "createdAt": "2026-07-01T00:00:00Z",
                        "mergedAt": "2026-07-01T01:00:00Z",
                        "additions": 10,
                        "deletions": 2,
                        "changedFiles": 1,
                        "reviews": { "totalCount": 1 },
                    }]
                }),
            );
        }
        json!({ "data": data })
    }

    fn slices(n: usize) -> Vec<Slice> {
        (0..n)
            .map(|i| {
                let d = i + 1;
                Slice::new(format!("2026-08-{d:02}"), format!("2026-08-{d:02}"))
            })
            .collect()
    }

    /// A merged pull request node, with a one-hour cycle time.
    ///
    /// Carries the identity fields too, so a fixture exercises the same
    /// shape the live document returns rather than a subset that would let
    /// an outlier-mapping bug pass.
    fn node(login: &str, add: u64, del: u64, files: u64, reviews: u64) -> serde_json::Value {
        json!({
            "number": 1,
            "title": "a pull request",
            "url": "https://github.com/owner/repo/pull/1",
            "repository": { "nameWithOwner": "owner/repo" },
            "author": { "login": login },
            "createdAt": "2026-08-01T00:00:00Z",
            "mergedAt": "2026-08-01T01:00:00Z",
            "additions": add,
            "deletions": del,
            "changedFiles": files,
            "reviews": { "totalCount": reviews },
        })
    }

    /// A node with the caller's identity, timestamps and size, for the
    /// outlier and cycle-time tests where those are the subject.
    #[allow(clippy::too_many_arguments)]
    fn pr(
        login: &str,
        number: u64,
        repo: &str,
        created: &str,
        merged: Option<&str>,
        add: u64,
        del: u64,
    ) -> serde_json::Value {
        json!({
            "number": number,
            "title": format!("pr {number}"),
            "url": format!("https://github.com/{repo}/pull/{number}"),
            "repository": { "nameWithOwner": repo },
            "author": { "login": login },
            "createdAt": created,
            "mergedAt": merged,
            "additions": add,
            "deletions": del,
            "changedFiles": 1,
            "reviews": { "totalCount": 0 },
        })
    }

    /// The happy path: two slices, three authors, every figure summed
    /// across slices rather than per slice.
    ///
    /// Asserted on the WHOLE row rather than on one field, because the
    /// bug this guards is a field accumulated into the wrong bucket --
    /// which a single-field check passes while the board is wrong.
    #[test]
    fn rows_aggregate_across_slices() {
        let map = json!({
            "s0": { "issueCount": 2, "nodes": [
                node("alice", 10, 5, 2, 1),
                node("bob", 100, 0, 1, 0),
            ]},
            "s1": { "issueCount": 2, "nodes": [
                node("alice", 1, 1, 1, 3),
                node("carol", 7, 7, 7, 7),
            ]},
        });
        let b = Board::from_alias_map(&map, &slices(2), 1, unmeasured());
        assert!(b.complete, "nothing was short: {:?}", b.truncated_slices);
        assert_eq!(b.total, Some(4));
        assert_eq!(b.retrieved, 4);
        assert_eq!(
            b.row_for("alice"),
            Some(&AuthorRow {
                login: "alice".into(),
                prs: 2,
                additions: 11,
                deletions: 6,
                changed_files: 3,
                reviews_received: 4,
                // Both of alice's fixtures are merged an hour after opening.
                cycle_time_hours: vec![1.0, 1.0],
            })
        );
        assert_eq!(b.row_for("alice").unwrap().lines_changed(), 17);
        // Three authors, and nobody invented.
        let mut logins: Vec<&str> = b.rows.iter().map(|r| r.login.as_str()).collect();
        logins.sort_unstable();
        assert_eq!(logins, ["alice", "bob", "carol"]);
    }

    /// A slice that returned fewer nodes than its own `issueCount` makes
    /// the board INCOMPLETE and names the gap.
    ///
    /// This is the failure a ranking cannot survive silently: #802 and
    /// #790 both shipped a truncation that looked like a smaller number.
    #[test]
    fn a_short_slice_is_named_and_sized() {
        let map = json!({
            "s0": { "issueCount": 50, "nodes": [node("alice", 1, 1, 1, 0)] },
        });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        assert!(!b.complete, "a short slice must not read as complete");
        assert_eq!(b.total, Some(50));
        assert_eq!(b.retrieved, 1);
        assert_eq!(b.missing(), Some(49));
        assert_eq!(
            b.truncated_slices,
            vec![ShortSlice {
                from: "2026-08-01".into(),
                to: "2026-08-01".into(),
                issue_count: 50,
                retrieved: 1,
            }]
        );
        assert_eq!(b.truncated_slices[0].missing(), 49);
    }

    /// An alias MISSING from the map is reported, not treated as an empty
    /// slice.
    ///
    /// `fetch::probe_round` states the rule for the probe document and
    /// this is the same rule for the detail document. The stakes are
    /// higher here: a dropped slice in a ranking can change who is first,
    /// where in a count it only shrinks a total.
    #[test]
    fn a_missing_alias_is_not_an_empty_slice() {
        // Two slices planned, only one came back.
        let map = json!({
            "s0": { "issueCount": 1, "nodes": [node("alice", 1, 1, 1, 0)] },
        });
        let b = Board::from_alias_map(&map, &slices(2), 1, unmeasured());
        assert!(!b.complete, "a dropped slice must not read as complete");
        assert_eq!(
            b.truncated_slices.len(),
            1,
            "the absent alias must be reported: {:?}",
            b.truncated_slices
        );
        assert_eq!(b.truncated_slices[0].from, "2026-08-02");
        // Its true size is UNKNOWN, and claiming 0 would be a guess.
        assert_eq!(b.truncated_slices[0].issue_count, 0);
    }

    /// A REFUSED field makes the board incomplete even when the node
    /// count matches.
    ///
    /// This is the channel #827 left at 0 on the sliced path, flagged
    /// rather than silently zeroed. A refusal can blank fields inside
    /// nodes that are all present, so `retrieved == total` is not enough
    /// to call a board complete.
    ///
    /// The fixture carries `__refused` rather than an `errors` array,
    /// which is the shape this layer actually receives and is worth
    /// stating because it is not the shape GitHub sends.
    /// `graphql_partial_ok` consumes the `errors` array and injects the
    /// count INTO `data` (`client.rs:1204-1213`), so by the time a
    /// response reaches a mapper the errors are gone and the count is a
    /// field. A fixture built from GitHub's own wire format would
    /// therefore pass this test while asserting nothing -- which is how
    /// it failed first time round.
    #[test]
    fn a_refused_field_is_not_an_absence() {
        let map = json!({
            "s0": { "issueCount": 1, "nodes": [node("alice", 1, 1, 1, 0)] },
            "__refused": 3,
        });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        assert_eq!(Some(b.retrieved), b.total, "the node count itself matched");
        assert_eq!(
            b.refused_fields, 3,
            "the refusal count must be carried, not dropped"
        );
        assert!(
            !b.complete,
            "a board over refused fields must not read as complete"
        );
    }

    /// The refusal channel is read through `client::refused_fields_of`,
    /// not through a second reading of the response.
    ///
    /// Guarded because the key is `__refused` -- a private-looking name
    /// that a reader would not guess, and that a second implementation
    /// would spell differently while still compiling and still returning
    /// 0. A board that always reported 0 refusals is exactly the state
    /// #827 left behind and this module exists to fix, so the wiring
    /// needs an assertion rather than only a call.
    #[test]
    fn the_refusal_count_comes_from_the_clients_own_reader() {
        let map = json!({ "__refused": 7, "s0": { "issueCount": 0, "nodes": [] } });
        assert_eq!(
            crate::github::client::refused_fields_of(&map),
            7,
            "the client's reader is the one source for this count"
        );
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        assert_eq!(b.refused_fields, 7);
    }

    /// A person with NO activity has no row -- not a zero row.
    ///
    /// #826's empty-means-empty rule. `None` is what lets the UI print
    /// "no activity" instead of a `0` that might be a failed query.
    #[test]
    fn absence_is_absence_not_zero() {
        let map = json!({
            "s0": { "issueCount": 1, "nodes": [node("alice", 1, 1, 1, 0)] },
        });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        assert!(b.row_for("alice").is_some());
        assert_eq!(
            b.row_for("bob"),
            None,
            "an inactive member must be absent, not zero"
        );
    }

    /// Ranking is by the measure asked for, highest first, capped at
    /// TOP_N, and a zero is excluded rather than padding the board.
    #[test]
    fn top_by_ranks_on_the_measure_it_is_given() {
        let map = json!({
            "s0": { "issueCount": 6, "nodes": [
                // Most PRs, fewest lines.
                node("many_prs", 1, 0, 1, 0),
                node("many_prs", 1, 0, 1, 0),
                node("many_prs", 1, 0, 1, 0),
                // Fewest PRs, most lines -- a generated diff, which is
                // exactly the gameability #823 accepted and labelled.
                node("big_diff", 90_000, 10_000, 2, 0),
                node("reviewed", 5, 5, 5, 42),
                node("quiet", 0, 0, 0, 0),
            ]},
        });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());

        let by_prs = b.top_by(|r| r.prs);
        assert_eq!(by_prs[0].login, "many_prs");
        let by_lines = b.top_by(|r| r.lines_changed());
        assert_eq!(
            by_lines[0].login, "big_diff",
            "the three rankings must be able to disagree, or only one is needed"
        );
        let by_reviews = b.top_by(|r| r.reviews_received);
        assert_eq!(by_reviews[0].login, "reviewed");

        // `quiet` merged a PR with no lines and no reviews. It appears on
        // the PR board and on NEITHER of the other two: a zero has no
        // rank, and padding a top-five with zeroes presents people as
        // ranked on a measure they do not appear in at all.
        assert!(by_prs.iter().any(|r| r.login == "quiet"));
        assert!(!by_lines.iter().any(|r| r.login == "quiet"));
        assert!(!by_reviews.iter().any(|r| r.login == "quiet"));
    }

    /// A tie breaks on login, so the board does not reorder between
    /// loads when nothing changed.
    ///
    /// The arrival order of `fetch.rs`'s concurrent waves is
    /// nondeterministic by design, so without an explicit tie-break two
    /// equal rows would swap on refresh -- which reads as a bug in the
    /// data rather than in the sort.
    #[test]
    fn ties_break_deterministically() {
        let map = json!({
            "s0": { "issueCount": 2, "nodes": [
                node("zoe", 10, 0, 1, 0),
                node("adam", 10, 0, 1, 0),
            ]},
        });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        let top = b.top_by(|r| r.lines_changed());
        assert_eq!(
            top.iter().map(|r| r.login.as_str()).collect::<Vec<_>>(),
            ["adam", "zoe"]
        );
    }

    /// TOP_N is honoured, and the cap is the reason the constant is
    /// exported rather than spelled in the UI.
    #[test]
    fn top_by_caps_at_top_n() {
        let nodes: Vec<serde_json::Value> = (0..12)
            .map(|i| node(&format!("user{i:02}"), 100 - i, 0, 1, 0))
            .collect();
        let map = json!({ "s0": { "issueCount": 12, "nodes": nodes } });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        assert_eq!(b.top_by(|r| r.lines_changed()).len(), TOP_N);
        assert_eq!(
            b.rows.len(),
            12,
            "the board keeps everyone; only the CHART cuts"
        );
    }

    /// Mine and Others partition the board with nobody lost and nobody
    /// double-counted.
    #[test]
    fn mine_and_others_partition_the_board() {
        let map = json!({
            "s0": { "issueCount": 3, "nodes": [
                node("me", 1, 1, 1, 0),
                node("them", 2, 2, 2, 0),
                node("other", 3, 3, 3, 0),
            ]},
        });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        let mine = b.row_for("me").expect("my row");
        let others: Vec<&str> = b.others("me").map(|r| r.login.as_str()).collect();
        assert_eq!(others, ["other", "them"]);
        assert_eq!(
            1 + others.len(),
            b.rows.len(),
            "Mine plus Others must be the whole board"
        );
        assert_eq!(mine.prs, 1);
    }

    /// A deleted author is bucketed, not dropped -- so the rows still sum
    /// to `retrieved` and the board's own arithmetic holds.
    #[test]
    fn a_deleted_author_is_bucketed_not_dropped() {
        let map = json!({
            "s0": { "issueCount": 2, "nodes": [
                node("alice", 1, 1, 1, 0),
                { "author": null, "additions": 5, "deletions": 5,
                  "changedFiles": 1, "reviews": { "totalCount": 0 } },
            ]},
        });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        assert_eq!(b.retrieved, 2);
        let summed: u64 = b.rows.iter().map(|r| r.prs).sum();
        assert_eq!(
            summed, b.retrieved,
            "every retrieved PR must land in exactly one row"
        );
        assert!(b.row_for(GHOST).is_some());
        assert!(is_ghost(GHOST));
        assert!(!is_ghost("alice"));
    }

    /// A board asks about EVERYONE, so its query carries no `author:`.
    ///
    /// `scope.rs` has the same assertion for `StatsQuery` directly; this
    /// one guards the call site, because the bug is not a broken
    /// `StatsQuery` but a caller that passes `Some(subject)` into it and
    /// renders a one-row leaderboard.
    #[test]
    fn a_board_query_names_nobody() {
        let q = StatsQuery::new(
            None,
            Scope::Org("FNX-Labs".into()),
            super::super::scope::Measure::Merged,
        );
        let s = q.search_query("2026-08-01", "2026-08-31");
        assert!(
            !s.contains("author:"),
            "a leaderboard constrained to one author is a board with one name on it: {s}"
        );
        assert!(s.contains("org:FNX-Labs"));
    }

    /// The document this module reads must spell `reviews` the FREE way.
    ///
    /// MEASURED 2026-09-11 (module docs carry the table): `reviews
    /// { totalCount }` costs nothing extra at 3, 6 and 15 searches, while
    /// `reviews(first: 1) { totalCount }` costs 2 points from 6 searches
    /// up. The difference is four characters, and it is the difference
    /// between a free reviewer leaderboard and one that multiplies the
    /// cost of every detail request.
    ///
    /// Asserted against the generated document, not a comment, because a
    /// comment cannot fail.
    #[test]
    fn the_reviews_field_is_the_unpaged_spelling() {
        let q = StatsQuery::new(
            None,
            Scope::Org("FNX-Labs".into()),
            super::super::scope::Measure::Merged,
        );
        let doc = super::super::query::slice_detail_query(&q, &slices(1), 0, 50);
        assert!(
            doc.contains("reviews { totalCount }"),
            "the reviewer board needs a review count: {doc}"
        );
        assert!(
            !doc.contains("reviews("),
            "a PAGED reviews connection is priced per search; the unpaged \
             form measured free. See the table in board.rs's module docs."
        );
    }

    /// This document's chunk is SMALLER than the probe's, and that is a
    /// measurement rather than caution.
    ///
    /// 10 aliases x 50 nodes measured 0 of 3 against real dense
    /// day-slices, every failure at 10.6s against #827's ~11s deadline.
    /// The probe document at the same 10 aliases answered in 1.4-1.5s
    /// because it materialises no nodes, so the two chunk sizes are
    /// genuinely different numbers for genuinely different documents.
    #[test]
    fn the_board_document_is_chunked_smaller_than_the_probe() {
        const {
            assert!(BOARD_ALIAS_CHUNK >= 1);
            assert!(
                BOARD_ALIAS_CHUNK < super::super::query::ALIAS_CHUNK,
                "10 aliases x 50 nodes measured 0 of 3 on dense data; \
                 re-measure before raising this to the probe's chunk"
            );
        }
        // And it stays inside the ceiling the probe document is held to,
        // which is the weaker of the two bounds but the one a reviewer
        // editing a chunk size will look for.
        const {
            assert!(BOARD_ALIAS_CHUNK <= super::super::query::ALIAS_CEILING);
        }
    }

    /// TOP_N is five, because #826 says top five. Pinned so the UI and
    /// the Rust side cannot disagree about what "top five" cuts at.
    #[test]
    fn top_n_is_five() {
        const {
            assert!(TOP_N == 5);
        }
    }

    /// An UNMERGED pull request gets no cycle time, and is kept out of the
    /// outliers and the repo counts.
    ///
    /// This is the figure most easily turned into an accusation: measured
    /// against "now", an open pull request gets slower every second and
    /// would top a "slowest to merge" list while not having failed to merge
    /// at all. It still counts toward `prs` and the line totals, because it
    /// IS work -- so the row's `cycle_time_hours` is deliberately shorter
    /// than its `prs`, which the field's doc comment states because a caller
    /// dividing one by the other would be wrong.
    #[test]
    fn an_open_pull_request_has_no_cycle_time() {
        let map = json!({
            "s0": { "issueCount": 2, "nodes": [
                pr("alice", 1, "owner/repo", "2026-08-01T00:00:00Z",
                   Some("2026-08-02T00:00:00Z"), 10, 0),
                // Still open: `mergedAt` is null.
                pr("alice", 2, "owner/repo", "2026-08-01T00:00:00Z", None, 99_999, 0),
            ]},
        });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        let row = b.row_for("alice").expect("alice");
        assert_eq!(row.prs, 2, "an open PR is still work and still counted");
        assert_eq!(row.additions, 100_009, "its lines still count");
        assert_eq!(
            row.cycle_time_hours,
            vec![24.0],
            "only the merged one has a cycle time"
        );
        // And the open one -- by far the largest -- is absent from the
        // outliers, because "largest merged" and "slowest to merge" must be
        // drawn from the same population as each other.
        assert_eq!(b.largest.len(), 1);
        assert_eq!(b.largest[0].number, 1);
        assert_eq!(b.slowest.len(), 1);
        assert_eq!(
            b.repo_counts,
            vec![RepoCount {
                repo: "owner/repo".into(),
                merged: 1
            }],
            "repo counts are MERGED counts, matching the figure above them"
        );
    }

    /// A negative cycle time is dropped rather than clamped.
    ///
    /// It should be impossible -- a merge cannot precede its own creation --
    /// but the timestamps arrive independently, and a negative value sorts
    /// to the TOP of an ascending distribution and drags a median below
    /// zero. Clamping to 0 would be a measurement claim about a value
    /// nobody can explain.
    #[test]
    fn a_negative_cycle_time_is_dropped_not_clamped() {
        let map = json!({
            "s0": { "issueCount": 1, "nodes": [
                pr("alice", 1, "owner/repo", "2026-08-02T00:00:00Z",
                   Some("2026-08-01T00:00:00Z"), 1, 1),
            ]},
        });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        let row = b.row_for("alice").expect("alice");
        assert_eq!(row.prs, 1);
        assert!(
            row.cycle_time_hours.is_empty(),
            "an inexplicable duration must not become a 0 the UI reports"
        );
    }

    /// Cycle times come back ASCENDING, which is the contract the UI's
    /// `percentile()` indexes against.
    ///
    /// `MergedDetail::cycle_time_hours` documents the same requirement for
    /// the unscoped page. Unsorted, `percentile(x, 0.5)` returns whichever
    /// value happens to sit in the middle of the arrival order -- a number
    /// that is not a median and not anything else either.
    #[test]
    fn cycle_times_are_sorted_ascending() {
        let map = json!({
            "s0": { "issueCount": 3, "nodes": [
                pr("alice", 1, "o/r", "2026-08-01T00:00:00Z", Some("2026-08-01T05:00:00Z"), 1, 0),
                pr("alice", 2, "o/r", "2026-08-01T00:00:00Z", Some("2026-08-01T01:00:00Z"), 1, 0),
                pr("alice", 3, "o/r", "2026-08-01T00:00:00Z", Some("2026-08-01T09:00:00Z"), 1, 0),
            ]},
        });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        assert_eq!(
            b.row_for("alice").unwrap().cycle_time_hours,
            vec![1.0, 5.0, 9.0]
        );
    }

    /// Outliers are the SCOPE's extremes, chosen after the whole window is
    /// mapped rather than per slice, and capped at `OUTLIERS`.
    ///
    /// Picked per slice they would be the last slice's extremes, which on a
    /// sliced org window is an arbitrary month -- and nothing on screen
    /// would say so.
    #[test]
    fn outliers_are_the_whole_windows_extremes() {
        let map = json!({
            // The biggest and the slowest are in the FIRST slice, so a
            // per-slice or last-wins implementation fails this.
            "s0": { "issueCount": 2, "nodes": [
                pr("alice", 10, "o/r", "2026-08-01T00:00:00Z", Some("2026-08-20T00:00:00Z"), 50_000, 0),
                pr("bob", 11, "o/r", "2026-08-01T00:00:00Z", Some("2026-08-01T01:00:00Z"), 1, 0),
            ]},
            "s1": { "issueCount": 1, "nodes": [
                pr("carol", 12, "o/r", "2026-08-02T00:00:00Z", Some("2026-08-02T02:00:00Z"), 9, 0),
            ]},
        });
        let b = Board::from_alias_map(&map, &slices(2), 1, unmeasured());
        assert_eq!(b.largest[0].number, 10, "the largest is in the first slice");
        assert_eq!(b.slowest[0].number, 10, "and so is the slowest");
        assert_eq!(b.largest[0].size, 50_000);
        // Identity carried through, which is the entire point of the
        // outlier lists: a figure that cannot name its pull request is the
        // defect `Outliers` exists to fix.
        assert_eq!(b.largest[0].repo, "o/r");
        assert_eq!(b.largest[0].author, "alice");
        assert!(b.largest[0].url.contains("/pull/10"));
        assert!(b.largest.len() <= OUTLIERS && b.slowest.len() <= OUTLIERS);
    }

    /// Repo counts are ordered most-merged first, with a deterministic tie
    /// break.
    #[test]
    fn repo_counts_rank_by_merged_count() {
        let map = json!({
            "s0": { "issueCount": 4, "nodes": [
                pr("a", 1, "o/zeta", "2026-08-01T00:00:00Z", Some("2026-08-01T01:00:00Z"), 1, 0),
                pr("a", 2, "o/alpha", "2026-08-01T00:00:00Z", Some("2026-08-01T01:00:00Z"), 1, 0),
                pr("a", 3, "o/busy", "2026-08-01T00:00:00Z", Some("2026-08-01T01:00:00Z"), 1, 0),
                pr("a", 4, "o/busy", "2026-08-01T00:00:00Z", Some("2026-08-01T01:00:00Z"), 1, 0),
            ]},
        });
        let b = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        assert_eq!(
            b.repo_counts,
            vec![
                RepoCount {
                    repo: "o/busy".into(),
                    merged: 2
                },
                // Equal counts break on the NAME, so the list does not
                // reshuffle between loads when nothing changed.
                RepoCount {
                    repo: "o/alpha".into(),
                    merged: 1
                },
                RepoCount {
                    repo: "o/zeta".into(),
                    merged: 1
                },
            ]
        );
    }

    /// A board is planned against the PAGE, not against the search cap.
    ///
    /// This is the defect I shipped first and then measured: at the count
    /// path's threshold of `SUBDIVIDE_AT = 800`, a 30-day `org:FNX-Labs`
    /// window of **569** merged pull requests plans as ONE slice -- correct
    /// for a count, since `issueCount` is exact at any size -- and the detail
    /// fetch then retrieves `SLICE_PAGE_FULL = 50` of them. A 9% sample. The
    /// board reported it honestly, and a top-five over 9% of the data is
    /// still not a ranking.
    ///
    /// Asserted on `split_factor_for`, the function that decides it, with the
    /// real measured numbers rather than round ones -- so the test fails if
    /// someone "simplifies" the threshold back to one value.
    #[test]
    fn a_board_subdivides_to_the_page_not_to_the_search_cap() {
        use super::super::fetch::SLICE_PAGE_FULL;
        use super::super::slice::{split_factor_for, SUBDIVIDE_AT};

        // The measured window. Against the COUNT threshold it is not split
        // at all, which is what made the board a sample.
        let measured_window = 569u64;
        assert!(
            measured_window < SUBDIVIDE_AT,
            "the whole point: 569 is UNDER the count path's threshold, so that \
             threshold cannot be what a board uses"
        );

        // Against the PAGE threshold it splits, and by enough to matter.
        let page = u64::from(SLICE_PAGE_FULL);
        let n = split_factor_for(measured_window, page);
        assert!(
            n >= 2,
            "a 569-PR window must be cut against a {page}-node page, got {n}"
        );
        // Capped at ALIAS_CHUNK so one subdivision's probes fit one request,
        // so 569/50 = 12 pieces takes two rounds rather than one. That is
        // cheap -- the 30-alias probe document measured 1 point and 3.5s --
        // and it is why the cap is not raised here.
        assert!(n <= super::super::query::ALIAS_CHUNK as u32);
    }

    /// A board load has ONE wall-clock ceiling, not two in series.
    ///
    /// `load_board` calls `plan_to` and then `load_detail_chunked`, each of
    /// which carries its own `LOAD_TIMEOUT`. Without an outer bound the board
    /// could run twice the ceiling -- 120s against the 60 `load_count` holds
    /// itself to -- which breaks the rule `LOAD_TIMEOUT`'s own doc states.
    /// Found in review.
    ///
    /// Asserted on the source, because the property is structural: it is about
    /// WHERE the timeout sits, and no value a function returns reveals that.
    /// The alternative -- an integration test that waits two minutes for a
    /// hung request -- is a test nobody would run.
    #[test]
    fn a_board_load_is_bounded_once_around_the_whole_thing() {
        let src = include_str!("board.rs");
        let body = src
            .split_once("pub async fn load_board(")
            .expect("load_board")
            .1;
        let head = body
            .split_once("async fn board_inner")
            .expect("the split")
            .0;
        assert!(
            head.contains("tokio::time::timeout"),
            "load_board must bound the whole load, not leave it to the two \
             inner ceilings running in series"
        );
        assert!(
            head.contains("board_inner"),
            "the bounded thing must be the WHOLE load, which is what \
             `board_inner` is for"
        );
        // And the work itself must not also be inline in `load_board`, which
        // would mean the timeout wraps only part of it.
        assert!(
            !head.contains("load_detail_chunked"),
            "the detail fetch belongs inside the bounded inner function"
        );
    }

    /// The threshold is a PARAMETER, and the two callers pass different
    /// values. Guarded because collapsing them back to one constant is the
    /// obvious-looking simplification that reintroduces the sample.
    #[test]
    fn the_count_and_board_thresholds_are_different_numbers() {
        use super::super::fetch::SLICE_PAGE_FULL;
        use super::super::slice::SUBDIVIDE_AT;
        const {
            assert!(
                (SLICE_PAGE_FULL as u64) < SUBDIVIDE_AT,
                "a board's threshold must be the page size, which is far below \
                 the count path's; if these ever meet, a board over a busy \
                 scope silently becomes a sample again"
            );
        }
    }

    /// A zero threshold cuts as far as the grammar allows rather than
    /// dividing by zero.
    ///
    /// A caller asking for slices holding fewer than zero pull requests has a
    /// bug; panicking inside a planner, or silently substituting the count
    /// path's number for a question nobody asked, are both worse than cutting
    /// to the limit and letting `is_one_day` stop the recursion.
    #[test]
    fn a_zero_threshold_does_not_divide_by_zero() {
        let n = super::super::slice::split_factor_for(100, 0);
        assert!(n >= 2);
        assert!(n <= super::super::query::ALIAS_CHUNK as u32);
    }

    /// The detail document carries the identity the outliers need.
    ///
    /// Asserted on the generated document rather than on a comment, because
    /// the outlier lists degrade silently without these: a missing `url`
    /// renders a link to nowhere and a missing `title` an empty row, neither
    /// of which fails a mapper test built from a fixture that has them.
    #[test]
    fn the_detail_document_carries_pull_request_identity() {
        let q = StatsQuery::new(
            None,
            Scope::Org("FNX-Labs".into()),
            super::super::scope::Measure::Merged,
        );
        let doc = super::super::query::slice_detail_query(&q, &slices(1), 0, 50);
        for field in ["number", "title", "url", "repository { nameWithOwner }"] {
            assert!(
                doc.contains(field),
                "the outlier lists cannot name a pull request without {field}: {doc}"
            );
        }
        // MEASURED 2026-09-11: all four leave the document at cost 1. The
        // guard is that they are OBJECTS and scalars rather than paged
        // connections -- `repository` would be priced if it were asked for
        // a page of something.
        assert!(
            !doc.contains("repository("),
            "a paged `repository` would be the priced shape"
        );
    }

    // ---- Accumulation across loads (#1004) ----------------------------

    /// A stored pull request, for the accumulation tests.
    fn stored(repo: &str, number: u64, author: &str, hours: f64, size: u64) -> StoredPr {
        StoredPr {
            repo: repo.into(),
            number,
            merged_at: "2026-08-01".into(),
            title: format!("pr {number}"),
            url: format!("https://github.com/{repo}/pull/{number}"),
            author: author.into(),
            cycle_time_hours: hours,
            size,
            additions: size,
            deletions: 0,
            changed_files: 1,
            reviews_received: 1,
        }
    }

    /// The ledger's verdict on a window, for the `from_stored` tests.
    ///
    /// `whole` means every day measured with nothing outstanding, which is
    /// what a complete board needs; the tests that care about partiality
    /// build their own.
    fn whole_coverage(total: u64, retrieved: u64) -> crate::store::pr_slice::Coverage {
        crate::store::pr_slice::Coverage {
            days: vec!["2026-08-01".to_string()],
            total: Some(total),
            retrieved,
            partial: false,
        }
    }

    /// A fetched board carrying only the facts `from_stored` reads off it.
    fn fetched(total: u64, retrieved: u64, refused: usize) -> Board {
        Board {
            rows: Vec::new(),
            total: Some(total),
            retrieved,
            complete: retrieved == total && refused == 0,
            truncated_slices: Vec::new(),
            refused_fields: refused,
            slices: 1,
            rounds: 1,
            spend: unmeasured(),
            slowest: Vec::new(),
            largest: Vec::new(),
            repo_counts: Vec::new(),
            accumulated: retrieved,
            accumulating: false,
            days_covered: 1,
            days_total: 1,
        }
    }

    /// THE load-bearing property of #1004, at the board layer: a second
    /// partial load's board contains pull requests only the FIRST load
    /// retrieved.
    ///
    /// This is the union, asserted where a user would see it -- in the
    /// rows and the outlier lists, not merely in a row count. Without
    /// accumulation the second board holds only the second load's, which
    /// is the reported defect: "1,419 retrieved then 800 more gives two
    /// partial boards, never 2,219".
    #[test]
    fn a_second_load_assembles_a_board_over_both_loads_pull_requests() {
        // Load one retrieved alice's two; load two retrieved carol's two.
        // The table holds the union.
        let union = vec![
            stored("o/a", 1, "alice", 10.0, 100),
            stored("o/a", 2, "alice", 20.0, 200),
            stored("o/c", 4, "carol", 30.0, 300),
            stored("o/c", 5, "carol", 40.0, 400),
        ];
        // The SECOND load itself fetched only carol's two, of four in window.
        let board = Board::from_stored(&union, &fetched(4, 2, 0), &whole_coverage(4, 4), 1);

        // Both authors are on the board, though this load fetched one of them.
        let logins: Vec<&str> = board.rows.iter().map(|r| r.login.as_str()).collect();
        assert_eq!(logins, vec!["alice", "carol"]);
        assert_eq!(
            board
                .row_for("alice")
                .expect("alice must survive load two")
                .prs,
            2,
            "a pull request only the FIRST load retrieved must still be on the board"
        );
        assert_eq!(board.accumulated, 4);
        assert!(board.accumulating);

        // And specifically in the outlier lists, which are what the user reads.
        assert!(
            board
                .largest
                .iter()
                .any(|p| p.number == 1 && p.repo == "o/a"),
            "an outlier only the first load retrieved must be rankable"
        );
    }

    /// Completeness is computed against the WINDOW, not the fetch.
    ///
    /// A load that fetched 2 of 4 is complete once the TABLE holds 4 --
    /// that is #1004's definition, and it is what makes the caveat go away
    /// for a user who loaded twice.
    #[test]
    fn complete_flips_only_when_the_window_is_genuinely_covered() {
        let three = vec![
            stored("o/a", 1, "alice", 1.0, 10),
            stored("o/a", 2, "alice", 2.0, 20),
            stored("o/a", 3, "bob", 3.0, 30),
        ];
        // Three of four stored: still incomplete, and visibly so.
        let partial = Board::from_stored(&three, &fetched(4, 3, 0), &whole_coverage(4, 3), 1);
        assert!(!partial.complete, "3 of 4 stored is not complete");
        assert_eq!(partial.accumulated, 3);

        // The fourth arrives -- from a load that fetched only IT.
        let mut four = three.clone();
        four.push(stored("o/b", 9, "carol", 4.0, 40));
        let whole = Board::from_stored(&four, &fetched(4, 1, 0), &whole_coverage(4, 4), 1);
        assert!(
            whole.complete,
            "every PR in the window is stored, so the answer is complete even \
             though this load fetched one of them"
        );
        assert_eq!(whole.accumulated, 4);
    }

    /// Accumulation makes partiality SHRINK; it never makes it invisible
    /// while incomplete (#824, and #1004's explicit non-goal).
    ///
    /// A refusal is not cured by rows arriving from another load: the data
    /// GitHub declined to give is still missing, so `complete` stays false
    /// even at full row coverage.
    #[test]
    fn a_refusal_keeps_the_board_incomplete_however_many_rows_accumulate() {
        let all = vec![
            stored("o/a", 1, "alice", 1.0, 10),
            stored("o/a", 2, "bob", 2.0, 20),
        ];
        let board = Board::from_stored(&all, &fetched(2, 2, 3), &whole_coverage(2, 2), 1);
        assert_eq!(board.accumulated, 2);
        assert_eq!(board.total, Some(2));
        assert!(
            !board.complete,
            "row coverage must not launder a refusal into a confident board"
        );
        assert_eq!(
            board.refused_fields, 3,
            "the refusal channel is carried through"
        );
    }

    /// The truncation channel survives accumulation too, for the same
    /// reason: it names WHICH range was short, and a later load that has
    /// not re-covered it has not fixed it.
    #[test]
    fn truncated_slices_are_carried_through_accumulation() {
        let mut f = fetched(10, 4, 0);
        f.truncated_slices = vec![ShortSlice {
            from: "2026-08-01".into(),
            to: "2026-08-01".into(),
            issue_count: 6,
            retrieved: 0,
        }];
        let board = Board::from_stored(
            &[stored("o/a", 1, "alice", 1.0, 10)],
            &f,
            &whole_coverage(10, 4),
            1,
        );
        assert_eq!(board.truncated_slices.len(), 1);
        assert!(!board.complete);
    }

    /// `retrieved` keeps meaning "what THIS load fetched".
    ///
    /// Overwriting it with the accumulated figure would erase the very
    /// distinction the caveat needs to tell converging from stuck.
    #[test]
    fn retrieved_still_describes_this_load_not_the_accumulation() {
        let five: Vec<StoredPr> = (1..=5)
            .map(|n| stored("o/a", n, "alice", n as f64, n * 10))
            .collect();
        let board = Board::from_stored(&five, &fetched(10, 2, 0), &whole_coverage(10, 5), 1);
        assert_eq!(board.retrieved, 2, "this load fetched two");
        assert_eq!(board.accumulated, 5, "five are stored");
        assert_eq!(board.total, Some(10), "GitHub says the window holds ten");
    }

    /// A board built from one fetch reports no accumulation, so the UI
    /// cannot promise convergence that is not happening.
    ///
    /// The #841 fail-open guard: a claim we cannot keep, presented as a
    /// fact, is the failure mode this flag exists to prevent.
    #[test]
    fn a_single_fetch_board_does_not_claim_to_be_accumulating() {
        let map = json!({ "s0": { "issueCount": 1, "nodes": [node("alice", 10, 5, 2, 1)] } });
        let board = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        assert!(!board.accumulating);
        assert_eq!(
            board.accumulated, board.retrieved,
            "with nothing stored, the accumulated figure is simply what was fetched"
        );
    }

    /// `retrieved_prs` harvests the whole retrieved set, not the truncated
    /// outlier lists.
    ///
    /// Writing down `slowest` and `largest` would store five pull requests
    /// per load and converge on nothing -- the bug that would make this
    /// feature silently useless while every other test still passed.
    #[test]
    fn the_harvest_is_the_whole_page_not_the_five_outliers() {
        let nodes: Vec<serde_json::Value> = (1..=12)
            .map(|n| {
                pr(
                    "alice",
                    n,
                    "o/a",
                    "2026-08-01T00:00:00Z",
                    Some("2026-08-01T05:00:00Z"),
                    n * 10,
                    0,
                )
            })
            .collect();
        let map = json!({ "s0": { "issueCount": 12, "nodes": nodes } });
        let harvested = Board::retrieved_prs(&map, &slices(1));
        assert_eq!(
            harvested.len(),
            12,
            "all twelve must be written down, not the {OUTLIERS} outliers"
        );
        assert!(harvested.len() > OUTLIERS);
    }

    /// An UNMERGED pull request is not accumulated.
    ///
    /// The stored population must match the one the outlier lists and
    /// `repo_counts` are drawn from, or a board assembled from storage
    /// would rank a different population than one assembled from a fetch.
    #[test]
    fn an_unmerged_pull_request_is_not_accumulated() {
        let map = json!({
            "s0": {
                "issueCount": 2,
                "nodes": [
                    pr("alice", 1, "o/a", "2026-08-01T00:00:00Z", Some("2026-08-01T05:00:00Z"), 10, 0),
                    pr("bob", 2, "o/a", "2026-08-01T00:00:00Z", None, 20, 0),
                ]
            }
        });
        let harvested = Board::retrieved_prs(&map, &slices(1));
        assert_eq!(harvested.len(), 1);
        assert_eq!(harvested[0].author, "alice");
    }

    /// A node with no repository is skipped rather than stored under an
    /// empty key, where it would collide with every other such node on
    /// `(repo, number)` and silently overwrite them.
    #[test]
    fn a_node_without_a_repository_is_not_accumulated() {
        let map = json!({
            "s0": {
                "issueCount": 1,
                "nodes": [{
                    "number": 1,
                    "title": "t",
                    "url": "u",
                    "repository": { "nameWithOwner": "" },
                    "author": { "login": "alice" },
                    "createdAt": "2026-08-01T00:00:00Z",
                    "mergedAt": "2026-08-01T01:00:00Z",
                    "additions": 1, "deletions": 0, "changedFiles": 1,
                    "reviews": { "totalCount": 0 }
                }]
            }
        });
        assert!(Board::retrieved_prs(&map, &slices(1)).is_empty());
    }

    /// Assembling from storage produces the SAME author aggregates a
    /// single complete fetch would.
    ///
    /// The regression guard for a small account: accumulation must not
    /// change the numbers where nothing was ever partial.
    #[test]
    fn a_complete_load_assembles_identically_from_storage() {
        let map = json!({
            "s0": {
                "issueCount": 2,
                "nodes": [
                    pr("alice", 1, "o/a", "2026-08-01T00:00:00Z", Some("2026-08-01T02:00:00Z"), 30, 10),
                    pr("alice", 2, "o/a", "2026-08-01T00:00:00Z", Some("2026-08-01T04:00:00Z"), 5, 5),
                ]
            }
        });
        let direct = Board::from_alias_map(&map, &slices(1), 1, unmeasured());
        let harvested = Board::retrieved_prs(&map, &slices(1));
        let assembled = Board::from_stored(&harvested, &direct, &whole_coverage(2, 2), 1);

        assert!(direct.complete && assembled.complete);
        let a = direct.row_for("alice").unwrap();
        let b = assembled.row_for("alice").unwrap();
        assert_eq!(a.prs, b.prs);
        assert_eq!(a.additions, b.additions);
        assert_eq!(a.deletions, b.deletions);
        assert_eq!(a.changed_files, b.changed_files);
        assert_eq!(a.reviews_received, b.reviews_received);
        assert_eq!(a.cycle_time_hours, b.cycle_time_hours);
        assert_eq!(direct.repo_counts, assembled.repo_counts);
        assert_eq!(direct.largest, assembled.largest);
        assert_eq!(direct.slowest, assembled.slowest);
    }

    /// A board whose every alias DID answer is still incomplete if the
    /// ceiling expired (#1044).
    ///
    /// The case the unconditional `complete = false` guards, and the only
    /// one that isolates it. Everywhere else the missing aliases do the
    /// work: a wave that never ran leaves its slices absent, and
    /// `from_alias_map` already reports an absent alias as a short slice,
    /// which clears the flag on its own. Here every slice answered and the
    /// load ran out of wall clock anyway -- the mapper sees a whole board
    /// and would call it complete.
    ///
    /// Reachable: the ceiling is on the WHOLE of `board_inner`, so it can
    /// expire after the last wave merged and before the future returned.
    /// A board presented as complete after a load that timed out is a
    /// confident claim made by a load that did not finish, which is the one
    /// thing #1044's rule forbids.
    #[test]
    fn a_timed_out_board_is_incomplete_even_when_every_alias_answered() {
        // `Budget::new()` reads the process-wide OBSERVED_REMAINING, which
        // `invariants.rs` requires every synchronous test touching it to
        // serialise against -- #868's race.
        let _g = super::super::budget::observed_test_lock();
        let _restore = super::super::budget::RestoreObserved::capture();
        let planned = slices(2);
        let progress = BoardProgress::default();
        progress.publish(PlannedSlices {
            slices: planned.clone(),
            rounds: 1,
        });

        let sink = super::super::fetch::PartialDetail::new();
        let whole = json!({
            "s0": { "issueCount": 1, "nodes": [node("alice", 10, 5, 2, 1)] },
            "s1": { "issueCount": 1, "nodes": [node("bob", 1, 1, 1, 0)] },
        });
        sink.record(whole.as_object().unwrap(), 0, planned.len());

        let loaded = partial_on_timeout(
            &progress,
            &sink,
            &Budget::new(),
            std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(60),
        )
        .expect("every alias answered, so there is a board to render");

        assert_eq!(loaded.prs.len(), 2, "both pull requests are kept");
        assert!(
            loaded.board.truncated_slices.is_empty(),
            "nothing was short -- which is exactly why the flag needs forcing"
        );
        assert!(
            !loaded.board.complete,
            "a load that ran out of wall clock must not present itself as \
             complete, however whole the aliases it did reach look"
        );
    }

    /// The partial board names the ranges it never read.
    ///
    /// Asserted on the SHAPE rather than only on the flag, because
    /// "incomplete" is a boolean a user cannot act on: #826's rule is that
    /// a short board says WHICH part is short, so a reader can ask GitHub
    /// the same question for that range. This is the timeout path
    /// inheriting that property from `from_alias_map` rather than
    /// re-deriving it.
    #[test]
    fn a_timed_out_board_names_the_slices_it_never_reached() {
        // `Budget::new()` reads the process-wide OBSERVED_REMAINING, which
        // `invariants.rs` requires every synchronous test touching it to
        // serialise against -- #868's race.
        let _g = super::super::budget::observed_test_lock();
        let _restore = super::super::budget::RestoreObserved::capture();
        let planned = slices(3);
        let progress = BoardProgress::default();
        progress.publish(PlannedSlices {
            slices: planned.clone(),
            rounds: 1,
        });

        let sink = super::super::fetch::PartialDetail::new();
        // Only the FIRST slice answered.
        let first = json!({ "s0": { "issueCount": 1, "nodes": [node("alice", 10, 5, 2, 1)] } });
        sink.record(first.as_object().unwrap(), 0, 1);

        let loaded = partial_on_timeout(
            &progress,
            &sink,
            &Budget::new(),
            std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(60),
        )
        .expect("one slice answered, so there is something to render");

        assert_eq!(loaded.prs.len(), 1);
        assert!(!loaded.board.complete);
        let named: Vec<&str> = loaded
            .board
            .truncated_slices
            .iter()
            .map(|s| s.from.as_str())
            .collect();
        assert_eq!(
            named,
            vec![planned[1].from.as_str(), planned[2].from.as_str()],
            "the two slices the ceiling swallowed must be named by their own \
             date ranges, not merely counted"
        );
        // Unknown size, not zero. The only figure that could have said how
        // big those slices were is the `issueCount` that never arrived.
        assert!(loaded
            .board
            .truncated_slices
            .iter()
            .all(|s| s.issue_count == 0 && s.retrieved == 0));
    }

    /// A ceiling that expires while PLANNING is an error, not an empty board.
    ///
    /// The plan never published, so there are no slices to index aliases
    /// against and nothing was fetched. An empty board here would say "we
    /// measured this scope and found nobody", which is the zero-for-absent
    /// confusion this module's header forbids -- so the timeout survives.
    #[test]
    fn a_ceiling_that_expires_while_planning_is_still_an_error() {
        let _g = super::super::budget::observed_test_lock();
        let _restore = super::super::budget::RestoreObserved::capture();
        let e = partial_on_timeout(
            &BoardProgress::default(),
            &super::super::fetch::PartialDetail::new(),
            &Budget::new(),
            std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(60),
        )
        .expect_err("nothing was planned and nothing was fetched");
        assert!(matches!(e, ClientError::Timeout(60)));
    }

    /// A board that runs out of wall clock KEEPS the pull requests it
    /// already retrieved, and says it is incomplete (#1044).
    ///
    /// # Why this test is end-to-end rather than a unit test on the mapper
    ///
    /// The bug was not in any mapper. Every piece of #1004's accumulation
    /// was correct; `tokio::time::timeout` DROPPED the future holding the
    /// result, and no unit test can see a drop. The existing tests covered
    /// the success path and the error TYPE, so nothing asserted that a
    /// timed-out load leaves the store better off than before -- which is
    /// precisely the gap that let this ship.
    ///
    /// So this drives the shipped `load_board_within` against a server
    /// that answers the plan and the FIRST detail wave and then stalls,
    /// with a ceiling short enough to expire in the stall. It asserts the
    /// first wave's pull requests come back rather than an error.
    ///
    /// # How the plan is forced into two waves
    ///
    /// A wave is `BOARD_ALIAS_CHUNK * READ_CONCURRENCY` = 30 slices, so
    /// the window has to plan into more than 30. Three probe rounds do it:
    /// a 40-day window over the subdivision threshold cuts into 10 pieces
    /// of 4 days, each still over cuts into 4 single days, and 40 single
    /// days are irreducible. 40 slices is two waves, with the second one
    /// the stall.
    #[tokio::test]
    async fn a_timed_out_board_keeps_the_pull_requests_it_retrieved() {
        use wiremock::matchers::{body_string_contains, method};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // The read semaphore is process-wide and these tests stall
        // requests on purpose -- see `budget::READ_PERMIT_TEST_LOCK`.
        let _permits = crate::github::stats::budget::READ_PERMIT_TEST_LOCK
            .lock()
            .await;
        let server = MockServer::start().await;

        // How many first-wave detail RESPONSES the mock has served (#1073).
        //
        // # Why this exists: the ceiling used to race a real clock
        //
        // The pairing was a 1500ms ceiling against a 5s stall, and the
        // test asserted the whole first wave had landed inside that
        // 1500ms. On a loaded runner it had not -- the observed panic was
        // `!loaded.prs.is_empty()`, meaning not one first-wave response
        // had arrived. It fired twice on two different commits, once on
        // `main` itself. A failed attempt is PERMANENT to the release gate
        // (`wait-for-duplicates: true`), so a flake here can burn a tag,
        // which is how v5.20.0 was lost (#1048).
        //
        // The property under test was never about speed: it is "what
        // arrived is kept, and what did not is named". So the margins are
        // now two orders of magnitude apart in both directions, and this
        // counter lets the test CHECK that its own premise held rather
        // than assume it.
        let first_wave_done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let served = first_wave_done.clone();
        // Two orders of magnitude above what an undelayed first wave
        // needs, and a tenth of the second wave's stall.
        const CEILING: std::time::Duration = std::time::Duration::from_secs(3);

        // The probe. `issueCount` without `nodes`, so it is matched by the
        // ABSENCE of the node selection -- and answered generously enough
        // that every slice subdivides until it is a single day.
        //
        // One body for every probe round: `probe_body` fills in whatever
        // aliases the round asked for, so the same mock serves rounds of
        // 1, 10 and 40 slices without three separate mocks that could
        // disagree about the counts.
        Mock::given(method("POST"))
            .and(body_string_contains("issueCount"))
            .respond_with(move |req: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                let doc = body["query"].as_str().unwrap_or("");
                if doc.contains("nodes {") {
                    // A DETAIL document. The aliases it asks for say which
                    // wave this is: the first wave's are s0..s29, so any
                    // request mentioning an alias at or past s30 is the
                    // second wave and is the one that stalls.
                    let second_wave =
                        (BOARD_ALIAS_CHUNK * super::super::fetch::READ_CONCURRENCY..40).any(|i| {
                            doc.contains(&format!(
                                "  {}: search",
                                super::super::query::slice_alias(i)
                            ))
                        });
                    if second_wave {
                        // Long enough to outlast the ceiling by two orders
                        // of magnitude, and BOUNDED rather than infinite:
                        // `fetch::READ_PERMITS` is process-wide, so a
                        // request that never returns would hold a permit
                        // against every other test in the binary.
                        // Ten times the ceiling (#1073). Bounded rather
                        // than infinite: `fetch::READ_PERMITS` is
                        // process-wide, so a request that never returns
                        // would hold a permit against every other test.
                        return ResponseTemplate::new(200)
                            .set_body_json(detail_body(&[]))
                            .set_delay(std::time::Duration::from_secs(30));
                    }
                    let aliases = aliases_in(doc);
                    // Counted AFTER the body is built, so it rises only
                    // for a response this mock really is returning (#1073).
                    let r = ResponseTemplate::new(200).set_body_json(detail_body(&aliases));
                    served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    return r;
                }
                let aliases = aliases_in(doc);
                ResponseTemplate::new(200).set_body_json(probe_body(&aliases))
            })
            .mount(&server)
            .await;

        let client = mock_client(&server).await;
        // The observed figure, scoped to THIS TEST's thread (#1079).
        //
        // `Budget::permits` takes the LOWER of this load's own figure and
        // the process-wide one, so a test that inherits a starved global
        // is refused by the PLANNER before any wave runs -- and fails with
        // a message about a budget it never set. That is how the v5.22.0
        // tag was lost.
        //
        // `scoped::enter` rather than `observed_test_lock`: the lock is a
        // `std::sync::MutexGuard` and clippy's `await_holding_lock` under
        // `-D warnings` refuses to let one be held across an `.await`.
        // This guard is a thread-local write, safe to hold here, and it
        // restores on drop including on panic.
        let _observed = crate::github::stats::budget::scoped::enter(50_000);
        let budget = Budget::new();
        let scope = Scope::Org("acme".into());

        // Two different units, and conflating them is a mistake made while
        // fixing this (#1073): the first wave is `READ_CONCURRENCY`
        // REQUESTS of `BOARD_ALIAS_CHUNK` aliases each -- 6 responses
        // carrying 30 slices, one merged pull request per slice.
        const FIRST_WAVE_REQUESTS: usize = super::super::fetch::READ_CONCURRENCY;
        const FIRST_WAVE_SLICES: usize = BOARD_ALIAS_CHUNK * FIRST_WAVE_REQUESTS;

        let loaded = super::load_board_within(
            &client,
            &scope,
            super::super::scope::Measure::Merged,
            Slice::new("2026-07-01".to_string(), "2026-08-09".to_string()),
            &budget,
            CEILING,
        )
        .await
        .expect("a timed-out load must return what it retrieved, not an error");

        // The GUARD on this test's own premise. If a runner is ever slow
        // enough to reorder a 3-second ceiling against an undelayed first
        // wave, this fails FIRST and says so -- rather than a retention
        // assertion failing and looking like a defect in the code under
        // test. That misattribution is what made the old flake expensive.
        assert_eq!(
            first_wave_done.load(std::sync::atomic::Ordering::SeqCst),
            FIRST_WAVE_REQUESTS,
            "the first wave must be served in full before the ceiling lands, \
             or this test measures the runner rather than the retention it \
             exists to prove"
        );

        // THE ASSERTION #1044 IS ABOUT. The first wave answered 30 slices
        // with one merged pull request each; every one of them was paid
        // for and every one of them must survive the drop.
        assert!(
            !loaded.prs.is_empty(),
            "a load that retrieved pull requests before the ceiling expired \
             must hand them to the caller for `pr_history::put_many`; \
             got none"
        );
        // EXACT, not a threshold (#1073). `>= 20` was a proxy for "most
        // of the first wave arrived", and it was the assertion that raced.
        // With the premise guarded above, the number that survives is not
        // a matter of timing -- it is the whole wave, and anything less is
        // a real retention bug.
        assert_eq!(
            loaded.prs.len(),
            FIRST_WAVE_SLICES,
            "every first-wave pull request was paid for and must survive the \
             drop: got {} of {FIRST_WAVE_SLICES}",
            loaded.prs.len()
        );
        // Qualified, never confident. The slices the second wave never
        // reached are absent, so the ranking between authors could be
        // wrong even though each author's own figures are a floor.
        assert!(
            !loaded.board.complete,
            "a board assembled from part of its slices must say so"
        );
        // And it names WHICH ranges it is missing, through the same
        // `from_alias_map` channel a refused wave uses -- so the partial
        // path needs no second mapper.
        assert!(
            !loaded.board.truncated_slices.is_empty(),
            "the slices the stall swallowed must be named, not merely counted away"
        );
        assert!(
            loaded.board.retrieved > 0,
            "the rows must be built from the kept nodes"
        );
    }

    /// The same load, given time to finish, returns everything.
    ///
    /// The control for the test above: without it, a `load_board_within`
    /// that had simply stopped fetching after one wave would pass the
    /// retention assertion while being badly broken.
    #[tokio::test]
    async fn the_same_load_is_complete_when_it_is_given_time() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // The read semaphore is process-wide and these tests stall
        // requests on purpose -- see `budget::READ_PERMIT_TEST_LOCK`.
        let _permits = crate::github::stats::budget::READ_PERMIT_TEST_LOCK
            .lock()
            .await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(move |req: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                let doc = body["query"].as_str().unwrap_or("");
                let aliases = aliases_in(doc);
                if doc.contains("nodes {") {
                    return ResponseTemplate::new(200).set_body_json(detail_body(&aliases));
                }
                ResponseTemplate::new(200).set_body_json(probe_body(&aliases))
            })
            .mount(&server)
            .await;

        let client = mock_client(&server).await;
        // The observed figure, scoped to THIS TEST's thread (#1079).
        //
        // `Budget::permits` takes the LOWER of this load's own figure and
        // the process-wide one, so a test that inherits a starved global
        // is refused by the PLANNER before any wave runs -- and fails with
        // a message about a budget it never set. That is how the v5.22.0
        // tag was lost.
        //
        // `scoped::enter` rather than `observed_test_lock`: the lock is a
        // `std::sync::MutexGuard` and clippy's `await_holding_lock` under
        // `-D warnings` refuses to let one be held across an `.await`.
        // This guard is a thread-local write, safe to hold here, and it
        // restores on drop including on panic.
        let _observed = crate::github::stats::budget::scoped::enter(50_000);
        let budget = Budget::new();
        let loaded = super::load_board_within(
            &client,
            &Scope::Org("acme".into()),
            super::super::scope::Measure::Merged,
            Slice::new("2026-07-01".to_string(), "2026-08-09".to_string()),
            &budget,
            std::time::Duration::from_secs(30),
        )
        .await
        .expect("an unstalled load must succeed");

        assert_eq!(
            loaded.prs.len(),
            40,
            "every slice answered, so every slice's pull request is kept"
        );
        assert!(
            loaded.board.truncated_slices.is_empty(),
            "nothing was short: {:?}",
            loaded.board.truncated_slices
        );
    }

    /// A ceiling that expires before ANY slice was covered is still an error.
    ///
    /// The other half of the rule. "Partial is not nothing" is not "nothing
    /// is partial": a board with no retrieved pull requests would render as
    /// "we measured nobody", which is the zero-for-absent confusion this
    /// module's header forbids. So the timeout error survives for exactly
    /// the case where there is nothing to qualify.
    #[tokio::test]
    async fn a_board_that_retrieved_nothing_is_still_an_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // The read semaphore is process-wide and these tests stall
        // requests on purpose -- see `budget::READ_PERMIT_TEST_LOCK`.
        let _permits = crate::github::stats::budget::READ_PERMIT_TEST_LOCK
            .lock()
            .await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(move |req: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                let doc = body["query"].as_str().unwrap_or("");
                let aliases = aliases_in(doc);
                if doc.contains("nodes {") {
                    // EVERY detail request stalls, so the plan completes
                    // and not one wave does.
                    return ResponseTemplate::new(200)
                        .set_body_json(detail_body(&[]))
                        .set_delay(std::time::Duration::from_secs(5));
                }
                ResponseTemplate::new(200).set_body_json(probe_body(&aliases))
            })
            .mount(&server)
            .await;

        let client = mock_client(&server).await;
        // The observed figure, scoped to THIS TEST's thread (#1079).
        //
        // `Budget::permits` takes the LOWER of this load's own figure and
        // the process-wide one, so a test that inherits a starved global
        // is refused by the PLANNER before any wave runs -- and fails with
        // a message about a budget it never set. That is how the v5.22.0
        // tag was lost.
        //
        // `scoped::enter` rather than `observed_test_lock`: the lock is a
        // `std::sync::MutexGuard` and clippy's `await_holding_lock` under
        // `-D warnings` refuses to let one be held across an `.await`.
        // This guard is a thread-local write, safe to hold here, and it
        // restores on drop including on panic.
        let _observed = crate::github::stats::budget::scoped::enter(50_000);
        let budget = Budget::new();
        let e = super::load_board_within(
            &client,
            &Scope::Org("acme".into()),
            super::super::scope::Measure::Merged,
            Slice::new("2026-07-01".to_string(), "2026-08-09".to_string()),
            &budget,
            std::time::Duration::from_millis(1500),
        )
        .await
        .expect_err("a load that retrieved nothing has nothing to render");
        assert!(
            matches!(e, ClientError::Timeout(_)),
            "and it is still the timeout, not some other error: {e:?}"
        );
    }
}
