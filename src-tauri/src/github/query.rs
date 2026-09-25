//! GraphQL query documents. Every document HERE is read-only -- a
//! `search` or a `repository` lookup, never a mutation.
//!
//! Writes exist, and live in `mutate.rs`. Keeping them in a separate
//! module is the point: this file stays auditable as pure reads, and the
//! write surface is small enough to read in one sitting.

use chrono::{DateTime, Duration, Utc};

/// One query returns every open PR with everything the UI needs: CI rollup,
/// mergeability, review decision, merge-queue membership, and labels.
/// Measured at 27 PRs in ~2.9s for 2 rate-limit points of 5000/hour.
/// Two aliased searches: PRs the user authored, and PRs awaiting their
/// review. Aliased searches cost ONE point in total regardless of count,
/// so the second list is free -- verified against the live API.
///
/// `search` is aliased to `authored` rather than left bare, so the mapper
/// has to name which list it is reading and cannot silently take the wrong
/// one when a third is added.
/// One search's worth of pull requests.
///
/// ONE search per request. It used to carry both the authored and
/// review-requested searches as aliases, so every caller paid for both:
/// on a reported account with 40 authored and 71 review-requested, that
/// is 111 fully populated pull requests fetched whenever either list was
/// wanted. My pull requests recovered on that account once the page
/// shrank; To review did not, because 71 is nearly twice 40.
///
/// The alias stays named `authored` whatever the search is -- the mapper
/// reads it by that name, and renaming it per caller would mean the
/// query and the mapper could disagree.
pub const PRS_QUERY: &str = r#"
query($q: String!, $first: Int!, $after: String) {
  rateLimit { cost remaining resetAt }
  viewer { login }
  authored: search(query: $q, type: ISSUE, first: $first, after: $after) {
    issueCount
    nodes {
      ... on PullRequest {
        id number title url isDraft createdAt updatedAt
        # When it became ready for review (#1407), which is `createdAt`
        # only for a pull request that was never a draft. Filtered to the
        # one event type and paged `last: 1`, because `map.rs`'s
        # `ready_at` reads the LAST node as the ready time.
        #
        # No `totalCount`: the page is the latest event by construction,
        # so there is no truncation to report.
        #
        # MEASURED free, 2026-09-24, this document extracted with its `#`
        # comment lines stripped, `gh api graphql -F first=25`, before
        # and after adding this line, three runs each: cost 2 before and
        # 2 after on `author:@me`, `review-requested:@me`,
        # `repo:kubernetes/kubernetes` and `repo:vercel/next.js`, and 2
        # and 2 on those two repositories plus rust-lang/rust and
        # microsoft/vscode in one search. Wall clock moved within the
        # run-to-run spread (6.8-8.8s after against 7.4-8.8s before on
        # kubernetes/kubernetes). `MEASURED_COST` in `poll.rs` stands.
        timelineItems(itemTypes: [READY_FOR_REVIEW_EVENT], last: 1) {
          nodes { ... on ReadyForReviewEvent { createdAt } }
        }
        headRefName headRefOid baseRefName
        headRef { id }
        author { login }
        repository { nameWithOwner }
        # `mergeStateStatus` is the single most expensive field here, and
        # it STAYS. #744 investigated removing it; the measurements said
        # not to, and they are recorded here so the next reader does not
        # repeat the experiment.
        #
        # It is genuinely slow: GitHub computes it per pull request
        # synchronously. MEASURED live against this query's own shape,
        # `first: 25`, marginal cost over an `id`-only search:
        #
        #   5 items  ~ +0.26s     10 items ~ +0.9s     25 items ~ +3.1s
        #
        # -- roughly 50-125ms per item, matching the ~154ms recorded on
        # `client.rs` when PAGE_SIZE was set.
        #
        # WHY DEFERRING IT DOES NOT WORK. The cost is per ITEM, not per
        # request, so a second query that fetches only `id` and
        # `mergeStateStatus` for the same 25 pull requests still measured
        # ~4.8-7.4s against a ~2.2-3.5s baseline. Deferral relocates the
        # spend, it does not remove it, and it adds a rate-limit point
        # and a second list to keep in step with the paging race.
        #
        # Splitting the query in two and issuing both concurrently DOES
        # cut wall time (median 8.6s -> 6.6s, 6 of 6 runs), but the
        # deferred half sets a ~6.4s floor of its own, so time-to-paint
        # only improves from a median ~9.7s to ~8.4s -- about 1.3s, which
        # is inside the run-to-run variance.
        #
        # WHY IT IS NOT WORTH THAT 1.3s. `unknown` is ALREADY a meaningful
        # value on this field: GitHub returns it while recomputing after
        # an approval, and `hooks.ts` polls every 3s precisely on that
        # state (#699). A deferred fetch would make "not requested yet"
        # indistinguishable from "GitHub is recomputing", turning that
        # poller into a permanent spin and reintroducing the exact staleness
        # bug #699 fixed. The consumers all fail safe on `unknown` -- merge
        # is refused, chips and actions are withheld -- so nothing would be
        # UNSAFE, but every row would flicker its merge chip in after paint
        # on every poll, and the merge button would be withheld for a
        # second on pull requests that can in fact merge.
        #
        # The latency answer for #744 was the FETCH CEILING instead: see
        # `poll::FETCH_TIMEOUT`.
        mergeable mergeStateStatus reviewDecision isInMergeQueue totalCommentsCount
        # `isInMergeQueue` stays TRUE for an entry the queue has
        # rejected, so a pull request that was declined rendered as
        # calmly queued -- the amber "In merge queue" icon on something
        # that is actually stuck. The entry's own state is what
        # distinguishes them.
        mergeQueueEntry { state }
        # WHO is blocking this pull request, not just that something is.
        #
        # MEASURED against live repos: costs no extra rate-limit point,
        # and requested reviewers run median 2, max 4 across 30
        # kubernetes/kubernetes pull requests -- so 5 is ample.
        #
        # `requestedReviewer` is a UNION: User, Team and Mannequin all
        # satisfy it. Only User is selected here; the mapper drops the
        # rest rather than inventing a name for them.
        #
        # Empty is an ordinary state, not an error: repositories that
        # assign reviewers through a bot (rust-lang/rust, for one)
        # return nothing here at all.
        # The FALLBACK for repositories that do not use GitHub's review
        # request mechanism.
        #
        # MEASURED across three public repos: `reviewRequests` is empty
        # on 25 of 25 rust-lang/rust pull requests and 18 of 25 on
        # vercel/next.js, against 1 of 25 on kubernetes/kubernetes.
        # rust-lang assigns the reviewer as an ASSIGNEE instead, and
        # reading it rescues 19 of those 25 -- so without this the
        # feature shows nothing at all on whole repositories.
        #
        # Costs nothing: the live query still measures 3 points.
        #
        # `totalCount` on all four connections below, and the raise from
        # 5 to 20 on `latestReviews`, are FREE. MEASURED live 2026-09-16,
        # the same method `poll.rs`'s cost guard records -- this document
        # extracted verbatim with its `#` comment lines stripped, `gh api
        # graphql -F first=25` (which is `client.rs`'s real PAGE_SIZE),
        # three runs each:
        #
        # | Document                          | Cost | Wall clock |
        # |-----------------------------------|------|------------|
        # | before, `author:@me`              | 2    | 2.23-3.08s |
        # | after, `author:@me`               | 2    | 2.59-3.18s |
        # | before, `repo:kubernetes/kubernetes` | 2 | 5.73-5.82s |
        # | after, `repo:kubernetes/kubernetes`  | 2 | 5.88-6.40s |
        #
        # Cost is unchanged at 2, so `MEASURED_COST` in `poll.rs` stands.
        # That matches what `stats/query.rs:694-720` measured: GitHub
        # prices the `first:` ARGUMENT, and all four of these were paged
        # already -- `totalCount` rides along on a connection that is
        # being paid for either way, and the page SIZE is not the price.
        #
        # The truncation is not theoretical. On the same live run,
        # `labels.totalCount` reached **28** against this `first: 20`
        # window on open kubernetes/kubernetes pull requests -- a label
        # the user filters on could simply be absent from a row that
        # looked complete.
        assignees(first: 5) { totalCount nodes { login } }
        reviewRequests(first: 5) {
          totalCount
          nodes { requestedReviewer { ... on User { login } } }
        }
        # Who has already reviewed, and what they said. A different
        # question from `reviewDecision`, which collapses everyone into
        # one verdict and names nobody.
        #
        # 20, NOT 5, to match the detail query (`:529`). `map.rs`'s
        # `latest_reviews` is shared "so the row and the view it opens
        # cannot disagree about who reviewed" -- but the MAPPER being
        # shared does nothing if the page size is not, and at 5 the row
        # and its detail view could name different reviewers for the
        # same pull request.
        #
        # The cap here is worse than an undercount: `pendingReviewers`
        # (`derive.ts`) builds its answered set from this list, so a
        # reviewer who fell outside the window never enters it and is
        # reported as still pending when they have already APPROVED.
        # That names a person and tells you to chase someone who is done.
        latestReviews(first: 20) { totalCount nodes { state author { login } } }
        labels(first: 20) { totalCount nodes { name color } }
        # 100, the connection maximum, so the badge is an exact count
        # rather than a cap (#810). It was `first: 20`, and the mapper
        # counts the unresolved, un-outdated threads in the window -- so a
        # pull request with more than 20 silently understated, while the
        # code read like an exact number.
        #
        # MEASURED on live data rather than inherited from #808, because
        # the cost question here is genuinely different: this connection is
        # nested inside this document's own 100-pull-request page and
        # fans out across every row in the result set, where the detail
        # query fetches ONE.
        #
        # Worded to avoid the search keyword followed by a paren: the
        # cost guard in `poll.rs` counts that as a raw substring, so
        # writing it here -- even inside a GraphQL comment, which the
        # server never sees -- reads as a second alias and trips the
        # one-per-document assertion. Exactly the comment-versus-code
        # confusion #869 is about, except this one fails safe: a false
        # positive that blocks CI, not a false negative that ships. Measured anyway free -- the whole document still costs 1
        # point against rust-lang/rust, kubernetes/kubernetes,
        # vercel/next.js and microsoft/vscode at 100 pull requests each,
        # with no latency difference (~0.75s either way over three runs)
        # and no node-limit error at 100x100.
        #
        # The undercount is real and not theoretical. vercel/next.js
        # #98469 carries 33 threads: the old window reported 2 unresolved
        # where the truth is 9. Most repositories never reach the window
        # at all -- rust-lang/rust's busiest open pull request has 13 --
        # which is exactly why this stayed invisible.
        #
        # `totalCount` is deliberately NOT used for the badge, though it
        # is free: it counts resolved and outdated threads too, so it
        # would overstate the unresolved ones. That is the opposite error,
        # and #808 recorded the same trap.
        reviewThreads(first: 100) { nodes { isResolved isOutdated } }
        # `state` alone is not enough: the rollup RANKS FAILURE above
        # PENDING, so a pull request whose checks are re-running after a
        # fix still reads as failing. Per-check `status` is what
        # separates "failed" from "failed, and now re-running".
        #
        # MEASURED: this takes the query from 3 points to 4, and there is
        # no cheaper shape -- `first: 1`, `first: 20` and `checkSuites`
        # all cost the same, because the cost is the connection rather
        # than the page. Worth it: on a real account 16 of 58 check
        # suites were QUEUED, which is exactly the state being hidden.
        commits(last: 1) {
          nodes {
            commit {
              statusCheckRollup {
                state
                contexts(first: 100) { nodes { ... on CheckRun { status } } }
              }
            }
          }
        }
      }
    }
  }
}
"#;

/// The dashboard counters, as one aliased query costing 1 point.
/// `$week` and `$month` are ISO dates.
///
/// `rateLimit` added for #824. It was the only stats query that asked
/// GitHub for counts without asking what they cost, which mattered once
/// anything started READING cost: `github::stats::budget` accumulates
/// spend per scope load, and a request that reports no cost is counted as
/// `unmetered` rather than guessed at 1 -- so omitting the field here
/// would have made every total that included this query a floor instead
/// of a figure, for no reason beyond the field being absent.
pub const STATS_QUERY: &str = r#"
query($week: String!, $month: String!) {
  rateLimit { cost remaining resetAt }
  merged_week: search(query: $week, type: ISSUE) { issueCount }
  merged_month: search(query: $month, type: ISSUE) { issueCount }
}"#;

/// Detail on the most recent merged PRs, sampled for the insight cards.
///
/// 100 is the per-page maximum. It is a sample, not a census: the totals it
/// feeds are labelled with their sample size in the UI rather than being
/// presented as lifetime figures.
pub const MERGED_DETAIL_QUERY: &str = r#"
query($first: Int!) {
  rateLimit { cost remaining resetAt }
  merged: search(query: "is:pr author:@me is:merged", type: ISSUE, first: $first) {
    nodes {
      ... on PullRequest {
        number
        title
        url
        createdAt
        mergedAt
        additions
        deletions
        changedFiles
        comments { totalCount }
        repository { nameWithOwner }
      }
    }
  }
}"#;

/// Days per request when fetching history.
///
/// Sized for LATENCY, not just the 502 ceiling. GitHub evaluates search
/// aliases serially, so response time scales with alias count: measured
/// 30 aliases = 7.8s but 10 aliases = 2.8s. Chunks are fetched
/// concurrently, so total wall-clock is roughly one chunk rather than the
/// sum -- 30 days went from 17s serial to ~3s.
///
/// 5 days = 10 aliases, far under the ~44 where GitHub starts
/// intermittently returning 502 Bad Gateway.
pub const HISTORY_CHUNK_DAYS: i64 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodRanges {
    pub week_current: (String, String),
    pub week_previous: (String, String),
    pub month_current: (String, String),
    pub month_previous: (String, String),
}

fn day(d: DateTime<Utc>) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// One `search` alias per day per series, `m{i}` merged and `o{i}` opened,
/// counting back `days` from `now`.
///
/// Aliased searches cost ONE rate-limit point in total regardless of how
/// many aliases the query carries — measured against the live API at 60
/// aliases plus 6 period aliases, cost 1. That is why this is a single
/// query rather than paginating merged PRs, which would take 6+ sequential
/// requests at current volume.
pub fn history_query(now: DateTime<Utc>, days: i64) -> String {
    history_query_range(now, 0, days)
}

/// The day buckets for `start..start+len`, keeping alias indices absolute
/// so a chunked fetch can merge results without renumbering.
///
/// GitHub 502s on a query carrying too many concurrent `search` aliases.
/// Measured: 28-36 aliases succeed 5/5, 44-46 fail INTERMITTENTLY (44
/// succeeded once then failed twice; 46 failed once then succeeded twice),
/// and 60 fails outright. It is a server-side timeout, not a documented
/// limit, so the fix is to stay well below it rather than retry into it.
pub fn history_query_range(now: DateTime<Utc>, start: i64, len: i64) -> String {
    // `rateLimit` on every chunk (#824). Each chunk is its own request, so
    // the cost has to be readable per chunk or a 6-chunk history fetch
    // reports a sixth of what it spent. Free: the field is not a
    // connection and the document still costs 1 point.
    let mut q = String::from("query {\n  rateLimit { cost remaining resetAt }\n");
    for i in start..start + len {
        let d = day(now - Duration::days(i));
        q.push_str(&format!(
            "  m{i}: search(query: \"is:pr author:@me is:merged merged:{d}\", type: ISSUE) {{ issueCount }}\n"
        ));
        q.push_str(&format!(
            "  o{i}: search(query: \"is:pr author:@me created:{d}\", type: ISSUE) {{ issueCount }}\n"
        ));
    }
    q.push_str("}\n");
    q
}

/// Comparison windows, each ending YESTERDAY.
///
/// Today is still accumulating, so including it compares a partial period
/// against complete ones and drags every delta downward. Measured on real
/// data: including today reported +47% week-over-week where the honest
/// full-week comparison was +66%.
pub fn period_ranges(now: DateTime<Utc>) -> PeriodRanges {
    let end = now - Duration::days(1);
    let win = |offset: i64, len: i64| {
        let e = end - Duration::days(offset);
        (day(e - Duration::days(len - 1)), day(e))
    };
    PeriodRanges {
        week_current: win(0, 7),
        week_previous: win(7, 7),
        month_current: win(0, 30),
        month_previous: win(30, 30),
    }
}

/// The day buckets plus the six period-comparison aliases, so the chart
/// series and all four delta cards arrive in ONE request at cost 1.
///
/// Built by appending inside the brace `history_query` already closed, so
/// the two are never allowed to drift out of sync.
pub fn history_query_with_periods(now: DateTime<Utc>, days: i64) -> String {
    history_query_range_with_periods(now, 0, days)
}

/// Just the six period-comparison aliases.
///
/// Split out so the delta cards can render from one small, fast request
/// rather than waiting on the whole daily series.
pub fn periods_query(now: DateTime<Utc>) -> String {
    let r = period_ranges(now);
    let mut q = String::from("query {\n  rateLimit { cost remaining resetAt }\n");
    let mut add = |alias: &str, filter: &str, range: &(String, String)| {
        q.push_str(&format!(
            "  {alias}: search(query: \"is:pr author:@me {filter}{}..{}\", type: ISSUE) {{ issueCount }}\n",
            range.0, range.1
        ));
    };
    add("week_current", "is:merged merged:", &r.week_current);
    add("week_previous", "is:merged merged:", &r.week_previous);
    add("opened_week_current", "created:", &r.week_current);
    add("opened_week_previous", "created:", &r.week_previous);
    add("month_current", "is:merged merged:", &r.month_current);
    add("month_previous", "is:merged merged:", &r.month_previous);
    q.push_str("}\n");
    q
}

/// The first chunk of a chunked fetch: day buckets plus the six period
/// aliases, which ride along at no extra cost.
pub fn history_query_range_with_periods(now: DateTime<Utc>, start: i64, len: i64) -> String {
    let base = history_query_range(now, start, len);
    let inner = base
        .strip_suffix("}\n")
        .expect("history_query always ends with a closing brace and newline");
    let r = period_ranges(now);
    let mut q = String::from(inner);
    let mut add = |alias: &str, filter: &str, range: &(String, String)| {
        q.push_str(&format!(
            "  {alias}: search(query: \"is:pr author:@me {filter}{}..{}\", type: ISSUE) {{ issueCount }}\n",
            range.0, range.1
        ));
    };
    add("week_current", "is:merged merged:", &r.week_current);
    add("week_previous", "is:merged merged:", &r.week_previous);
    add("opened_week_current", "created:", &r.week_current);
    add("opened_week_previous", "created:", &r.week_previous);
    add("month_current", "is:merged merged:", &r.month_current);
    add("month_previous", "is:merged merged:", &r.month_previous);
    q.push_str("}\n");
    q
}

/// Cycle time for two adjacent windows, so the headline figure has a prior
/// period to compare against.
///
/// Both windows in ONE aliased document at a total cost of 1 point. Each
/// window is capped at 100 nodes, so for a busy week these are SAMPLES of
/// that week rather than a census -- `sampled` says so, and the UI must
/// not present the result as complete.
pub fn cycle_trend_query(now: DateTime<Utc>) -> String {
    let r = period_ranges(now);
    format!(
        r#"query {{
  rateLimit {{ cost remaining resetAt }}
  current: search(query: "is:pr author:@me is:merged merged:{}..{}", type: ISSUE, first: 100) {{
    issueCount
    nodes {{ ... on PullRequest {{ createdAt mergedAt }} }}
  }}
  previous: search(query: "is:pr author:@me is:merged merged:{}..{}", type: ISSUE, first: 100) {{
    issueCount
    nodes {{ ... on PullRequest {{ createdAt mergedAt }} }}
  }}
}}"#,
        r.week_current.0, r.week_current.1, r.week_previous.0, r.week_previous.1
    )
}

/// Everything the detail view shows, in one request.
///
/// Measured at cost 1 including per-check contexts. Fetched on open
/// rather than in the poll loop: it is per-PR and only needed while the
/// view is on screen.
///
/// Deliberately no file diff and no commit history. Headstate is for
/// deciding and acting; reviewing code belongs in GitHub or an editor,
/// and fetching a diff here would cost far more than a point.
///
/// `rateLimit` added for #844. It was one of two documents the
/// hand-written list in `budget::tests::every_stats_query_meters_itself`
/// never covered, and the derived replacement found it immediately -- which
/// is the whole argument for deriving the list. RE-MEASURED live 2026-09-11
/// on this document extracted verbatim (`gh api graphql`, `pktstorm/headstate`
/// PR 786, 3 runs): **cost 1** in 1.25-1.52s, confirming the figure this
/// comment already claimed and that the field is free.
pub const PR_DETAIL_QUERY: &str = r#"
query($owner: String!, $repo: String!, $number: Int!) {
  rateLimit { cost remaining resetAt }
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      id number title url state isDraft body
      mergeable mergeStateStatus reviewDecision
      # Does THIS pull request's base branch use a merge queue?
      #
      # Per-pull-request rather than per-repository, which is the only
      # correct granularity: the setting is branch-scoped, so a repo can
      # queue `main` and not `release/*`, and asking at the repo level
      # would mislabel the button on every PR targeting another branch.
      #
      # MEASURED against the live API: the detail query still costs 1
      # point with this field, so it is affordable here in a way it
      # would not be on `PRS_QUERY` -- see #312.
      isMergeQueueEnabled
      # Same pair as the list query, and mapped by the same function:
      # `isInMergeQueue` alone reports a REJECTED entry as queued.
      isInMergeQueue mergeQueueEntry { state }
      additions deletions changedFiles
      headRefName headRefOid baseRefName
        headRef { id }
      createdAt updatedAt
      author { login }
      comments(first: 50) {
        totalCount
        nodes { author { login } createdAt body }
      }
      # The DETAIL query carries the whole thread; the list query above
      # keeps its two-field shape and only counts. MEASURED against the
      # live API: this query still costs 1 point with the comments and
      # the three viewer-permission fields, so the detail view can afford
      # what `PRS_QUERY` cannot -- the same reasoning as #312.
      #
      # `viewerCan*` are three separate questions and all three are
      # asked: on a repository where the viewer lacks write access,
      # Resolve renders and then fails with a 403. A button that cannot
      # work must not be shown, and only GitHub can answer that.
      #
      # `first: 100` is the connection maximum, and the page size is NOT
      # the cost -- the same fact the rollup's `contexts` comment below
      # records. Was 20, which silently dropped thread 21 onwards (#802).
      #
      # MEASURED against the live API on a pull request with real
      # threads: `first: 20` and `first: 100` both leave the whole detail
      # query at cost 1, with no measurable latency difference (557ms vs
      # 574ms, inside the noise) and no node-limit error -- 100 threads
      # x 10 comments is 1,000 nodes against GitHub's 500,000 ceiling.
      # Asking for the largest page is therefore free, and it is the
      # reason #802 needs no cursor loop: see `map_review_threads` for
      # the sampling behind that, and why a serial page chain was
      # REJECTED rather than merely skipped.
      #
      # `totalCount` so a truncated list can say how many threads it is
      # missing rather than rendering a plausible-looking subset. Free
      # for the same reason: GitHub charges the connection, not the
      # fields on it. This is what removes the SILENCE, which was the
      # actual defect -- a reviewer deciding from a complete-looking view
      # of 20 of 25 threads decides on partial information without being
      # told it is partial.
      reviewThreads(first: 100) {
        totalCount
        nodes {
          id isResolved isOutdated path line
          viewerCanReply viewerCanResolve viewerCanUnresolve
          # DELIBERATE, and deliberately left at 10 (#802 asks for a
          # decision either way rather than silence).
          #
          # Kept because the gap here is ALREADY honest: `comment_count`
          # carries the thread's real total and `ReviewThreads.tsx` renders
          # "Showing 10 of 14. See the rest on GitHub." A truncated thread
          # therefore says so, which is the property the thread COUNT was
          # missing and this change adds.
          #
          # Raising it is not free the way the page above is. Thread
          # comments are the only nested connection in this query, so the
          # node count is the product of the two pages: 100 x 100 is
          # 10,000 bodies of markdown on the wire for a view that renders
          # the first few and collapses the rest. The cost of a long
          # back-and-forth is also bounded differently -- the tenth
          # comment is a conversation you read on GitHub, where the
          # twenty-first THREAD could be an unanswered blocking question
          # the view never admitted existed. Different severity, and only
          # the second one was silent.
          comments(first: 10) {
            totalCount
            nodes { author { login } createdAt body }
          }
        }
      }
      # Does the BASE branch of this pull request use a merge queue?
      #
      # `mergeQueue(branch:)` is branch-scoped because the setting is:
      # a repository can queue `main` and not `release/*`. Asking about
      # the PR's own base branch is therefore the only correct question
      # -- the repository-wide form would answer about the default
      # branch and mislabel the button on every PR targeting another.
      #
      # MEASURED: adds nothing to the query cost (the whole detail
      # query still totals 1 point), so this is affordable in a way the
      # list query is not -- see #312, which is why this lives on the
      # DETAIL query and not on `PRS_QUERY`.
      #
      # `null` means no queue, and also means "we could not tell". Both
      # collapse to "not queued", which is the safe default: it offers
      # a plain Merge, and GitHub refuses it if the branch really does
      # require the queue.
      # The VIEWER's own latest review, which is a different question
      # from `reviewDecision`. The decision is the pull request's
      # aggregate state: it reads CHANGES_REQUESTED when someone else
      # blocked it, and REVIEW_REQUIRED when a second approval is
      # outstanding -- neither of which tells the user whether THEIR
      # click landed. Approving and then seeing an unchanged button is
      # the reported confusion, so the answer has to be per-viewer.
      #
      # `latestReviews` returns one review per reviewer, so a small page
      # covers any realistic pull request and the viewer's entry is
      # found by matching login.
      latestReviews(first: 20) { nodes { state author { login } } }
      commits(last: 1) {
        nodes { commit { statusCheckRollup {
          state
          # 100 is the connection maximum. The page is NOT the cost --
          # measured on the list query, `first: 1` and `first: 20` cost
          # the same, because GitHub charges the connection -- so asking
          # for the largest page is free and cuts the number of follow-up
          # requests to nearly always zero.
          # `totalCount` so a CAPPED check list can say how many it is
          # missing rather than rendering a plausible-looking subset.
          # #790 cut the page budget from 20 to 3, which means the cap
          # is now reachable on a real pull request -- and the whole
          # reason the pagination exists (see `append_remaining_checks`)
          # is that a truncated check list does not look truncated.
          # Free: GitHub charges the connection, not the fields on it,
          # and the detail query still totals 1 point.
          contexts(first: 100) {
            totalCount
            pageInfo { hasNextPage endCursor }
            nodes {
            ... on CheckRun {
              name conclusion detailsUrl
              # The workflow RUN, not the check run: re-running failed
              # jobs is one REST call per run, where per-check would be
              # one call each and could not re-run a job that never
              # started. `workflowRun` is null for check runs from apps
              # that are not GitHub Actions, which is why the mapper
              # treats it as optional rather than assuming it.
              checkSuite { workflowRun { databaseId } }
            }
            ... on StatusContext { context state targetUrl }
            }
          }
        } } }
      }
    }
  }
}"#;

/// One further page of `statusCheckRollup.contexts`.
///
/// Deliberately narrow: the detail query re-fetches comments, review
/// threads and labels, none of which change between check pages, so
/// paging with it would pay for all of that again per page. Named
/// `ChecksPage` so the follow-up is identifiable in a request log.
///
/// `rateLimit` added for #844, alongside `PR_DETAIL_QUERY` and for the same
/// reason: the derived metering guard found both, where the hand-written
/// six-name list it replaced covered neither. It matters more here than on
/// most documents, because this one runs in a LOOP bounded by `MAX_PAGES =
/// 10` -- so a per-page cost that nothing reads is a spend of up to ten
/// points per detail view that no total could account for.
///
/// MEASURED live 2026-09-11 on this document extracted verbatim (`gh api
/// graphql`, 3 runs): **cost 1 per page** in 1.08-1.17s, so the field is
/// free and the loop's worst case is 10 points.
pub const PR_CHECKS_PAGE_QUERY: &str = r#"
query ChecksPage($owner: String!, $repo: String!, $number: Int!, $after: String!) {
  rateLimit { cost remaining resetAt }
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      commits(last: 1) {
        nodes { commit { statusCheckRollup {
          contexts(first: 100, after: $after) {
            # Re-selected per page so the merged value is the one from
            # the LAST page fetched. A rollup that grows mid-pagination
            # (a workflow that queues more jobs) would otherwise report
            # a total from before the growth, understating what is
            # missing -- and understating is the failure mode #790's cap
            # is specifically guarding against.
            totalCount
            pageInfo { hasNextPage endCursor }
            nodes {
              ... on CheckRun {
                name conclusion detailsUrl
                checkSuite { workflowRun { databaseId } }
              }
              ... on StatusContext { context state targetUrl }
            }
          }
        } } }
      }
    }
  }
}"#;

/// The authenticated user's login, and what asking cost.
///
/// # Why this is a named const and not an inline literal (#844)
///
/// It used to be `json!({ "query": "query { viewer { login } }" })` written
/// inline at `client.rs:889`, and it selected no `rateLimit`. MEASURED live
/// 2026-09-11 via the `X-Ratelimit-Used` response header, which advances
/// whether or not the document asks: three consecutive `viewer { login }`
/// requests moved it 86 -> 87 -> 88, and the same document WITH
/// `rateLimit { cost }` moved it 88 -> 89. **One point either way** -- so the
/// request was always costing a point and simply not reporting it, and adding
/// the field is free.
///
/// The consequence was worse than an undercount. `Budget::record` counts a
/// response with no `rateLimit` as `unmetered`, which is what makes
/// `Spend::is_exact()` false and a total honest about being a floor -- but
/// `fetch_viewer` was called BEFORE `Budget::new()` (`commands.rs:2301`,
/// `:2638`), so it was not recorded at all. `points` and `requests`
/// understated by one per call while `is_exact()` returned **true**: exactly
/// what `budget.rs:252-255` forbids -- "the accumulated spend UNDERSTATES the
/// truth ... A reported total must say so rather than look exact."
///
/// Notably `board_projection` already budgeted for it (`commands.rs:2557`,
/// `// +1 for fetch_viewer.`), so the PROJECTION knew about a request the
/// ACCOUNTING did not.
///
/// Named rather than left inline so
/// `budget::tests::every_stats_query_meters_itself` can find it: that guard
/// enumerated six query names by hand and omitted this one, which is the same
/// list-based blind spot as #842's poll guard. It now derives its list from
/// this file, and an inline literal would be invisible to that.
pub const VIEWER_QUERY: &str = r#"
query {
  rateLimit { cost remaining resetAt }
  viewer { login }
}
"#;

/// How many pull requests a search matches, and nothing else.
///
/// `issueCount` alone: no nodes, so none of the per-pull-request fields
/// that make the list query expensive are resolved at all. This exists
/// because the sidebar badge needs a NUMBER, and fetching 100 fully
/// populated pull requests to render one was both the largest wasted
/// request in the app and a way to fail on a view that shows no pull
/// requests at all.
pub const COUNT_QUERY: &str = r#"
query($q: String!) {
  rateLimit { cost remaining resetAt }
  matching: search(query: $q, type: ISSUE) { issueCount }
}
"#;

/// How many branch names one merged-PR lookup asks about (#1440).
///
/// 36, the ceiling this app already chunks aliased searches at: GitHub
/// answers 502 above roughly 44 aliases in one document. MEASURED live
/// 2026-09-25: 36 aliases at `first: 10` cost ONE rate-limit point.
pub const MERGED_HEADS_CHUNK: usize = 36;

/// Merged pull requests for up to [`MERGED_HEADS_CHUNK`] head branch
/// names in one repository (#1440).
///
/// The worktree view's merge detection is offline and content-based, and
/// it loses a squash-merged branch as soon as the default branch edits the
/// same files again. GitHub's record of the merge does not decay that way,
/// so this document asks for it -- as a POSITIVE signal only; see
/// `worktrees::github` for the strict rule the answer has to pass.
///
/// Each alias `hN` is bound to the variable `$hN`, so a branch name is
/// never spliced into the document text: branch names are arbitrary
/// strings the user (or an agent) chose, and quoting them by hand is how a
/// name containing `"` becomes a malformed or different query.
///
/// `defaultBranchRef` is asked for because the rule needs the base to be
/// the default branch as GITHUB names it, not as a local ref happens to.
/// `headRefOid` is the exact commit GitHub merged, which is what the rule
/// compares the worktree's HEAD against. `first: 10` because branch names
/// are reused; a name merged more than ten times is simply not upgraded,
/// which is the safe direction.
pub fn merged_heads_query(n: usize) -> String {
    let mut vars = String::new();
    let mut aliases = String::new();
    for i in 0..n {
        vars.push_str(&format!(", $h{i}: String!"));
        aliases.push_str(&format!(
            "    h{i}: pullRequests(headRefName: $h{i}, states: MERGED, first: 10, \
             orderBy: {{field: UPDATED_AT, direction: DESC}}) \
             {{ nodes {{ number headRefOid baseRefName }} }}\n"
        ));
    }
    format!(
        "query($owner: String!, $name: String!{vars}) {{\n  \
         rateLimit {{ cost remaining resetAt }}\n  \
         repository(owner: $owner, name: $name) {{\n    \
         defaultBranchRef {{ name }}\n{aliases}  }}\n}}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn builds_two_aliases_per_day() {
        let q = history_query(at("2026-08-20T14:00:00Z"), 30);
        assert_eq!(q.matches("search(").count(), 60);
        assert!(q.contains("m0: search"));
        assert!(q.contains("o29: search"));
    }

    #[test]
    fn counts_back_from_today() {
        let q = history_query(at("2026-08-20T14:00:00Z"), 2);
        assert!(q.contains("merged:2026-08-20"));
        assert!(q.contains("merged:2026-08-19"));
    }

    // Naive day arithmetic breaks across month ends; this pins the
    // behaviour chrono gives us.
    #[test]
    fn crosses_a_leap_day() {
        let q = history_query(at("2024-03-01T00:30:00Z"), 2);
        assert!(q.contains("merged:2024-02-29"));
    }

    // The current day is incomplete, so counting it against full prior
    // weeks understates the trend. Windows end yesterday.
    #[test]
    fn period_windows_exclude_today_and_do_not_overlap() {
        let r = period_ranges(at("2026-08-20T14:00:00Z"));
        assert_eq!(
            r.week_current,
            ("2026-08-13".to_string(), "2026-08-19".to_string())
        );
        assert_eq!(
            r.week_previous,
            ("2026-08-06".to_string(), "2026-08-12".to_string())
        );
    }

    #[test]
    fn month_windows_are_thirty_days_each() {
        let r = period_ranges(at("2026-08-20T14:00:00Z"));
        assert_eq!(
            r.month_current,
            ("2026-07-21".to_string(), "2026-08-19".to_string())
        );
        assert_eq!(
            r.month_previous,
            ("2026-06-21".to_string(), "2026-07-20".to_string())
        );
    }

    // The combined query must stay valid GraphQL: exactly one top-level
    // brace pair, with the period aliases INSIDE it.
    /// The README states the chunk size in prose. It drifted once already
    /// -- it claimed 15 days and three points after HISTORY_CHUNK_DAYS
    /// became 5 and the cost became 8 -- so the doc is pinned to the
    /// constant rather than trusted to be updated by hand.
    #[test]
    fn the_readme_matches_the_chunk_constant() {
        let readme = include_str!("../../../README.md");
        let spelled = match HISTORY_CHUNK_DAYS {
            5 => "five",
            7 => "seven",
            10 => "ten",
            15 => "fifteen",
            n => panic!("add a spelling for {n} days"),
        };
        assert!(
            readme.contains(&format!("chunks of {spelled} days")),
            "README does not describe {HISTORY_CHUNK_DAYS}-day chunks"
        );
    }

    // GitHub 502s on too many concurrent search aliases, intermittently
    // from ~44, and latency scales with alias count besides. A chunk must
    // stay well under that ceiling at whatever HISTORY_CHUNK_DAYS is set to.
    #[test]
    fn a_chunk_stays_under_the_alias_ceiling() {
        let now = at("2026-08-20T14:00:00Z");
        let first = history_query_range_with_periods(now, 0, HISTORY_CHUNK_DAYS);
        let rest = history_query_range(now, HISTORY_CHUNK_DAYS, HISTORY_CHUNK_DAYS);
        // Two aliases per day, plus six period aliases in the first chunk.
        let per_chunk = (HISTORY_CHUNK_DAYS * 2) as usize;
        assert_eq!(first.matches("search(").count(), per_chunk + 6);
        assert_eq!(rest.matches("search(").count(), per_chunk);
        // Derived from the constant, so changing the chunk size cannot
        // silently drift past the ceiling.
        assert!(
            first.matches("search(").count() <= 40,
            "chunk too close to the ~44-alias 502 ceiling"
        );
    }

    // Alias indices must be ABSOLUTE, or merging chunks would overwrite
    // day 0 with day 15 and silently corrupt the series.
    #[test]
    fn chunk_alias_indices_are_absolute() {
        let now = at("2026-08-20T14:00:00Z");
        let rest = history_query_range(now, 15, 3);
        assert!(rest.contains("m15: search"));
        assert!(rest.contains("m17: search"));
        assert!(!rest.contains("m0: search"));
        // And the dates line up with the absolute index.
        assert!(rest.contains("merged:2026-08-05"));
    }

    #[test]
    fn combined_query_is_balanced_and_complete() {
        let q = history_query_with_periods(at("2026-08-20T14:00:00Z"), 3);
        assert_eq!(q.matches('{').count(), q.matches('}').count());
        assert_eq!(q.matches("search(").count(), 12); // 3 days x 2 + 6 periods
        assert!(q.trim_end().ends_with('}'));
        assert!(q.contains("week_current: search"));
        assert!(q.contains("month_previous: search"));
        // The transition from the last day bucket to the first period
        // alias must not carry a closing top-level brace. Alias lines end
        // in "}" from "{ issueCount }", so check the line itself.
        // Alias lines end in "}" from "{ issueCount }", so a substring
        // check is meaningless here. What matters: exactly ONE bare
        // closing brace, and it is the last line.
        let bare: Vec<usize> = q
            .lines()
            .enumerate()
            .filter(|(_, l)| l.trim() == "}")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(bare.len(), 1, "exactly one top-level closing brace");
        assert_eq!(bare[0], q.lines().count() - 1, "and it closes the query");
    }

    /// The review-thread fix lives in the QUERY, and the mapper's tests
    /// cannot see it: they feed `map_detail` a JSON literal, so they pass
    /// just as happily against a document that never asks for
    /// `totalCount` or that still asks for 20 threads. Without this guard
    /// the whole of #802 can be silently undone -- and the symptom would
    /// be a view that looks complete, which is the defect itself.
    /// The LIST query's thread window, which the guard below does not
    /// cover (#810).
    ///
    /// A separate test rather than a widened one, because the two
    /// documents are the same shape for different reasons and a single
    /// assertion over both would pass while one of them regressed. The
    /// detail query needs the full page so a reviewer sees every thread;
    /// this one needs it so a COUNT is exact. Note this query must not
    /// select `totalCount` for the badge -- it counts resolved and
    /// outdated threads too, so reading it would overstate the
    /// unresolved ones -- which is why the assertion here is only about
    /// the page size.
    ///
    /// Measured free before being widened: the document still costs 1
    /// point at 100 pull requests x 100 threads. A future narrowing
    /// would silently restore an undercount that no mapper test can see,
    /// since the mapper is correct either way and only the window lies.
    #[test]
    fn the_list_query_counts_every_thread_rather_than_the_first_twenty() {
        let q = PRS_QUERY;
        let threads = q
            .split_once("reviewThreads(")
            .expect("the list query asks for review threads")
            .1;
        let window = threads.split_once(')').expect("the connection closes").0;
        assert!(
            window.contains("first: 100"),
            "the badge is a COUNT, so its window must be the connection maximum; \
             found `reviewThreads({window})`, which caps the number silently"
        );
    }

    #[test]
    fn the_detail_query_asks_for_every_thread_and_its_true_count() {
        let q = PR_DETAIL_QUERY;
        // 100 is the connection maximum, and MEASURED free: the detail
        // query still costs 1 point, with no latency difference and no
        // node-limit error. A smaller page is a silent regression, since
        // no mapper test can tell the difference.
        assert!(
            q.contains("reviewThreads(first: 100)"),
            "threads must be asked for at the connection maximum"
        );
        // Nested inside that connection, NOT the comment connection's own
        // `totalCount` a line or two down, so the assertion would fail if
        // the field were dropped from the threads but kept on comments.
        let threads = q
            .split_once("reviewThreads(first: 100)")
            .expect("the thread connection")
            .1;
        let before_nodes = threads.split_once("nodes").expect("thread nodes").0;
        assert!(
            before_nodes.contains("totalCount"),
            "the thread connection must select totalCount, or truncation is silent again"
        );
    }

    /// Every PAGED connection in the list query's node selection must
    /// select `totalCount`, or its truncation is silent again (#1089).
    ///
    /// # Derived, not a list
    ///
    /// The obvious guard names the four connections #1089 fixed, and has
    /// the failure mode `poll.rs`'s cost guard documents as its third
    /// occurrence: a hand-written list cannot cover the connection nobody
    /// remembered to add. So this scans for the SHAPE instead -- any
    /// `name(` taking an argument in the node selection -- exactly as the
    /// cost guard's deny-list does, and for the same reason. A fifth
    /// paged connection added tomorrow is covered without anyone editing
    /// this test.
    ///
    /// `reviewThreads` is the one deliberate exemption, and it is
    /// EXEMPTED BY NAME with its reason recorded beside it: its badge
    /// counts unresolved, un-outdated threads, while `totalCount`
    /// includes the resolved and outdated ones. Reading it would
    /// OVERSTATE the badge, which #808 and #810 both recorded as the
    /// opposite error. Its own guard above pins its window at 100
    /// instead, so it is not unguarded -- it is guarded differently.
    #[test]
    fn every_paged_connection_in_the_list_query_reports_its_true_total() {
        // Comment lines first. This repo documents rules directly above
        // the code they govern, so `latestReviews(` appears in prose here
        // far more often than in the document -- matching a comment
        // instead of code is #874's mistake, and these `#` lines are
        // stripped before the server ever sees them anyway.
        let doc: String = PRS_QUERY
            .replace("\r\n", "\n")
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let nodes = doc
            .split_once("... on PullRequest {")
            .expect("the node selection")
            .1;

        // `reviewThreads` is exempt for the reason in this test's docs;
        // `commits`/`contexts` carry no count the UI renders, and the
        // check list's own total is selected in the DETAIL query where it
        // is shown.
        //
        // `timelineItems` is `last: 1` of one event type (#1407): the page
        // IS the answer, the latest ready-for-review event, so there is no
        // truncation for a count to reveal.
        const EXEMPT: [&str; 4] = ["reviewThreads", "commits", "contexts", "timelineItems"];

        let mut rest = nodes;
        let mut checked = 0;
        while let Some(i) = rest.find('(') {
            let head = &rest[..i];
            let start = head
                .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(0, |p| p + 1);
            let name = head[start..].to_string();
            rest = &rest[i + 1..];
            if name.is_empty() || EXEMPT.contains(&name.as_str()) {
                continue;
            }
            // The connection's own selection set, up to its `nodes` --
            // so a sibling connection's `totalCount` further down cannot
            // satisfy this one. That is the containment mistake the
            // detail-query guard above already had to avoid.
            let body = rest.split_once("nodes").map_or(rest, |(b, _)| b);
            assert!(
                body.contains("totalCount"),
                "`{name}(` is a paged connection in PRS_QUERY's node selection \
                 and does not select `totalCount`, so a truncated list is \
                 indistinguishable from a complete one (#1089). Add it -- \
                 measured free, since GitHub prices the `first:` argument and \
                 this connection is paying for it already."
            );
            checked += 1;
        }
        // The scan must actually have found something. A refactor that
        // renamed the split marker would leave `nodes` empty and this
        // test passing over nothing -- which is a guard reporting safety
        // it is not checking.
        assert!(
            checked >= 4,
            "expected at least the four connections #1089 fixed, scanned {checked}"
        );
    }

    /// `latestReviews` must ask for the same page in BOTH documents.
    ///
    /// `map.rs`'s `latest_reviews` is shared between the list and detail
    /// mappers "so the row and the view it opens cannot disagree about
    /// who reviewed". Sharing the MAPPER does nothing if the page sizes
    /// differ: at `first: 5` on the list and `first: 20` on the detail, a
    /// row and the view it opens could name different reviewers for the
    /// same pull request, which is precisely what that comment claims the
    /// design prevents (#1089).
    ///
    /// Derived from the two documents rather than asserted against a
    /// literal, so raising one without the other fails here instead of
    /// shipping a disagreement no mapper test can see.
    #[test]
    fn the_row_and_its_detail_view_ask_for_the_same_reviewers() {
        fn window(doc: &str) -> String {
            // Comment lines stripped first: #874's sabotage proof caught a
            // guard matching its own pattern inside a doc comment, and
            // both documents discuss `latestReviews` in prose directly
            // above the field.
            let code: String = doc
                .replace("\r\n", "\n")
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n");
            let after = code
                .split_once("latestReviews(")
                .expect("latestReviews is asked for")
                .1;
            after
                .split_once(')')
                .expect("the connection closes")
                .0
                .trim()
                .to_string()
        }
        let list = window(PRS_QUERY);
        let detail = window(PR_DETAIL_QUERY);
        assert_eq!(
            list, detail,
            "the list asks for `latestReviews({list})` and the detail view for \
             `latestReviews({detail})`. A shared mapper cannot reconcile \
             different page sizes: the row and the view it opens would \
             disagree about who reviewed (#1089)."
        );
    }

    /// Every field a mapper READS, taken from the mapper's own source.
    ///
    /// # Why this is derived rather than a list
    ///
    /// #847's fix asks for a shape guard on documents whose mappers feed on
    /// `json!` literals in their own tests -- a document can drop a field and
    /// every mapper test still passes, because the literal supplies it. The
    /// obvious guard is `assert!(doc.contains("issueCount"))` per field, and
    /// the obvious guard has the failure mode #842's poll guard and #844's
    /// metering guard both have and both document: a HAND-WRITTEN list of
    /// names cannot cover the name nobody remembered to add. `poll.rs`'s own
    /// comment calls that out as its third occurrence.
    ///
    /// So the list comes from the mapper. `serde_json::Value` is indexed by
    /// string literal -- `v["current"]["issueCount"]` -- so every field a
    /// mapper reads appears in its body as `["name"]`, and scanning the
    /// source for that pattern yields the read set exactly. Add a field to a
    /// mapper and the guard demands it in the document on the next `cargo
    /// test`, with nothing to remember.
    ///
    /// # What it cannot see, and why that is acceptable
    ///
    /// - A field read through a LOCAL variable rather than a literal index
    ///   (`v[&alias]`, which `stats/fetch.rs` does for its aliases). Those
    ///   are alias names rather than schema fields, and the alias guards in
    ///   `stats/query.rs` cover them.
    /// - A field read in a HELPER the mapper calls. Helpers are passed
    ///   explicitly here for that reason -- `check_state` reads `conclusion`
    ///   and `state`, which `map_detail_checks` never names itself.
    /// - A name that is a field in one document and a local key in another
    ///   (`nodes`, `state`). Harmless: a document that selects nodes contains
    ///   the word, and one that does not has a real gap.
    ///
    /// It is a SHAPE guard, not a schema check: it asserts the document
    /// mentions the field, not that it mentions it in the right place. The
    /// nesting checks that need precision are written separately below
    /// (`a_cycle_window_reports_its_true_total_beside_its_nodes`), which is
    /// the split `stats/query.rs:550` already uses.
    fn fields_read_by(fns: &[&str]) -> Vec<String> {
        let src = include_str!("map.rs");
        let mut out = std::collections::BTreeSet::new();
        for name in fns {
            let anchor = format!("fn {name}(");
            let from = src
                .find(&anchor)
                .unwrap_or_else(|| panic!("{name} not found in map.rs -- rename it here too"));
            let body = &src[from..];
            // To the next top-level item, so only THIS function's reads are
            // collected. `every_stats_query_meters_itself` scopes the same
            // way and for the same reason: a test that reads the wrong
            // region reports a defect at a location that does not have one.
            let end = ["\nfn ", "\npub fn ", "\n#[cfg(test)]"]
                .iter()
                .filter_map(|m| body[1..].find(m).map(|i| i + 1))
                .min()
                .unwrap_or(body.len());
            let body = &body[..end];
            // `["name"]` -- serde_json's index-by-literal, which is how
            // every mapper in this file names a GraphQL field.
            let mut rest = body;
            while let Some(i) = rest.find("[\"") {
                rest = &rest[i + 2..];
                let Some(j) = rest.find('"') else { break };
                let name = &rest[..j];
                if rest[j..].starts_with("\"]")
                    && !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    out.insert(name.to_string());
                }
                rest = &rest[j..];
            }
        }
        assert!(
            !out.is_empty(),
            "no field reads found -- the scan is broken, not the documents"
        );
        out.into_iter().collect()
    }

    /// Deleting `issueCount` from `cycle_trend_query` turns a SAMPLE into a
    /// census, and nothing else notices.
    ///
    /// `map_cycle_trend` (`map.rs:666-686`) computes
    /// `sampled: cur_n > cur_len` from `issueCount`, through
    /// `as_u64().unwrap_or(0)`. Drop the field and `cur_n` is 0, so
    /// `0 > 100` is false, `sampled` flips to FALSE, and a 100-PR sample of
    /// a busy week is presented as the complete week -- a median cycle time
    /// labelled as the week's, computed from an arbitrary 100 of N.
    ///
    /// The three mapper tests (`map.rs:1334`, `:1358`, `:1371`) cannot catch
    /// it: they feed `json!` literals that supply `issueCount` themselves,
    /// which is exactly the hole
    /// `the_detail_query_asks_for_every_thread_and_its_true_count` above
    /// describes for review threads and `stats/query.rs:550` closes for the
    /// detail slice. This is the same guard for the same property.
    #[test]
    fn the_cycle_trend_query_asks_for_every_field_its_mapper_reads() {
        let q = cycle_trend_query(at("2026-08-20T14:00:00Z"));
        for f in fields_read_by(&["map_cycle_trend", "median_hours"]) {
            assert!(
                q.contains(&f),
                "map_cycle_trend reads `{f}` and cycle_trend_query does not select it; \
                 a field the mapper defaults would be silently missing"
            );
        }
    }

    /// And `issueCount` must sit on the SAME connection as the nodes it
    /// qualifies, per window.
    ///
    /// The derived guard above asserts the document mentions `issueCount`; it
    /// cannot tell `current` from `previous`, so dropping it from ONE window
    /// would still pass. `sampled` is an OR over both windows, so one window
    /// losing its count silently halves the check.
    ///
    /// The same nesting assertion `stats/query.rs:550`
    /// (`a_detail_slice_reports_its_true_total_beside_its_nodes`) makes for
    /// its slices, for the same reason: the count and the node list have to
    /// describe the same search or the comparison between them means nothing.
    #[test]
    fn a_cycle_window_reports_its_true_total_beside_its_nodes() {
        let q = cycle_trend_query(at("2026-08-20T14:00:00Z"));
        for window in ["current: search", "previous: search"] {
            let body = q
                .split_once(window)
                .unwrap_or_else(|| panic!("the {window} alias"))
                .1;
            let before_nodes = body
                .split_once("nodes")
                .unwrap_or_else(|| panic!("{window} nodes"))
                .0;
            assert!(
                before_nodes.contains("issueCount"),
                "{window} must select issueCount beside its nodes, or a 100-PR \
                 sample of that window reads as the complete window"
            );
        }
    }

    /// Deleting `pageInfo` from `PR_CHECKS_PAGE_QUERY` stops the cursor loop
    /// after page 1 while all three of its tests pass.
    ///
    /// The loop reads `pageInfo` (`client.rs:822`, `:830`) and the three
    /// tests that exercise it (`client.rs:2379`, `:2445`, `:2510`) are
    /// wiremock tests whose RESPONSES are `json!` literals supplying
    /// `pageInfo` themselves. Their only contact with the document is the
    /// mock router's `body_string_contains("ChecksPage")` -- an operation
    /// name chosen to route mocks, not a contract. A PR with 150 checks would
    /// show 100, which is #790's understated-shortfall bug reintroduced.
    ///
    /// Derived from `map_detail_checks` for the node fields, plus the three
    /// the LOOP reads rather than the mapper: `pageInfo`, `hasNextPage` and
    /// `endCursor` are read in `client.rs`, and `totalCount` is read by
    /// `map_detail`'s `checks_total`. Those four are named here because they
    /// live in a different function in a different file; every per-check
    /// field comes from the mapper's own source.
    #[test]
    fn the_checks_page_query_asks_for_every_field_its_readers_read() {
        let q = PR_CHECKS_PAGE_QUERY;
        for f in fields_read_by(&["map_detail_checks", "check_state"]) {
            assert!(
                q.contains(&f),
                "map_detail_checks reads `{f}` and PR_CHECKS_PAGE_QUERY does not \
                 select it; the merged page would be missing it silently"
            );
        }
        // What the PAGINATION reads, which no mapper names. Without these the
        // loop stops after one page and the list is short without saying so.
        for f in ["pageInfo", "hasNextPage", "endCursor"] {
            assert!(
                q.contains(f),
                "the cursor loop at client.rs:822 reads `{f}`; without it \
                 pagination stops after page 1 and a 150-check PR shows 100"
            );
        }
        // Re-selected per page so the merged total is the LAST page's -- a
        // rollup that grows mid-pagination would otherwise report a total
        // from before the growth, understating what is missing.
        assert!(
            q.contains("totalCount"),
            "each page must carry the connection's own total, or a capped \
             check list cannot say how many it is missing"
        );
        // Named so the follow-up is identifiable in a request log, and
        // because `client.rs`'s own wiremock router matches on it: renaming
        // the operation would silently stop routing the mocks.
        assert!(q.contains("query ChecksPage"), "the operation name is read");
    }

    /// `MERGED_DETAIL_QUERY` had NO text assertion at all (#847's table).
    ///
    /// It is the insight-card sample, and every figure on those cards is a
    /// scalar off a node: drop `additions` and the size distribution reads as
    /// zeros rather than as missing, because `map_merged_detail` defaults
    /// each field. Derived from the mapper, so a card added later cannot
    /// reach a document that does not feed it.
    #[test]
    fn the_merged_detail_query_asks_for_every_field_its_mapper_reads() {
        for f in fields_read_by(&["map_merged_detail"]) {
            assert!(
                MERGED_DETAIL_QUERY.contains(&f),
                "map_merged_detail reads `{f}` and MERGED_DETAIL_QUERY does not \
                 select it; the card would render a default as a measurement"
            );
        }
    }

    /// `COUNT_QUERY`'s `matching:` alias, pinned against its reader.
    ///
    /// `count_reviewing` (`client.rs:554`) reads `v["matching"]["issueCount"]`
    /// through `unwrap_or(0)`. Rename the alias in the document and the
    /// sidebar badge reads 0 -- "no pull requests await your review", which
    /// is a claim rather than a gap, and is indistinguishable from the true
    /// answer on a quiet day.
    #[test]
    fn the_count_query_alias_matches_its_reader() {
        assert!(
            COUNT_QUERY.contains("matching: search"),
            "count_reviewing reads the `matching` alias; renaming it here \
             makes the sidebar badge read 0 rather than fail"
        );
        assert!(COUNT_QUERY.contains("issueCount"));
        // A COUNT: no nodes, which is the whole reason this document exists
        // rather than reusing PRS_QUERY. Measured at 1 point and ~0.9s
        // against 2 points and ~2.3s for the list (`client.rs:545-551`, and
        // the 2-point figure re-measured live 2026-09-11 for #842).
        assert!(
            !COUNT_QUERY.contains("nodes"),
            "nodes here would resolve the per-PR fields this document exists \
             to avoid"
        );
    }

    /// `VIEWER_QUERY` selects the one field its mapper reads.
    ///
    /// The shortest document in the app and the one with the most leverage:
    /// `map_viewer` reads `["viewer"]["login"]`, and the login is what
    /// every `author:@me`-shaped search is built from. #769 records what a
    /// defaulted login costs -- `unwrap_or_default()` would have sent
    /// `user:` to GitHub, "which returns no results rather than an error".
    /// So the document losing `login` does not fail; it empties every list
    /// in the app, quietly.
    ///
    /// Derived from the mapper rather than listed, the same way
    /// `the_cycle_trend_query_asks_for_every_field_its_mapper_reads` is.
    /// #844 promoted this document out of an inline literal in `client.rs`
    /// specifically so guards could see it; this is the shape half of
    /// that, where the metering guard was the cost half (#854).
    #[test]
    fn the_viewer_query_asks_for_every_field_its_mapper_reads() {
        for f in fields_read_by(&["map_viewer"]) {
            assert!(
                VIEWER_QUERY.contains(&f),
                "map_viewer reads `{f}` and VIEWER_QUERY does not select it; an absent \
                 login defaults to empty, and `author:@me` built from an empty login \
                 returns NO RESULTS rather than an error (#769)"
            );
        }
        // The nesting, which a read-set derivation cannot express: `login`
        // has to be inside `viewer`, not merely somewhere in the document.
        let viewer = VIEWER_QUERY
            .split_once("viewer {")
            .expect("the viewer selection")
            .1;
        assert!(
            viewer
                .split_once('}')
                .expect("viewer closes")
                .0
                .contains("login"),
            "login must be selected INSIDE viewer"
        );
    }

    /// `STATS_QUERY` carries both aliases its reader reads, count-only.
    ///
    /// `fetch_stats` reads `merged_week` and `merged_month` by name, so a
    /// renamed alias reads as a zero rather than as a failure -- the same
    /// property `the_count_query_alias_matches_its_reader` pins for
    /// `COUNT_QUERY`, which is its sibling and was guarded while this was
    /// not (#854).
    #[test]
    fn the_stats_query_aliases_match_their_reader() {
        for alias in ["merged_week: search", "merged_month: search"] {
            assert!(
                STATS_QUERY.contains(alias),
                "fetch_stats reads the `{alias}` alias by name; renaming it here makes \
                 the dashboard read 0 merged pull requests rather than fail"
            );
        }
        assert!(STATS_QUERY.contains("issueCount"));
        // COUNT-only, which is the whole reason this document is separate
        // from `PRS_QUERY`. `nodes` here would resolve the per-PR fields
        // that make the list query cost 6 points instead of 1.
        assert!(
            !STATS_QUERY.contains("nodes"),
            "nodes here would resolve the per-PR fields this document exists to avoid"
        );
    }

    /// `periods_query` carries all six aliases `fetch_periods` reads.
    ///
    /// Six, derived from the READER rather than listed here, so an alias
    /// added to `Periods` demands its place in the document on the next
    /// `cargo test`. `fetch_periods` reads each one by a string literal, so
    /// the read set is extractable exactly the way `fields_read_by` does it
    /// for a mapper -- and a missing alias renders as a zeroed period,
    /// which is a delta card claiming a change that did not happen.
    ///
    /// `combined_query_is_balanced_and_complete` already checks two of
    /// these names on `history_query_with_periods`, the document that
    /// appends them. It never checked `periods_query`, which is the
    /// standalone request the delta cards fire on their own (#854).
    #[test]
    fn the_periods_query_carries_every_alias_its_reader_reads() {
        let q = periods_query(at("2026-08-20T14:00:00Z"));
        // The reader's own source, scoped to `fetch_periods`.
        //
        // Line endings normalised first: a Windows checkout with
        // `core.autocrlf` has CRLF, so the `"\n    pub async fn "` scope
        // terminator below would find nothing and the body would run to
        // the end of the file. The `found == 6` assertion would still
        // catch that here, but relying on a downstream count to cover a
        // broken scope is how a guard reads the wrong region and reports
        // a defect at a location that does not have one. Observed on the
        // `platform (windows-latest)` job by `health::runaway`'s guard.
        let client = include_str!("client.rs").replace("\r\n", "\n");
        let client = client.as_str();
        let from = client
            .find("pub async fn fetch_periods(")
            .expect("fetch_periods not found in client.rs");
        let body = &client[from..];
        let end = body[1..]
            .find("\n    pub async fn ")
            .map_or(body.len(), |j| j + 1);
        let body = &body[..end];

        let mut found = 0usize;
        for line in body.lines() {
            // `count("week_current")?` -- the alias as a quoted literal.
            let Some((_, rest)) = line.split_once("count(\"") else {
                continue;
            };
            let Some((alias, _)) = rest.split_once('"') else {
                continue;
            };
            found += 1;
            assert!(
                q.contains(&format!("{alias}: search")),
                "fetch_periods reads the `{alias}` alias and periods_query does not \
                 emit it; the delta card would render a zero as a real period"
            );
        }
        assert_eq!(
            found, 6,
            "six period aliases are expected; found {found} -- the scan is broken, \
             not the document"
        );
        // And count-only: these are six searches in one request precisely
        // because none of them resolves a node.
        assert!(
            !q.contains("nodes"),
            "nodes would make six aliased searches resolve per-PR fields"
        );
    }

    /// Every GraphQL document has a shape guard naming it.
    ///
    /// # What it enforces
    ///
    /// The document list is DERIVED -- it reuses
    /// `stats::budget::tests::every_query_document`, the same scan #857's
    /// metering guard runs on -- and every document it finds must be named
    /// by at least one test in a guard file, or appear in `NO_SHAPE_GUARD`
    /// below with a reason.
    ///
    /// # The finding it would have caught (#847, #854)
    ///
    /// #847 is the canonical statement of why a shape guard is needed at
    /// all: a mapper's tests feed it `json!` literals, so they supply the
    /// fields themselves and pass just as happily against a document that
    /// stopped asking for one. `cycle_trend_query`'s own guard spells the
    /// cost out -- drop `issueCount` and `map_cycle_trend` computes
    /// `sampled` from a zero, `0 > 100` is false, and a 100-PR sample of a
    /// busy week is presented as the complete week.
    ///
    /// #847 then fixed TWO documents by hand and #857 built
    /// `fields_read_by` to derive the field list from the mapper. Both are
    /// good. Neither asked the general question: IS every document
    /// guarded? It was still a set of individually-written tests, so a new
    /// document arrives unguarded by default and nothing says so -- which
    /// is the same list-shaped blind spot #842, #844 and #847 each were.
    ///
    /// # What it cannot see, stated rather than glossed
    ///
    /// - **Whether the guard is any GOOD.** It checks that a test NAMES
    ///   the document, not that the test asserts anything useful about it.
    ///   A test mentioning `PRS_QUERY` in a comment would satisfy this.
    ///   That is a real weakness and the reason `NO_SHAPE_GUARD`'s entries
    ///   carry reasons rather than being waved through: this guard raises
    ///   the floor from "nobody checked" to "somebody wrote a test naming
    ///   it", which is where a text scan's reach ends.
    /// - **Which mapper belongs to which document.** That binding is not
    ///   derivable from source: only four of the eleven documents are
    ///   built in the same function as the `map_*` call that reads them,
    ///   and three read their fields inline with no mapper at all. It was
    ///   attempted and abandoned rather than approximated -- a guess at
    ///   that pairing would assert the wrong mapper's read set against a
    ///   document and report a defect at a location that does not have
    ///   one, which is the mistake `every_stats_query_meters_itself`
    ///   records an earlier version of itself making.
    /// - **`mutate.rs`' nine documents**, which are named `*_DOC` and so
    ///   match no `query`/`QUERY` pattern. They are mutations: a dropped
    ///   field makes a write fail loudly rather than return a confident
    ///   partial answer, which is the property this whole family of guards
    ///   is about. Out of scope deliberately, not overlooked.
    #[test]
    fn every_graphql_document_has_a_shape_guard() {
        /// Documents with no shape guard, and why.
        ///
        /// An explicit, reviewed list with a reason each -- the form this
        /// codebase insists on over a looser pattern
        /// (`check-privacy.sh:120`, ~40 false positives from one
        /// unanchored regex).
        const NO_SHAPE_GUARD: &[(&str, &str)] = &[
            // Delegators: their body forwards to another builder, so the
            // document -- and its guard -- live one level down.
            (
                "history_query",
                "delegates to history_query_range, which is guarded",
            ),
            (
                "history_query_with_periods",
                "delegates to history_query_range_with_periods",
            ),
        ];

        let docs = crate::github::stats::budget::tests::every_query_document();
        // Every test source that could hold a guard. The guards live beside
        // the documents they check, which is where this codebase puts them.
        //
        // Line endings normalised: a Windows checkout with `core.autocrlf`
        // has CRLF, so the `"\n#[cfg(test)]"` split below would find
        // nothing, the closure would return `false` for every file, and
        // EVERY document would be reported unguarded. A loud false
        // positive on one platform -- which is the direction to be wrong
        // in, but still a red job nobody can reproduce locally.
        // `health::runaway`'s guard records observing this exact hazard on
        // the `platform (windows-latest)` job.
        let guards: Vec<String> = [
            include_str!("query.rs"),
            include_str!("stats/query.rs"),
            include_str!("stats/tree.rs"),
            include_str!("stats/board.rs"),
            include_str!("../poll.rs"),
        ]
        .iter()
        .map(|s| s.replace("\r\n", "\n"))
        .collect();

        let mut unguarded = Vec::new();
        for (file, name, _) in &docs {
            if NO_SHAPE_GUARD.iter().any(|(n, _)| n == name) {
                continue;
            }
            // A guard is a `fn` in a test module whose BODY names this
            // document. Scoped to a function body rather than searched
            // file-wide, because every one of these files defines its own
            // documents and would therefore "name" all of them.
            let guarded = guards.iter().any(|src| {
                let Some((_, tests)) = src.split_once("\n#[cfg(test)]") else {
                    return false;
                };
                tests.split("    fn ").skip(1).any(|body| {
                    // Only the portion before the next test attribute, so a
                    // mention in the NEXT test's doc comment does not count
                    // for this one.
                    let body = body.split("\n    #[").next().unwrap_or(body);
                    let uses = body
                        .lines()
                        .filter(|l| !l.trim_start().starts_with("//"))
                        .any(|l| l.contains(name.as_str()));
                    uses && (body.contains("contains(") || body.contains("split_once("))
                })
            });
            if !guarded {
                unguarded.push(format!("{file}'s {name}"));
            }
        }

        // Guards the guard: a broken derivation would report nothing to
        // check, and a broken guard-detector would report everything.
        assert!(
            docs.len() >= 10,
            "only {} document(s) derived; the scan is broken, not the documents",
            docs.len()
        );
        assert!(
            unguarded.is_empty(),
            "these GraphQL documents have no test asserting their shape:\n  {}\n\n\
             No mapper test can see a missing field: they feed `json!` literals that \
             supply the field themselves, so the document can stop asking for it and \
             the suite stays green (#847). What that costs is a PARTIAL answer \
             presented as a complete one -- drop `issueCount` from a window and \
             `map_cycle_trend` reports a 100-PR sample of a busy week as the whole \
             week. Add a test asserting the fields its reader reads, deriving them \
             with `fields_read_by` where the reader is a `map_*` function, plus a \
             nesting assertion where a count has to sit beside the nodes it \
             qualifies. If the document genuinely needs none, add it to \
             NO_SHAPE_GUARD with a reason (#854).",
            unguarded.join("\n  ")
        );
    }

    /// #1407: the list query asks for what `ready_at` reads, and ONLY the
    /// ready-for-review event.
    ///
    /// The mapper tests feed `json!` literals that supply `timelineItems`
    /// themselves, so dropping it from the document would leave them green
    /// while every row read as UNKNOWN -- the #847 hole. And the mapper
    /// takes the LAST node as "when it became ready", which is only true
    /// while the connection is filtered to `READY_FOR_REVIEW_EVENT` and
    /// paged `last: 1`: widen `itemTypes` and the last node could be a
    /// label or a comment, dating the PR from that instead.
    #[test]
    fn the_list_query_asks_for_the_ready_for_review_event_only() {
        let doc: String = PRS_QUERY
            .replace("\r\n", "\n")
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let nodes = doc
            .split_once("... on PullRequest {")
            .expect("the node selection")
            .1;
        for f in fields_read_by(&["ready_at"]) {
            assert!(
                nodes.contains(&f),
                "`ready_at` reads `{f}` and PRS_QUERY does not select it; every \
                 row would read as age unknown"
            );
        }
        let args = nodes
            .split_once("timelineItems(")
            .expect("timelineItems is asked for")
            .1
            .split_once(')')
            .expect("the connection closes")
            .0;
        assert!(
            args.contains("itemTypes: [READY_FOR_REVIEW_EVENT]"),
            "`timelineItems({args})` must be filtered to READY_FOR_REVIEW_EVENT \
             alone: the mapper reads the last node as the ready time"
        );
        assert!(
            args.contains("last: 1"),
            "`timelineItems({args})` must take the LATEST event: a pull request \
             marked ready, drafted again and re-marked became ready the second time"
        );
        assert!(
            nodes.contains("... on ReadyForReviewEvent { createdAt }"),
            "the event's own `createdAt` must be selected on the event fragment"
        );
    }

    /// #1440: the merged-PR lookup asks for every field the strict rule
    /// reads, and binds each branch name as a VARIABLE.
    ///
    /// `worktrees::github::map_merged_heads` reads `number`, `headRefOid`
    /// and `baseRefName` per node and `defaultBranchRef { name }` once. A
    /// mapper test feeds `json!` literals that supply those itself, so a
    /// document that stopped asking for `headRefOid` would leave it green
    /// while no row could ever qualify -- or, worse, one that stopped
    /// filtering to `MERGED` would hand an open PR's head to a rule that
    /// assumes it merged.
    #[test]
    fn the_merged_heads_query_asks_for_what_the_rule_reads() {
        let q = merged_heads_query(3);
        for f in [
            "defaultBranchRef { name }",
            "number",
            "headRefOid",
            "baseRefName",
            "rateLimit { cost remaining resetAt }",
        ] {
            assert!(q.contains(f), "merged_heads_query must select `{f}`");
        }
        // One alias per name, each filtered to MERGED and bound to its
        // own variable rather than to spliced text.
        assert_eq!(q.matches("states: MERGED").count(), 3);
        for i in 0..3 {
            assert!(q.contains(&format!("$h{i}: String!")), "variable h{i}");
            assert!(
                q.contains(&format!("h{i}: pullRequests(headRefName: $h{i},")),
                "alias h{i} must read its own variable"
            );
        }
        assert!(
            q.split_once("repository(owner: $owner, name: $name)")
                .is_some(),
            "the aliases must sit inside the repository they are about"
        );
    }
}
