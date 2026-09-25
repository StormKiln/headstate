//! Mapping from the raw GraphQL JSON to typed `PullRequest`s.

use super::model::{
    CheckRun, CiState, CycleTrend, HistoryPoint, Label, MergeState, MergeStateStatus, MergedDetail,
    MergedPr, PrComment, PrDetail, PullRequest, RepoCount, ReviewState, ReviewThread,
    ReviewerVerdict,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

fn ts(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    v[key].as_str()?.parse::<DateTime<Utc>>().ok()
}

fn ci_state(node: &Value) -> CiState {
    let rollup = &node["commits"]["nodes"][0]["commit"]["statusCheckRollup"];
    let state = rollup["state"].as_str();

    // A FAILURE the rollup reports may already be re-running.
    //
    // `statusCheckRollup.state` ranks FAILURE above PENDING, so a pull
    // request whose checks restarted after a fix keeps reading as
    // broken -- it sits in Needs Attention through the whole rerun,
    // which is exactly when the author has already acted. Per-check
    // `status` is what separates the two: a check that is QUEUED or
    // IN_PROGRESS has no verdict yet, whatever the collapsed state
    // says.
    //
    // Only applied to FAILURE. A rollup that says SUCCESS while
    // something is still running is not a state to second-guess, and
    // reporting pending there would make a mergeable pull request look
    // unready.
    if matches!(state, Some("FAILURE") | Some("ERROR")) && any_check_running(rollup) {
        return CiState::Pending;
    }

    match state {
        Some("SUCCESS") => CiState::Success,
        Some("FAILURE") | Some("ERROR") => CiState::Failure,
        Some("PENDING") | Some("EXPECTED") => CiState::Pending,
        // No rollup at all means the repo runs no checks on this PR.
        _ => CiState::None,
    }
}

/// Whether any individual check has not finished.
///
/// `status` is the check's LIFECYCLE (QUEUED / IN_PROGRESS / COMPLETED),
/// which is a different axis from `conclusion` (SUCCESS / FAILURE). A
/// check with no conclusion yet is one whose verdict is still coming.
fn any_check_running(rollup: &Value) -> bool {
    rollup["contexts"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter().any(|c| {
                matches!(
                    c["status"].as_str(),
                    Some("QUEUED") | Some("IN_PROGRESS") | Some("WAITING") | Some("PENDING")
                )
            })
        })
        .unwrap_or(false)
}

/// One check's outcome, normalised.
///
/// CheckRun reports `conclusion` (SUCCESS/FAILURE/...) while
/// StatusContext reports `state` (SUCCESS/PENDING/...). Both shapes reach
/// here, and a null conclusion means the run has not finished.
fn check_state(node: &Value) -> String {
    let raw = node["conclusion"]
        .as_str()
        .or_else(|| node["state"].as_str())
        .unwrap_or("PENDING");
    match raw {
        "SUCCESS" => "success",
        "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "STARTUP_FAILURE" => "failure",
        "PENDING" | "IN_PROGRESS" | "QUEUED" | "WAITING" | "EXPECTED" => "pending",
        "NEUTRAL" | "SKIPPED" => "skipped",
        other => other,
    }
    .to_string()
}

/// The detail payload for one pull request.
/// The per-check list for one pull request.
///
/// Extracted so the `run_id` rules are testable without building a whole
/// `PrDetail`: which checks can be re-run is the decision the UI gates
/// its button on, and it must be exercised directly.
pub fn map_detail_checks(pr: &Value) -> Vec<CheckRun> {
    let empty = vec![];
    pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|c| CheckRun {
            // CheckRun uses `name`, StatusContext uses `context`.
            name: c["name"]
                .as_str()
                .or_else(|| c["context"].as_str())
                .unwrap_or("check")
                .to_string(),
            state: check_state(c),
            url: c["detailsUrl"]
                .as_str()
                .or_else(|| c["targetUrl"].as_str())
                .unwrap_or_default()
                .to_string(),
            // Absent for a StatusContext and for check runs from apps
            // that are not Actions. None means "cannot re-run this",
            // which the UI reads rather than guessing.
            run_id: c["checkSuite"]["workflowRun"]["databaseId"].as_u64(),
        })
        .collect()
}

pub fn map_detail(v: &Value, repo: &str) -> PrDetail {
    let pr = &v["repository"]["pullRequest"];
    let empty = vec![];

    let checks = map_detail_checks(pr);
    // Bound ahead of the struct literal like `checks`, so the total below
    // can be defaulted to this length without mapping the threads twice.
    let review_threads = map_review_threads(pr);

    let comments = pr["comments"]["nodes"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|c| PrComment {
            author: c["author"]["login"].as_str().unwrap_or("ghost").to_string(),
            created_at: c["createdAt"].as_str().unwrap_or_default().to_string(),
            body: c["body"].as_str().unwrap_or_default().to_string(),
        })
        .collect();

    // #1457. `ready_at` is the list row's own function, so the header and
    // the row it was opened from cannot date the pull request differently.
    // An unreadable `createdAt` leaves both absent rather than failing the
    // whole view the way `map_node` drops a row: a detail payload is one
    // pull request the user asked for, and the rest of it is still true.
    let is_draft = pr["isDraft"].as_bool().unwrap_or(false);
    let created_at = ts(pr, "createdAt");

    PrDetail {
        id: pr["id"].as_str().unwrap_or_default().to_string(),
        number: pr["number"].as_u64().unwrap_or(0),
        title: pr["title"].as_str().unwrap_or_default().to_string(),
        url: pr["url"].as_str().unwrap_or_default().to_string(),
        state: pr["state"].as_str().unwrap_or("OPEN").to_lowercase(),
        is_draft,
        ready_at: created_at.and_then(|c| ready_at(pr, c, is_draft)),
        created_at,
        last_commit_at: ts(&pr["commits"]["nodes"][0]["commit"], "committedDate"),
        body: pr["body"].as_str().unwrap_or_default().to_string(),
        author: pr["author"]["login"]
            .as_str()
            .unwrap_or("ghost")
            .to_string(),
        repo: repo.to_string(),
        head_ref: pr["headRefName"].as_str().unwrap_or_default().to_string(),
        head_oid: pr["headRefOid"].as_str().unwrap_or_default().to_string(),
        head_ref_id: pr["headRef"]["id"].as_str().map(str::to_string),
        base_ref: pr["baseRefName"].as_str().unwrap_or_default().to_string(),
        merge_status: merge_status(pr),
        review: review_state(pr),
        merge_queue_enabled: pr["isMergeQueueEnabled"].as_bool().unwrap_or(false),
        in_merge_queue: in_merge_queue(pr),
        latest_reviews: pr["latestReviews"]["nodes"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            // A review whose author GitHub cannot name is useless for
            // matching against the viewer's login, so it is dropped
            // rather than mapped to a placeholder that could collide.
            .filter_map(|r| {
                Some(ReviewerVerdict {
                    author: r["author"]["login"].as_str()?.to_string(),
                    state: r["state"].as_str()?.to_string(),
                })
            })
            .collect(),
        additions: pr["additions"].as_u64().unwrap_or(0),
        deletions: pr["deletions"].as_u64().unwrap_or(0),
        changed_files: pr["changedFiles"].as_u64().unwrap_or(0),
        unresolved_threads: unresolved_threads(pr),
        comment_count: pr["comments"]["totalCount"].as_u64().unwrap_or(0),
        comments,
        // Read BEFORE `review_threads` is moved, and defaulted to the
        // number of threads that arrived rather than to 0, for the reason
        // `checks_total` below gives: absent `totalCount` (a payload
        // cached before #802 added the field, a partial response that
        // dropped it) must read as "nothing missing", where a 0 against a
        // non-empty list would render a nonsense "Showing 7 of 0".
        review_threads_total: pr["reviewThreads"]["totalCount"]
            .as_u64()
            .unwrap_or(review_threads.len() as u64),
        review_threads,
        // Read BEFORE `checks` is moved, and defaulted to the number of
        // checks we actually have rather than to 0: absent `totalCount`
        // (an old cached payload, a partial response that dropped the
        // field) must read as "nothing missing", and a 0 against a
        // non-empty list would instead be a nonsense "showing 37 of 0".
        checks_total: pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]
            ["totalCount"]
            .as_u64()
            .unwrap_or(checks.len() as u64),
        checks,
    }
}

/// Reviewers whose verdict is still outstanding.
///
/// Users only: `requestedReviewer` is a union that also admits Team and
/// Mannequin, and a reviewer with no login is dropped rather than given
/// an invented name.
fn requested_reviewers(node: &Value) -> Vec<String> {
    node["reviewRequests"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| Some(r["requestedReviewer"]["login"].as_str()?.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// How many entries this connection has, in terms of the MAPPED list.
///
/// # Why this is not just `totalCount`
///
/// Two different reductions are at work and conflating them invents a
/// truncation that does not exist:
///
/// - The WINDOW cuts the connection. `totalCount` minus the number of
///   nodes that arrived is the real shortfall, and the only thing the UI
///   should report.
/// - The MAPPER then drops nodes it cannot name -- a review with no
///   author, a `requestedReviewer` that is a Team or Mannequin rather
///   than a User. Those entries DID arrive; they are simply not people
///   this app can name.
///
/// Returning a bare `totalCount` against the mapped length adds the two
/// together, so a Team reviewer sitting inside the window reads as a
/// person the window hid. MEASURED on live data: 1 of 25 open
/// kubernetes/kubernetes pull requests has exactly that shape
/// (`totalCount` 3, three nodes, two of them Users), and it would have
/// rendered a truncation warning on a pull request whose reviewer list
/// arrived whole.
///
/// So the shortfall is measured against the RAW nodes and then added to
/// the mapped length: `mapped + (totalCount - arrived)`. A dropped Team
/// shrinks the list, as it should, without becoming a missing person.
///
/// The result is never below `mapped` and never 0 against a non-empty
/// list. An absent `totalCount` -- a snapshot cached before #1089 added
/// the field, a partial response that dropped it -- reads as "nothing
/// missing". A 0 there would render "3 of 0", which is the
/// absent-is-not-zero defect (#846) wearing a truncation notice;
/// `review_threads_total` and `checks_total` state the same rule.
fn connection_total(conn: &Value, mapped: usize) -> u64 {
    let arrived = conn["nodes"].as_array().map_or(0, Vec::len);
    // Saturating: `totalCount` can legitimately lag the nodes (the count
    // and the page come from slightly different moments), and that is
    // not a negative shortfall. The same care `ReviewThreads` takes.
    let unseen = conn["totalCount"]
        .as_u64()
        .map_or(0, |t| t.saturating_sub(arrived as u64));
    mapped as u64 + unseen
}

/// The logins in a `{ nodes: [{ login }] }` connection.
fn logins(conn: &Value) -> Vec<String> {
    conn["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|n| Some(n["login"].as_str()?.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Every reviewer's latest verdict.
///
/// Shared by the list and detail mappers so the row and the view it
/// opens cannot disagree about who reviewed.
fn latest_reviews(node: &Value) -> Vec<ReviewerVerdict> {
    node["latestReviews"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some(ReviewerVerdict {
                        author: r["author"]["login"].as_str()?.to_string(),
                        state: r["state"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Is this pull request genuinely sitting in the merge queue?
///
/// `isInMergeQueue` stays TRUE for an entry the queue has REJECTED, so
/// reading it alone renders a stuck pull request as calmly queued. The
/// entry's own state is what separates them.
///
/// Shared by the list and detail mappers deliberately: they must agree,
/// or the row and the button it opens would disagree about whether the
/// pull request is queued.
fn in_merge_queue(node: &Value) -> bool {
    node["isInMergeQueue"].as_bool().unwrap_or(false)
        && !matches!(
            node["mergeQueueEntry"]["state"].as_str(),
            Some("UNMERGEABLE") | Some("LOCKED")
        )
}

/// Open review conversations on the current code.
///
/// Both filters matter: a resolved thread needs no action, and an outdated
/// one hangs off a line that has since changed, so counting either would
/// nag the author about work already finished.
///
/// Counted over the threads that ARRIVED, so on a pull request past the
/// query's page this is a floor rather than the true number (#802). Left
/// that way on purpose: the alternative is inferring a count from
/// `reviewThreads.totalCount`, which counts resolved and outdated threads
/// too and would therefore OVERSTATE the open ones -- a header nagging
/// about conversations that are already settled. A floor plus a visible
/// "showing N of M" on the list beneath it is honest; a guessed total is
/// not.
fn unresolved_threads(node: &Value) -> u64 {
    node["reviewThreads"]["nodes"]
        .as_array()
        .map(|threads| {
            threads
                .iter()
                .filter(|t| {
                    !t["isResolved"].as_bool().unwrap_or(false)
                        && !t["isOutdated"].as_bool().unwrap_or(false)
                })
                .count() as u64
        })
        .unwrap_or(0)
}

/// The review conversations in full, for the detail view.
///
/// `unresolved_threads` above counts the same node and stays the source of
/// the NUMBER. This maps the threads themselves, resolved and outdated
/// included: the detail view shows resolved ones collapsed so they remain
/// findable, and an outdated thread can still hold an unanswered question.
///
/// NOT PAGINATED, deliberately -- and this is the one decision in #802
/// worth justifying, because the issue asks for a cursor loop and the
/// checks one next door is the obvious precedent.
///
/// The query asks for `first: 100`, the connection maximum, and MEASURED
/// against the live API that is free: the whole detail query still costs
/// 1 point, the latency is unchanged (557ms at 100 vs 574ms at 20, inside
/// the noise), and 100 threads x 10 comments is 1,000 nodes against
/// GitHub's 500,000 node ceiling. So the honest count from `totalCount`
/// plus the largest free page is the entire fix for every pull request
/// that exists, and a cursor loop would buy the 101st thread only.
///
/// REJECTED: a bounded cursor loop like `append_remaining_checks`. The
/// reason is the reason #790 CUT that loop's budget from 20 pages to 3 --
/// serial cursor pages are a latency budget, not a completeness budget.
/// Every page is another POST on the one strictly-serial chain in the
/// app, behind a blocked user gesture, inside a 30s command ceiling that
/// the checks loop is already spending up to 4 POSTs of. Adding more
/// serial pages to that chain needs to buy something, and here it buys
/// nothing measurable: sampled live across rust-lang/rust,
/// kubernetes/kubernetes and facebook/react, the busiest recently-merged
/// pull requests carry 0-10 review threads, an order of magnitude under
/// the page. Checks are different in kind and that is why they get a loop
/// -- a rollup legitimately runs to the hundreds (412 contexts in the
/// reports behind #790), so for checks the first page genuinely is not
/// enough.
///
/// What makes declining safe is that it is no longer SILENT: above 100
/// threads `review_threads_total` exceeds what arrived and the view says
/// so, which is the property the 20-thread window lacked and the actual
/// defect in #802. A pull request with 150 threads shows 100 and admits
/// it, rather than showing 20 and looking complete. If such a pull
/// request is ever actually observed, the loop is a small change on top
/// of this one and `review_threads_total` is what it would need anyway.
fn map_review_threads(node: &Value) -> Vec<ReviewThread> {
    let empty = vec![];
    node["reviewThreads"]["nodes"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|t| ReviewThread {
            id: t["id"].as_str().unwrap_or_default().to_string(),
            is_resolved: t["isResolved"].as_bool().unwrap_or(false),
            is_outdated: t["isOutdated"].as_bool().unwrap_or(false),
            path: t["path"].as_str().unwrap_or_default().to_string(),
            // `as_u64` already yields None for JSON null, which is what
            // GitHub sends once the anchor line is gone. No default: a
            // missing line must stay missing rather than become line 0.
            line: t["line"].as_u64(),
            // Default FALSE on a missing field, so a response we could not
            // read hides the button rather than offering one that 403s.
            viewer_can_reply: t["viewerCanReply"].as_bool().unwrap_or(false),
            viewer_can_resolve: t["viewerCanResolve"].as_bool().unwrap_or(false),
            viewer_can_unresolve: t["viewerCanUnresolve"].as_bool().unwrap_or(false),
            comments: t["comments"]["nodes"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .map(|c| PrComment {
                    author: c["author"]["login"].as_str().unwrap_or("ghost").to_string(),
                    created_at: c["createdAt"].as_str().unwrap_or_default().to_string(),
                    body: c["body"].as_str().unwrap_or_default().to_string(),
                })
                .collect(),
            comment_count: t["comments"]["totalCount"].as_u64().unwrap_or(0),
        })
        .collect()
}

/// GitHub's merge-readiness summary.
///
/// Unrecognised and absent values both become `Unknown` rather than
/// guessing: a merge button must never be enabled on a state we do not
/// understand.
fn merge_status(node: &Value) -> MergeStateStatus {
    match node["mergeStateStatus"].as_str() {
        Some("CLEAN") => MergeStateStatus::Clean,
        Some("DIRTY") => MergeStateStatus::Dirty,
        Some("BLOCKED") => MergeStateStatus::Blocked,
        Some("UNSTABLE") => MergeStateStatus::Unstable,
        Some("BEHIND") => MergeStateStatus::Behind,
        Some("DRAFT") => MergeStateStatus::Draft,
        _ => MergeStateStatus::Unknown,
    }
}

fn merge_state(node: &Value) -> MergeState {
    match node["mergeable"].as_str() {
        Some("MERGEABLE") => MergeState::Mergeable,
        Some("CONFLICTING") => MergeState::Conflicted,
        // UNKNOWN and anything unrecognised. Never Conflicted: GitHub
        // reports UNKNOWN while it computes, and a false conflict warning
        // would fire on every push.
        _ => MergeState::Checking,
    }
}

fn review_state(node: &Value) -> ReviewState {
    match node["reviewDecision"].as_str() {
        Some("APPROVED") => ReviewState::Approved,
        Some("CHANGES_REQUESTED") => ReviewState::ChangesRequested,
        Some("REVIEW_REQUIRED") => ReviewState::ReviewRequired,
        _ => ReviewState::None,
    }
}

fn labels(node: &Value) -> Vec<Label> {
    node["labels"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    Some(Label {
                        name: l["name"].as_str()?.to_string(),
                        color: l["color"].as_str().unwrap_or("cccccc").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// When the pull request became ready for review. See `PullRequest::ready_at`.
///
/// The query asks for `last: 1` of `READY_FOR_REVIEW_EVENT` only, so the
/// one node, if any, is the latest time it left draft.
///
/// Three different answers, kept apart:
/// - an EMPTY list is "never a draft", so `created_at` -- the same moment;
/// - a MISSING connection is unknown, not empty: reading it as
///   never-drafted would date a pull request marked ready an hour ago
///   from the week it spent in draft;
/// - an event whose time does not parse is unknown, never "now".
fn ready_at(node: &Value, created_at: DateTime<Utc>, is_draft: bool) -> Option<DateTime<Utc>> {
    if is_draft {
        return None;
    }
    match node["timelineItems"]["nodes"].as_array()?.last() {
        None => Some(created_at),
        Some(event) => ts(event, "createdAt"),
    }
}

fn map_node(node: &Value) -> Option<PullRequest> {
    // Bound ahead of the struct literal so each total can default to the
    // length of the list it describes without mapping anything twice --
    // the shape `map_detail` already uses for `review_threads_total`.
    let requested_reviewers = requested_reviewers(node);
    let assignees = logins(&node["assignees"]);
    let latest_reviews = latest_reviews(node);
    let labels = labels(node);
    let created_at = ts(node, "createdAt")?;
    let is_draft = node["isDraft"].as_bool().unwrap_or(false);

    Some(PullRequest {
        id: node["id"].as_str().unwrap_or_default().to_string(),
        number: node["number"].as_u64()?,
        title: node["title"].as_str()?.to_string(),
        url: node["url"].as_str()?.to_string(),
        repo: node["repository"]["nameWithOwner"].as_str()?.to_string(),
        author: node["author"]["login"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        is_draft,
        head_ref: node["headRefName"].as_str().unwrap_or_default().to_string(),
        head_oid: node["headRefOid"].as_str().unwrap_or_default().to_string(),
        head_ref_id: node["headRef"]["id"].as_str().map(str::to_string),
        base_ref: node["baseRefName"].as_str().unwrap_or_default().to_string(),
        ready_at: ready_at(node, created_at, is_draft),
        created_at,
        updated_at: ts(node, "updatedAt")?,
        ci: ci_state(node),
        merge: merge_state(node),
        merge_status: merge_status(node),
        review: review_state(node),
        // Queued means WAITING, not merely present. `isInMergeQueue`
        // stays true for an entry the queue has rejected -- state
        // UNMERGEABLE -- so a declined pull request reported as calmly
        // queued and got the amber merge-queue icon while actually
        // being stuck.
        //
        // An absent entry falls back to the flag: `mergeQueueEntry` is
        // null on repositories without a merge queue, where
        // `isInMergeQueue` is false anyway.
        in_merge_queue: in_merge_queue(node),
        requested_reviewers_total: connection_total(
            &node["reviewRequests"],
            requested_reviewers.len(),
        ),
        assignees_total: connection_total(&node["assignees"], assignees.len()),
        latest_reviews_total: connection_total(&node["latestReviews"], latest_reviews.len()),
        labels_total: connection_total(&node["labels"], labels.len()),
        requested_reviewers,
        assignees,
        latest_reviews,
        labels,
        comment_count: node["totalCommentsCount"].as_u64().unwrap_or(0),
        unresolved_threads: unresolved_threads(node),
    })
}

/// Map a search response. Note the response passed here is Octocrab's
/// already-unwrapped `data` object, so `search` is at the top level.
/// Nodes that fail to map are skipped rather than failing the whole poll:
/// one malformed PR should not blank the list.
pub fn map_search(v: &Value) -> Vec<PullRequest> {
    map_list(v, "authored")
}

/// One named search alias from the response.
pub fn map_list(v: &Value, alias: &str) -> Vec<PullRequest> {
    v[alias]["nodes"]
        .as_array()
        .map(|a| {
            let mapped: Vec<PullRequest> = a.iter().filter_map(map_node).collect();
            // Dropping a node is SILENT and it is how a refused field
            // empties a list: GitHub nulls the fields it could not
            // compute, `map_node` requires title/url/repository, and
            // `filter_map` discards whatever is left. With 79 fields
            // refused, that can be the whole page -- reported as "No
            // open pull requests", which is a confident wrong answer.
            //
            // Counts only, never titles or repository names.
            if mapped.len() < a.len() {
                log::warn!(
                    "dropped {} of {} pull requests: GitHub returned them without \
                     the fields needed to render one",
                    a.len() - mapped.len(),
                    a.len()
                );
            }
            mapped
        })
        .unwrap_or_default()
}

/// How many PRs GitHub says match, which may exceed how many it returned.
///
/// `PRS_QUERY` asks for `first: 100` and already selects `issueCount`, but
/// nothing read it -- so an account with 137 open PRs saw a list, a
/// sidebar, and a priorities strip all confidently reporting 100, with the
/// remaining 37 invisible. Worst case the strip, whose entire job is never
/// to have a false negative, renders "Nothing blocked on you" while a
/// conflicted PR sits at rank 118.
/// The rate-limit budget the response already carried.
///
/// `PRS_QUERY` has always selected `rateLimit`, and nothing read it -- so
/// when the budget did run out, the user got a generic failure and could
/// not tell it from a network problem or an expired token, nor learn how
/// long to wait, even though the reset time was in a response already
/// received.
pub fn map_rate_limit(v: &Value) -> Option<(u64, String)> {
    let remaining = v["rateLimit"]["remaining"].as_u64()?;
    let reset = v["rateLimit"]["resetAt"].as_str().unwrap_or("").to_string();
    Some((remaining, reset))
}

/// The authenticated user's login.
///
/// The app ran entirely on `@me` search qualifiers and never learned who
/// it was, which was fine while everything was read-only. It stops being
/// fine once reviews exist: GitHub refuses self-approval, and the UI
/// should say so before the click rather than surface a GraphQL refusal
/// after it.
///
/// Returns None rather than a guess if the field is absent, so a caller
/// gets "we do not know" instead of a wrong answer about authorship.
pub fn map_viewer(v: &Value) -> Option<String> {
    v["viewer"]["login"].as_str().map(str::to_string)
}

pub fn map_total(v: &Value) -> u64 {
    v["authored"]["issueCount"].as_u64().unwrap_or(0)
}

/// Day-bucket aliases into an ascending-by-date series.
///
/// Aliases are emitted newest-first (`m0` is today); the chart plots time
/// left to right, so the series is reversed here rather than in the view.
/// A missing alias maps to 0: GitHub omits nothing today, but a partial
/// response must not panic a dashboard.
pub fn map_history(v: &Value, days: i64, now: DateTime<Utc>) -> Vec<HistoryPoint> {
    let mut pts: Vec<HistoryPoint> = (0..days)
        .map(|i| HistoryPoint {
            date: (now - Duration::days(i)).format("%Y-%m-%d").to_string(),
            merged: v[format!("m{i}")]["issueCount"].as_u64().unwrap_or(0),
            opened: v[format!("o{i}")]["issueCount"].as_u64().unwrap_or(0),
        })
        .collect();
    pts.reverse();
    pts
}

/// Totals over the merged-PR sample.
///
/// Cycle times are collected only for nodes carrying both timestamps, but
/// such nodes still count toward volume totals -- the PR did merge; only
/// its duration is unknown. The vector is sorted so percentile() can index
/// it without re-sorting per call.
pub fn map_merged_detail(v: &Value) -> MergedDetail {
    let mut d = MergedDetail::default();
    let mut repos: std::collections::HashMap<String, u64> = Default::default();
    let mut merged_prs: Vec<MergedPr> = Vec::new();
    let empty = vec![];
    let nodes = v["merged"]["nodes"].as_array().unwrap_or(&empty);
    for n in nodes {
        d.sample_size += 1;
        let size = n["additions"].as_u64().unwrap_or(0) + n["deletions"].as_u64().unwrap_or(0);
        d.pr_sizes.push(size);
        d.additions += n["additions"].as_u64().unwrap_or(0);
        d.deletions += n["deletions"].as_u64().unwrap_or(0);
        d.changed_files += n["changedFiles"].as_u64().unwrap_or(0);
        d.comment_count += n["comments"]["totalCount"].as_u64().unwrap_or(0);
        if let Some(r) = n["repository"]["nameWithOwner"].as_str() {
            *repos.entry(r.to_string()).or_insert(0) += 1;
        }
        let mut hours_for_pr = 0.0;
        if let (Some(c), Some(m)) = (n["createdAt"].as_str(), n["mergedAt"].as_str()) {
            if let (Ok(c), Ok(m)) = (
                DateTime::parse_from_rfc3339(c),
                DateTime::parse_from_rfc3339(m),
            ) {
                let hours = (m - c).num_seconds() as f64 / 3600.0;
                if hours >= 0.0 {
                    d.cycle_time_hours.push(hours);
                    hours_for_pr = hours;
                }
            }
        }
        if let Some(num) = n["number"].as_u64() {
            merged_prs.push(MergedPr {
                number: num,
                title: n["title"].as_str().unwrap_or_default().to_string(),
                url: n["url"].as_str().unwrap_or_default().to_string(),
                repo: n["repository"]["nameWithOwner"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                cycle_time_hours: hours_for_pr,
                size,
            });
        }
    }
    d.cycle_time_hours.sort_by(|a, b| a.total_cmp(b));
    d.pr_sizes.sort_unstable();

    // The outliers the scalar figures describe, so a striking number is a
    // link rather than a dead end. Ties broken by number for a stable
    // order between refreshes.
    let mut by_time = merged_prs.clone();
    by_time.sort_by(|a, b| {
        // total_cmp, not partial_cmp().unwrap(). A panic inside a Tauri
        // command aborts the WHOLE APP, and the invariant that keeps NaN
        // out of cycle_time_hours lives in a guard forty lines away --
        // implicit, undocumented, and one edit from turning a stats
        // refresh into a crash. total_cmp is total over floats and free.
        b.cycle_time_hours
            .total_cmp(&a.cycle_time_hours)
            .then(a.number.cmp(&b.number))
    });
    d.slowest = by_time.into_iter().take(5).collect();

    let mut by_size = merged_prs;
    by_size.sort_by(|a, b| b.size.cmp(&a.size).then(a.number.cmp(&b.number)));
    d.largest = by_size.into_iter().take(5).collect();
    let mut rc: Vec<RepoCount> = repos
        .into_iter()
        .map(|(repo, merged)| RepoCount { repo, merged })
        .collect();
    // Ties broken by name so the table order is stable between refreshes.
    rc.sort_by(|a, b| b.merged.cmp(&a.merged).then(a.repo.cmp(&b.repo)));
    d.repo_counts = rc;
    d
}

/// Median cycle time in hours for one window's nodes.
///
/// Nearest rank, matching `percentile` on the frontend: `ceil(n*p) - 1`
/// zero-indexed. Nodes missing either timestamp are skipped.
fn median_hours(nodes: &Value) -> f64 {
    let empty = vec![];
    let mut hours: Vec<f64> = nodes
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|n| {
            let c = DateTime::parse_from_rfc3339(n["createdAt"].as_str()?).ok()?;
            let m = DateTime::parse_from_rfc3339(n["mergedAt"].as_str()?).ok()?;
            let h = (m - c).num_seconds() as f64 / 3600.0;
            (h >= 0.0).then_some(h)
        })
        .collect();
    if hours.is_empty() {
        return 0.0;
    }
    hours.sort_by(|a, b| a.total_cmp(b));
    let idx = ((hours.len() as f64 * 0.5).ceil() as usize).saturating_sub(1);
    hours[idx.min(hours.len() - 1)]
}

pub fn map_cycle_trend(v: &Value) -> CycleTrend {
    let cur_n = v["current"]["issueCount"].as_u64().unwrap_or(0);
    let prev_n = v["previous"]["issueCount"].as_u64().unwrap_or(0);
    let cur_len = v["current"]["nodes"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0) as u64;
    let prev_len = v["previous"]["nodes"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0) as u64;
    CycleTrend {
        current_hours: median_hours(&v["current"]["nodes"]),
        previous_hours: median_hours(&v["previous"]["nodes"]),
        current_count: cur_n,
        previous_count: prev_n,
        // GitHub returns at most 100 nodes per window; above that the
        // medians describe a sample of the week, not the week.
        sampled: cur_n > cur_len || prev_n > prev_len,
    }
}

#[cfg(test)]
mod tests {
    /// Reported: a pull request labelled `auto-merge-declined` showed
    /// the amber "In merge queue" icon.
    ///
    /// `isInMergeQueue` stays TRUE for an entry the queue has rejected,
    /// so the app reported it as calmly queued while it was actually
    /// stuck. Queued must mean WAITING, not merely present.
    #[test]
    fn a_rejected_queue_entry_does_not_count_as_queued() {
        for state in ["UNMERGEABLE", "LOCKED"] {
            let v = json!({"authored": {"nodes": [{
                "number": 1, "title": "t", "url": "u",
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
                "repository": {"nameWithOwner": "octocat/hello-world"},
                "isInMergeQueue": true,
                "mergeQueueEntry": {"state": state}
            }]}});
            assert!(
                !map_list(&v, "authored")[0].in_merge_queue,
                "{state} is not waiting in a queue"
            );
        }
    }

    #[test]
    fn a_waiting_queue_entry_still_counts_as_queued() {
        for state in ["QUEUED", "AWAITING_CHECKS", "MERGEABLE"] {
            let v = json!({"authored": {"nodes": [{
                "number": 1, "title": "t", "url": "u",
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
                "repository": {"nameWithOwner": "octocat/hello-world"},
                "isInMergeQueue": true,
                "mergeQueueEntry": {"state": state}
            }]}});
            assert!(
                map_list(&v, "authored")[0].in_merge_queue,
                "{state} is waiting"
            );
        }
    }

    /// `mergeQueueEntry` is null on a repository with no merge queue,
    /// where `isInMergeQueue` is false anyway -- the flag must still be
    /// what decides, not the absent entry.
    #[test]
    fn an_absent_entry_falls_back_to_the_flag() {
        let node = |q: bool| {
            json!({"authored": {"nodes": [{
                "number": 1, "title": "t", "url": "u",
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
                "repository": {"nameWithOwner": "octocat/hello-world"},
                "isInMergeQueue": q
            }]}})
        };
        let queued = node(true);
        assert!(map_list(&queued, "authored")[0].in_merge_queue);
        let not = node(false);
        assert!(!map_list(&not, "authored")[0].in_merge_queue);
    }

    /// The workflow run id is what makes re-running possible, and it
    /// is OPTIONAL by design: a plain commit status and a check run from
    /// a non-Actions app both have no workflow run. None must mean
    /// "cannot re-run this" rather than defaulting to a wrong id.
    #[test]
    fn check_runs_carry_their_workflow_run_when_there_is_one() {
        let v = json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {
            "state": "FAILURE",
            "contexts": {"nodes": [
                {"name": "lint", "conclusion": "FAILURE", "detailsUrl": "https://x/1",
                 "checkSuite": {"workflowRun": {"databaseId": 12345}}},
                // A StatusContext: no checkSuite at all.
                {"context": "legacy/ci", "state": "FAILURE", "targetUrl": "https://x/2"}
            ]}
        }}}]}});
        let checks = map_detail_checks(&v);
        assert_eq!(checks[0].run_id, Some(12345));
        assert_eq!(checks[1].run_id, None, "a status context cannot be re-run");
    }

    /// An Actions check whose suite has no workflow run -- which happens
    /// for check runs created by other apps -- must also come back None
    /// rather than panicking on the missing field.
    #[test]
    fn a_check_without_a_workflow_run_is_not_rerunnable() {
        let v = json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {
            "contexts": {"nodes": [
                {"name": "third-party", "conclusion": "FAILURE",
                 "checkSuite": {"workflowRun": null}}
            ]}
        }}}]}});
        assert_eq!(map_detail_checks(&v)[0].run_id, None);
    }

    /// The login must come back verbatim, and its absence must be None
    /// rather than an empty string -- "we could not ask" is not "nobody".
    #[test]
    fn viewer_login_is_read_or_honestly_absent() {
        assert_eq!(
            map_viewer(&json!({"viewer": {"login": "octocat"}})),
            Some("octocat".to_string())
        );
        assert_eq!(map_viewer(&json!({})), None);
        assert_eq!(map_viewer(&json!({"viewer": null})), None);
    }

    use super::*;
    use crate::github::model::{CiState, MergeState, ReviewState};
    use serde_json::json;

    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!("../../tests/fixtures/search.json")).unwrap()
    }

    #[test]
    fn maps_all_prs() {
        assert_eq!(map_search(&fixture()).len(), 3);
    }

    #[test]
    fn maps_a_green_approved_pr() {
        let prs = map_search(&fixture());
        let pr = &prs[0];
        assert_eq!(pr.number, 42);
        assert_eq!(pr.repo, "octocat/hello-world");
        assert_eq!(pr.author, "octocat");
        assert_eq!(pr.ci, CiState::Success);
        assert_eq!(pr.merge, MergeState::Mergeable);
        assert_eq!(pr.review, ReviewState::Approved);
        assert_eq!(pr.labels.len(), 1);
        assert_eq!(pr.labels[0].name, "enhancement");
    }

    #[test]
    fn maps_a_conflicted_failing_draft() {
        let pr = map_search(&fixture())
            .into_iter()
            .find(|p| p.number == 43)
            .unwrap();
        assert!(pr.is_draft);
        assert_eq!(pr.ci, CiState::Failure);
        assert_eq!(pr.merge, MergeState::Conflicted);
        assert_eq!(pr.review, ReviewState::ChangesRequested);
        assert_eq!(pr.comment_count, 5);
    }

    /// GitHub computes mergeability lazily and returns UNKNOWN right after a
    /// push. Mapping that to Conflicted would show a false "needs rebase".
    fn node_with_threads(threads: serde_json::Value) -> serde_json::Value {
        json!({"authored": {"nodes": [{
            "number": 1, "title": "t", "url": "u", "isDraft": false,
            "headRefName": "f", "baseRefName": "main",
            "createdAt": "2026-08-20T00:00:00Z", "updatedAt": "2026-08-20T00:00:00Z",
            "author": {"login": "a"}, "repository": {"nameWithOwner": "acme/a"},
            "mergeable": "MERGEABLE", "reviewDecision": null,
            "isInMergeQueue": false, "totalCommentsCount": 0,
            "labels": {"nodes": []}, "commits": {"nodes": []},
            "reviewThreads": {"nodes": threads}
        }]}})
    }

    /// More than twenty threads are all counted (#810).
    ///
    /// The mapper was never wrong here -- it counts whatever the document
    /// returns -- so this asserts the pairing rather than the arithmetic:
    /// with the query's old `first: 20` window the badge could not
    /// receive more than twenty nodes, and the count silently capped
    /// while reading like an exact number.
    ///
    /// The figures are a real pull request. vercel/next.js #98469 carried
    /// 33 threads, 9 of them unresolved and un-outdated, and the old
    /// window reported 2 -- the unresolved ones sat outside the first
    /// twenty. Measured live while fixing this, not invented: a
    /// hand-made 21-thread fixture would assert the same thing but would
    /// not record that the case occurs in the wild.
    ///
    /// `the_list_query_counts_every_thread_rather_than_the_first_twenty`
    /// in `query.rs` is the other half, and is the one that fails if the
    /// window narrows again.
    #[test]
    fn counts_every_conversation_past_the_old_twenty_thread_window() {
        let mut threads: Vec<serde_json::Value> = Vec::new();
        // The first twenty, all settled: exactly the shape that made the
        // old window report almost nothing on a busy pull request.
        for _ in 0..20 {
            threads.push(json!({"isResolved": true, "isOutdated": false}));
        }
        for _ in 0..9 {
            threads.push(json!({"isResolved": false, "isOutdated": false}));
        }
        for _ in 0..4 {
            threads.push(json!({"isResolved": false, "isOutdated": true}));
        }
        assert_eq!(threads.len(), 33, "vercel/next.js #98469's thread count");

        let v = node_with_threads(json!(threads));
        assert_eq!(
            map_search(&v)[0].unresolved_threads,
            9,
            "all nine unresolved threads are counted, including the ones past the 20th"
        );
    }

    #[test]
    fn counts_only_open_conversations() {
        let v = node_with_threads(json!([
            {"isResolved": false, "isOutdated": false},
            {"isResolved": false, "isOutdated": false},
            {"isResolved": true, "isOutdated": false},
        ]));
        assert_eq!(map_search(&v)[0].unresolved_threads, 2);
    }

    /// An outdated thread hangs off a line that has since changed, so the
    /// author has nothing left to answer. Counting it would nag about work
    /// already done -- and unlike a resolved thread, nobody clicked
    /// anything to dismiss it.
    #[test]
    fn outdated_threads_do_not_count() {
        let v = node_with_threads(json!([
            {"isResolved": false, "isOutdated": true},
            {"isResolved": false, "isOutdated": false},
        ]));
        assert_eq!(map_search(&v)[0].unresolved_threads, 1);
    }

    #[test]
    fn no_threads_means_zero_not_a_panic() {
        assert_eq!(
            map_search(&node_with_threads(json!([])))[0].unresolved_threads,
            0
        );
        // And a PR from a query that never selected the field at all.
        let v = json!({"authored": {"nodes": [{
            "number": 1, "title": "t", "url": "u", "isDraft": false,
            "headRefName": "f", "baseRefName": "main",
            "createdAt": "2026-08-20T00:00:00Z", "updatedAt": "2026-08-20T00:00:00Z",
            "author": {"login": "a"}, "repository": {"nameWithOwner": "acme/a"},
            "mergeable": "MERGEABLE", "reviewDecision": null,
            "isInMergeQueue": false, "totalCommentsCount": 0,
            "labels": {"nodes": []}, "commits": {"nodes": []}
        }]}});
        assert_eq!(map_search(&v)[0].unresolved_threads, 0);
    }

    #[test]
    fn normalises_both_check_shapes() {
        // CheckRun reports `conclusion`; StatusContext reports `state`.
        assert_eq!(check_state(&json!({"conclusion": "SUCCESS"})), "success");
        assert_eq!(check_state(&json!({"state": "SUCCESS"})), "success");
        assert_eq!(check_state(&json!({"conclusion": "TIMED_OUT"})), "failure");
        assert_eq!(check_state(&json!({"state": "PENDING"})), "pending");
        assert_eq!(check_state(&json!({"conclusion": "SKIPPED"})), "skipped");
        // A null conclusion means the run has not finished.
        assert_eq!(check_state(&json!({"conclusion": null})), "pending");
    }

    /// An unmodelled value passes through rather than being coerced into
    /// "success", which would claim a green check that does not exist.
    #[test]
    fn an_unknown_check_state_is_never_success() {
        assert_eq!(
            check_state(&json!({"conclusion": "ACTION_REQUIRED"})),
            "ACTION_REQUIRED"
        );
    }

    /// The detail view needs the threads themselves, not just the count
    /// the header shows.
    #[test]
    fn maps_review_threads_in_full() {
        let v = json!({"repository": {"pullRequest": {
            "reviewThreads": {"nodes": [{
                "id": "RT_1", "isResolved": false, "isOutdated": false,
                "path": "src/api/hooks.ts", "line": 412,
                "viewerCanReply": true, "viewerCanResolve": true,
                "viewerCanUnresolve": false,
                "comments": {"totalCount": 2, "nodes": [
                    {"author": {"login": "carol"}, "createdAt": "2026-08-20T10:00:00Z",
                     "body": "This leaks the subscription"}
                ]}
            }]}
        }}});
        let t = &map_detail(&v, "o/r").review_threads[0];
        assert_eq!(t.id, "RT_1");
        assert!(!t.is_resolved);
        assert_eq!(t.path, "src/api/hooks.ts");
        assert_eq!(t.line, Some(412));
        assert!(t.viewer_can_reply);
        assert!(t.viewer_can_resolve);
        assert!(!t.viewer_can_unresolve);
        assert_eq!(t.comments[0].author, "carol");
        assert_eq!(
            t.comment_count, 2,
            "the total, not the page: the query caps thread comments at 10"
        );
    }

    /// A force-push strands a thread and GitHub sends `line: null`. Zero
    /// would render as `file.ts:0`, pointing at a line that never existed.
    #[test]
    fn an_outdated_thread_keeps_a_missing_line_missing() {
        let v = json!({"repository": {"pullRequest": {
            "reviewThreads": {"nodes": [{
                "id": "RT_2", "isResolved": false, "isOutdated": true,
                "path": "src/poll.rs", "line": null,
                "comments": {"totalCount": 0, "nodes": []}
            }]}
        }}});
        let t = &map_detail(&v, "o/r").review_threads[0];
        assert_eq!(t.line, None);
        assert!(t.is_outdated);
        assert!(
            !t.is_resolved,
            "outdated is not resolved: the question can still be open"
        );
    }

    /// Absent permission fields must not read as permission granted --
    /// the button would render and then fail with a 403 on click.
    #[test]
    fn missing_viewer_permissions_default_to_denied() {
        let v = json!({"repository": {"pullRequest": {
            "reviewThreads": {"nodes": [{
                "id": "RT_3", "isResolved": false, "isOutdated": false,
                "comments": {"totalCount": 0, "nodes": []}
            }]}
        }}});
        let t = &map_detail(&v, "o/r").review_threads[0];
        assert!(!t.viewer_can_reply);
        assert!(!t.viewer_can_resolve);
        assert!(!t.viewer_can_unresolve);
    }

    /// GitHub's own thread count must reach the view, so a truncated list
    /// can say what it is missing rather than rendering a subset that
    /// looks like the whole conversation (#802). The shape this guards is
    /// the one `append_remaining_checks` documents for checks: 20 of 25
    /// threads reads as finished, and the missing five can hold the
    /// unresolved blocking comment.
    #[test]
    fn carries_the_real_thread_total_so_truncation_can_be_shown() {
        let v = json!({"repository": {"pullRequest": {
            "reviewThreads": {"totalCount": 137, "nodes": [
                {"id": "a", "isResolved": false, "isOutdated": false,
                 "comments": {"totalCount": 0, "nodes": []}}
            ]}
        }}});
        let d = map_detail(&v, "o/r");
        assert_eq!(d.review_threads.len(), 1);
        assert_eq!(
            d.review_threads_total, 137,
            "the view cannot announce a shortfall it was never told about"
        );
    }

    /// Absent `totalCount` reads as "nothing missing", not as zero.
    ///
    /// A payload cached before #802 added the field, or a partial response
    /// that dropped it, would otherwise make the section claim
    /// "Showing 3 of 0" -- a subtraction against a count smaller than the
    /// list, the same failure `checks_total` and `poll::truncation_payload`
    /// both take care over.
    #[test]
    fn without_a_thread_total_reports_no_shortfall() {
        let v = json!({"repository": {"pullRequest": {
            "reviewThreads": {"nodes": [
                {"id": "a", "isResolved": false, "isOutdated": false,
                 "comments": {"totalCount": 0, "nodes": []}},
                {"id": "b", "isResolved": false, "isOutdated": false,
                 "comments": {"totalCount": 0, "nodes": []}},
                {"id": "c", "isResolved": false, "isOutdated": false,
                 "comments": {"totalCount": 0, "nodes": []}}
            ]}
        }}});
        let d = map_detail(&v, "o/r");
        assert_eq!(
            d.review_threads_total, 3,
            "no total must mean complete, never a zero the UI subtracts from"
        );
    }

    /// The header count and the thread list are two renderings of one
    /// truth. This pins them to the same payload so they cannot drift.
    #[test]
    fn the_count_and_the_list_agree_on_one_payload() {
        let v = json!({"repository": {"pullRequest": {
            "reviewThreads": {"nodes": [
                {"id": "a", "isResolved": false, "isOutdated": false,
                 "comments": {"totalCount": 0, "nodes": []}},
                {"id": "b", "isResolved": true, "isOutdated": false,
                 "comments": {"totalCount": 0, "nodes": []}},
                {"id": "c", "isResolved": false, "isOutdated": true,
                 "comments": {"totalCount": 0, "nodes": []}}
            ]}
        }}});
        let d = map_detail(&v, "o/r");
        assert_eq!(d.review_threads.len(), 3, "every thread is carried");
        assert_eq!(
            d.unresolved_threads, 1,
            "but only the actionable one is counted"
        );
        // The exact rule the count uses, restated over the list the UI
        // renders: if these ever disagree the header contradicts the
        // section beneath it.
        let actionable = d
            .review_threads
            .iter()
            .filter(|t| !t.is_resolved && !t.is_outdated)
            .count() as u64;
        assert_eq!(actionable, d.unresolved_threads);
    }

    #[test]
    fn maps_a_detail_payload() {
        let v = json!({"repository": {"pullRequest": {
            "number": 42, "title": "Add retry", "url": "u", "state": "OPEN",
            "isDraft": false, "body": "the description",
            "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN",
            "reviewDecision": "APPROVED",
            "additions": 100, "deletions": 20, "changedFiles": 3,
            "headRefName": "feature", "baseRefName": "main",
            "author": {"login": "octocat"},
            "comments": {"totalCount": 2, "nodes": [
                {"author": {"login": "octocat"}, "createdAt": "2026-08-20T10:00:00Z", "body": "hi"}
            ]},
            "reviewThreads": {"nodes": [
                {"isResolved": false, "isOutdated": false},
                {"isResolved": true, "isOutdated": false}
            ]},
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {
                "state": "SUCCESS",
                "contexts": {"nodes": [
                    {"name": "build", "conclusion": "SUCCESS", "detailsUrl": "b"},
                    {"context": "legacy", "state": "FAILURE", "targetUrl": "l"}
                ]}
            }}}]}
        }}});
        let d = map_detail(&v, "octocat/hello-world");
        assert_eq!(d.number, 42);
        assert_eq!(d.repo, "octocat/hello-world");
        assert_eq!(d.body, "the description");
        assert_eq!(d.state, "open");
        assert_eq!(d.merge_status, MergeStateStatus::Clean);
        assert_eq!(d.unresolved_threads, 1, "resolved threads must not count");
        assert_eq!(d.checks.len(), 2);
        assert_eq!(d.checks[0].name, "build");
        assert_eq!(d.checks[1].name, "legacy", "StatusContext uses `context`");
        assert_eq!(d.checks[1].state, "failure");
        assert_eq!(d.comments[0].author, "octocat");
    }

    /// #312: a pull request stays in Needs Attention through the whole
    /// rerun after its author has already fixed the problem.
    ///
    /// `statusCheckRollup.state` RANKS FAILURE above PENDING, so the
    /// collapsed value keeps saying failure while checks restart. That
    /// is the single state most likely to make the list untrustworthy:
    /// it nags about work already done.
    #[test]
    fn a_failure_that_is_re_running_reads_as_pending() {
        let node = json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {
            "state": "FAILURE",
            "contexts": {"nodes": [
                {"status": "COMPLETED"},
                {"status": "IN_PROGRESS"}
            ]}
        }}}]}});
        assert_eq!(ci_state(&node), CiState::Pending);
    }

    /// A failure with nothing running is still a failure. Reporting
    /// pending here would hide a genuinely broken pull request, which
    /// is the opposite and worse error.
    #[test]
    fn a_settled_failure_is_still_a_failure() {
        let node = json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {
            "state": "FAILURE",
            "contexts": {"nodes": [{"status": "COMPLETED"}, {"status": "COMPLETED"}]}
        }}}]}});
        assert_eq!(ci_state(&node), CiState::Failure);
    }

    /// Only FAILURE is second-guessed. A rollup that says SUCCESS while
    /// something is still running is not a state to override --
    /// reporting pending would make a mergeable pull request look
    /// unready.
    #[test]
    fn a_success_is_not_downgraded_by_a_running_check() {
        let node = json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {
            "state": "SUCCESS",
            "contexts": {"nodes": [{"status": "IN_PROGRESS"}]}
        }}}]}});
        assert_eq!(ci_state(&node), CiState::Success);
    }

    /// A cached snapshot written before this field existed has no
    /// contexts at all, and a failure must not silently become pending
    /// because the data is absent.
    #[test]
    fn a_failure_without_context_data_stays_a_failure() {
        let node = json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {
            "state": "FAILURE"
        }}}]}});
        assert_eq!(ci_state(&node), CiState::Failure);
    }

    /// Who is blocking a pull request -- the question `reviewDecision`
    /// cannot answer, since it collapses everyone into one verdict and
    /// names nobody.
    #[test]
    fn maps_requested_reviewers_and_skips_non_users() {
        let v = json!({"search": {"nodes": [{
            "number": 1, "title": "t", "url": "u",
            "createdAt": "2026-08-20T10:00:00Z", "updatedAt": "2026-08-20T10:00:00Z",
            "repository": {"nameWithOwner": "o/r"},
            "reviewRequests": {"nodes": [
                {"requestedReviewer": {"login": "octocat"}},
                {"requestedReviewer": {"login": "hubot"}},
                // A Team or Mannequin selects no `login`, so it arrives
                // as an empty object. Dropped rather than named.
                {"requestedReviewer": {}}
            ]},
            "latestReviews": {"nodes": [
                {"state": "APPROVED", "author": {"login": "ghost-reviewer"}},
                // No author: nothing to attribute it to.
                {"state": "COMMENTED", "author": null}
            ]}
        }]}});
        let prs = map_list(&v, "search");
        assert_eq!(prs[0].requested_reviewers, vec!["octocat", "hubot"]);
        assert_eq!(prs[0].latest_reviews.len(), 1);
        assert_eq!(prs[0].latest_reviews[0].author, "ghost-reviewer");
    }

    /// GitHub's own counts are carried, so a truncated list can say so.
    ///
    /// The four connections page their results. Without the total, a
    /// list cut at the window is indistinguishable from a complete one
    /// (#1089) -- and for `latestReviews` that is not an undercount but
    /// a wrong ANSWER, since `pendingReviewers` subtracts this list from
    /// the requested ones.
    #[test]
    fn carries_githubs_own_count_for_each_paged_connection() {
        let v = json!({"search": {"nodes": [{
            "number": 1, "title": "t", "url": "u",
            "createdAt": "2026-08-20T10:00:00Z", "updatedAt": "2026-08-20T10:00:00Z",
            "repository": {"nameWithOwner": "o/r"},
            "reviewRequests": {"totalCount": 9, "nodes": [
                {"requestedReviewer": {"login": "octocat"}}
            ]},
            "assignees": {"totalCount": 7, "nodes": [{"login": "hubot"}]},
            "latestReviews": {"totalCount": 23, "nodes": [
                {"state": "APPROVED", "author": {"login": "octocat"}}
            ]},
            // MEASURED live on open kubernetes/kubernetes pull requests:
            // 28 labels against this query's window of 20. The cut is
            // real, not hypothetical.
            "labels": {"totalCount": 28, "nodes": [{"name": "bug", "color": "d73a4a"}]}
        }]}});
        let pr = &map_list(&v, "search")[0];
        assert_eq!(pr.requested_reviewers_total, 9);
        assert_eq!(pr.assignees_total, 7);
        assert_eq!(pr.latest_reviews_total, 23);
        assert_eq!(pr.labels_total, 28);
    }

    /// ABSENT IS NOT ZERO -- the rule #846 shipped as a defect.
    ///
    /// A snapshot cached before #1089 added these fields has no
    /// `totalCount`. Defaulting to 0 would make every such row claim a
    /// truncation against a list that is complete, rendering the nonsense
    /// "1 of 0". The default is the length of the list instead, which
    /// reads as "nothing missing".
    #[test]
    fn an_absent_total_defaults_to_the_list_rather_than_to_zero() {
        let v = json!({"search": {"nodes": [{
            "number": 1, "title": "t", "url": "u",
            "createdAt": "2026-08-20T10:00:00Z", "updatedAt": "2026-08-20T10:00:00Z",
            "repository": {"nameWithOwner": "o/r"},
            // Every connection present, none of them counted.
            "reviewRequests": {"nodes": [{"requestedReviewer": {"login": "octocat"}}]},
            "assignees": {"nodes": [{"login": "hubot"}, {"login": "ghost"}]},
            "latestReviews": {"nodes": [
                {"state": "APPROVED", "author": {"login": "octocat"}}
            ]},
            "labels": {"nodes": [{"name": "bug", "color": "d73a4a"}]}
        }]}});
        let pr = &map_list(&v, "search")[0];
        assert_eq!(pr.requested_reviewers_total, 1);
        assert_eq!(pr.assignees_total, 2);
        assert_eq!(pr.latest_reviews_total, 1);
        assert_eq!(pr.labels_total, 1);
        // Not merely non-zero: each must equal ITS OWN list, or the UI
        // would compute a shortfall out of a mismatch.
        assert_eq!(
            pr.requested_reviewers_total,
            pr.requested_reviewers.len() as u64
        );
        assert_eq!(pr.assignees_total, pr.assignees.len() as u64);
        assert_eq!(pr.latest_reviews_total, pr.latest_reviews.len() as u64);
        assert_eq!(pr.labels_total, pr.labels.len() as u64);
    }

    /// A Team reviewer inside the window is not a person the window hid.
    ///
    /// `reviewRequests.totalCount` counts Teams and Mannequins, which the
    /// mapper drops because it cannot name them. Comparing that count
    /// against the mapped list would add the dropped Team to the genuine
    /// shortfall and report a truncation on a connection that arrived
    /// whole -- a false warning, which is the opposite error to the one
    /// #1089 is about and just as dishonest.
    ///
    /// This shape is MEASURED, not hypothetical: 1 of 25 open
    /// kubernetes/kubernetes pull requests has `totalCount` 3 across
    /// three nodes of which two are Users.
    #[test]
    fn a_team_reviewer_inside_the_window_is_not_counted_as_one_the_window_hid() {
        let v = json!({"search": {"nodes": [{
            "number": 1, "title": "t", "url": "u",
            "createdAt": "2026-08-20T10:00:00Z", "updatedAt": "2026-08-20T10:00:00Z",
            "repository": {"nameWithOwner": "o/r"},
            // Three asked for, three arrived, one of them a Team.
            "reviewRequests": {"totalCount": 3, "nodes": [
                {"requestedReviewer": {"login": "octocat"}},
                {"requestedReviewer": {"login": "hubot"}},
                {"requestedReviewer": {}}
            ]}
        }]}});
        let pr = &map_list(&v, "search")[0];
        assert_eq!(pr.requested_reviewers.len(), 2);
        // Two, not three: nothing was cut by the window, so the total
        // must equal what the row can actually name.
        assert_eq!(
            pr.requested_reviewers_total, 2,
            "a dropped Team must shrink the list, not become a missing person"
        );
    }

    /// And the genuine shortfall still survives alongside a dropped Team.
    ///
    /// The two reductions are independent, so a connection that is BOTH
    /// truncated and carrying a Team must report the window's cut and
    /// only the window's cut.
    #[test]
    fn a_window_cut_is_still_reported_when_a_team_was_also_dropped() {
        let v = json!({"search": {"nodes": [{
            "number": 1, "title": "t", "url": "u",
            "createdAt": "2026-08-20T10:00:00Z", "updatedAt": "2026-08-20T10:00:00Z",
            "repository": {"nameWithOwner": "o/r"},
            // Nine asked for, three arrived (six cut by the window), and
            // one of the three is a Team.
            "reviewRequests": {"totalCount": 9, "nodes": [
                {"requestedReviewer": {"login": "octocat"}},
                {"requestedReviewer": {"login": "hubot"}},
                {"requestedReviewer": {}}
            ]}
        }]}});
        let pr = &map_list(&v, "search")[0];
        assert_eq!(pr.requested_reviewers.len(), 2);
        // Two named plus the six the window cut. NOT 9, which would count
        // the Team twice over, and not 2, which would hide the cut.
        assert_eq!(pr.requested_reviewers_total, 8);
    }

    /// The default is the MAPPED length, not the raw node count.
    ///
    /// The mappers drop what they cannot name -- a Team in
    /// `requestedReviewer`, a review with no author. Defaulting to the
    /// raw length would claim more than the row can show, which is the
    /// same "N of M" nonsense in the other direction.
    #[test]
    fn an_absent_total_counts_what_survived_mapping_not_what_arrived() {
        let v = json!({"search": {"nodes": [{
            "number": 1, "title": "t", "url": "u",
            "createdAt": "2026-08-20T10:00:00Z", "updatedAt": "2026-08-20T10:00:00Z",
            "repository": {"nameWithOwner": "o/r"},
            "reviewRequests": {"nodes": [
                {"requestedReviewer": {"login": "octocat"}},
                // A Team: no login, dropped by the mapper.
                {"requestedReviewer": {}}
            ]},
            "latestReviews": {"nodes": [
                {"state": "APPROVED", "author": {"login": "octocat"}},
                // No author: dropped.
                {"state": "COMMENTED", "author": null}
            ]}
        }]}});
        let pr = &map_list(&v, "search")[0];
        assert_eq!(pr.requested_reviewers.len(), 1);
        assert_eq!(
            pr.requested_reviewers_total, 1,
            "two arrived, one is nameable"
        );
        assert_eq!(pr.latest_reviews.len(), 1);
        assert_eq!(
            pr.latest_reviews_total, 1,
            "two arrived, one is attributable"
        );
    }

    /// Empty is ORDINARY. Repositories that assign reviewers through a
    /// bot return nothing here -- measured: 0 of 25 on rust-lang/rust,
    /// against 24 of 25 on kubernetes/kubernetes. Treating absence as
    /// malformed would break the app on every such repo.
    #[test]
    fn absent_reviewer_fields_are_not_an_error() {
        let v = json!({"search": {"nodes": [{
            "number": 1, "title": "t", "url": "u",
            "createdAt": "2026-08-20T10:00:00Z", "updatedAt": "2026-08-20T10:00:00Z",
            "repository": {"nameWithOwner": "o/r"}
        }]}});
        let prs = map_list(&v, "search");
        assert_eq!(prs.len(), 1, "the pull request must still map");
        assert!(prs[0].requested_reviewers.is_empty());
        assert!(prs[0].latest_reviews.is_empty());
    }

    /// The viewer's OWN verdict, which `reviewDecision` cannot answer.
    ///
    /// This payload is the exact case that made the reported bug
    /// invisible: the viewer approved, someone else requested changes,
    /// so the aggregate decision is CHANGES_REQUESTED. Reading the
    /// aggregate would tell the user their approval had not landed.
    #[test]
    fn maps_each_reviewers_latest_verdict_separately_from_the_decision() {
        let v = json!({"repository": {"pullRequest": {
            "number": 42,
            "reviewDecision": "CHANGES_REQUESTED",
            "latestReviews": {"nodes": [
                {"state": "APPROVED", "author": {"login": "octocat"}},
                {"state": "CHANGES_REQUESTED", "author": {"login": "hubot"}},
                // No author: unmatched against any login, so dropped
                // rather than given a placeholder that could collide
                // with a real user.
                {"state": "APPROVED", "author": null}
            ]}
        }}});
        let d = map_detail(&v, "octocat/hello-world");
        assert_eq!(d.review, ReviewState::ChangesRequested);
        assert_eq!(
            d.latest_reviews.len(),
            2,
            "the authorless review is dropped"
        );
        assert_eq!(d.latest_reviews[0].author, "octocat");
        assert_eq!(d.latest_reviews[0].state, "APPROVED");
        assert_eq!(d.latest_reviews[1].state, "CHANGES_REQUESTED");
    }

    /// #1457: the header's three dates, read from the detail payload.
    ///
    /// Ready is the list row's derivation: the latest ready-for-review
    /// event when there is one, `createdAt` when the timeline arrived
    /// empty (never drafted), and absent while it is a draft.
    #[test]
    fn a_detail_payload_carries_its_age_and_last_commit() {
        let at = |s: &str| s.parse::<DateTime<Utc>>().unwrap();
        let payload = |draft: bool, events: serde_json::Value| {
            json!({"repository": {"pullRequest": {
                "number": 7, "isDraft": draft,
                "createdAt": "2026-08-01T09:00:00Z",
                "timelineItems": {"nodes": events},
                "commits": {"nodes": [{"commit": {"committedDate": "2026-08-09T17:30:00Z"}}]}
            }}})
        };

        let d = map_detail(
            &payload(false, json!([{"createdAt": "2026-08-05T12:00:00Z"}])),
            "acme/widgets",
        );
        assert_eq!(d.created_at, Some(at("2026-08-01T09:00:00Z")));
        assert_eq!(d.ready_at, Some(at("2026-08-05T12:00:00Z")));
        assert_eq!(d.last_commit_at, Some(at("2026-08-09T17:30:00Z")));

        let never_drafted = map_detail(&payload(false, json!([])), "acme/widgets");
        assert_eq!(
            never_drafted.ready_at,
            Some(at("2026-08-01T09:00:00Z")),
            "no ready event means it was ready when opened"
        );

        let draft = map_detail(
            &payload(true, json!([{"createdAt": "2026-08-05T12:00:00Z"}])),
            "acme/widgets",
        );
        assert_eq!(
            draft.ready_at, None,
            "a draft is not ready, however old its event"
        );
    }

    /// Absent dates stay absent, never the epoch or "now": the header
    /// omits what it could not read rather than printing "0s ago".
    #[test]
    fn absent_detail_dates_are_absent_not_zero() {
        let d = map_detail(
            &json!({"repository": {"pullRequest": {
                "number": 7, "isDraft": false,
                "createdAt": "not a date",
                "timelineItems": {"nodes": []},
                "commits": {"nodes": []}
            }}}),
            "acme/widgets",
        );
        assert_eq!(d.created_at, None);
        assert_eq!(
            d.ready_at, None,
            "with no opening time, never-drafted cannot date readiness"
        );
        assert_eq!(d.last_commit_at, None, "no head commit, no commit time");

        // The connection MISSING is unknown, not empty -- the same rule
        // `ready_at` applies to the list row.
        let no_timeline = map_detail(
            &json!({"repository": {"pullRequest": {
                "number": 7, "createdAt": "2026-08-01T09:00:00Z"
            }}}),
            "acme/widgets",
        );
        assert!(no_timeline.created_at.is_some());
        assert_eq!(no_timeline.ready_at, None);
    }

    /// A PR with no checks, no comments and a null author must not panic:
    /// the mapper's rule is that one odd payload never blanks the view.
    #[test]
    fn a_sparse_detail_payload_is_survivable() {
        let v = json!({"repository": {"pullRequest": {"number": 1}}});
        let d = map_detail(&v, "octocat/hello-world");
        assert_eq!(d.number, 1);
        assert_eq!(d.author, "ghost");
        assert!(d.checks.is_empty());
        assert!(d.comments.is_empty());
    }

    #[test]
    fn maps_every_merge_state_status_we_model() {
        for (raw, want) in [
            ("CLEAN", MergeStateStatus::Clean),
            ("DIRTY", MergeStateStatus::Dirty),
            ("BLOCKED", MergeStateStatus::Blocked),
            ("UNSTABLE", MergeStateStatus::Unstable),
            ("BEHIND", MergeStateStatus::Behind),
            ("DRAFT", MergeStateStatus::Draft),
        ] {
            let v = json!({"mergeStateStatus": raw});
            assert_eq!(merge_status(&v), want, "for {raw}");
        }
    }

    /// A value we do not model must never masquerade as mergeable: the
    /// merge button keys off Clean, so guessing here would enable it on a
    /// PR GitHub would reject.
    #[test]
    fn unrecognised_merge_status_is_unknown_never_clean() {
        assert_eq!(
            merge_status(&json!({"mergeStateStatus": "HAS_HOOKS"})),
            MergeStateStatus::Unknown
        );
        assert_eq!(
            merge_status(&json!({"mergeStateStatus": null})),
            MergeStateStatus::Unknown
        );
        assert_eq!(merge_status(&json!({})), MergeStateStatus::Unknown);
    }

    /// Branch refs ride along in the existing query at no extra cost, and
    /// distinguish a stacked PR -- which cannot merge until its base does
    /// -- from one targeting the default branch.
    #[test]
    fn maps_the_branch_pair() {
        let prs = map_search(&fixture());
        assert_eq!(prs[0].head_ref, "feature/retry-client");
        assert_eq!(prs[0].base_ref, "main");
        // The stacked one: base is another feature branch.
        let stacked = prs.iter().find(|p| p.number == 7).unwrap();
        assert_eq!(stacked.head_ref, "stack/part-2");
        assert_eq!(stacked.base_ref, "stack/part-1");
    }

    /// A node missing the fields must not panic -- the mapper's rule is
    /// that one malformed PR never blanks the list.
    #[test]
    fn missing_branch_refs_map_to_empty_strings() {
        let v = json!({"authored": {"nodes": [{
            "number": 1, "title": "t", "url": "u", "isDraft": false,
            "createdAt": "2026-08-20T00:00:00Z", "updatedAt": "2026-08-20T00:00:00Z",
            "author": {"login": "a"}, "repository": {"nameWithOwner": "acme/a"},
            "mergeable": "MERGEABLE", "reviewDecision": null,
            "isInMergeQueue": false, "totalCommentsCount": 0,
            "labels": {"nodes": []}, "commits": {"nodes": []}
        }]}});
        let prs = map_search(&v);
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].head_ref, "");
        assert_eq!(prs[0].base_ref, "");
    }

    #[test]
    fn unknown_mergeable_maps_to_checking_never_conflicted() {
        let pr = map_search(&fixture())
            .into_iter()
            .find(|p| p.number == 7)
            .unwrap();
        assert_eq!(pr.merge, MergeState::Checking);
        assert_ne!(pr.merge, MergeState::Conflicted);
    }

    /// A PR with no CI configured has no rollup at all.
    #[test]
    fn missing_status_rollup_maps_to_none() {
        let pr = map_search(&fixture())
            .into_iter()
            .find(|p| p.number == 7)
            .unwrap();
        assert_eq!(pr.ci, CiState::None);
        assert!(pr.in_merge_queue);
    }

    #[test]
    fn malformed_nodes_are_skipped_not_panicked_on() {
        let v = serde_json::json!({"search": {"nodes": [{"number": 1}, null]}});
        assert_eq!(map_search(&v).len(), 0);
    }

    // `assert_eq!` on an f64 expands to `==`, which is what float_cmp
    // sees (#892). Exact equality is the right assertion here and a
    // margin would weaken it: both sides are whole hours this test chose,
    // arrived at by dividing a difference of whole seconds, so an exact
    // 2.0 is reachable and a 2.0000001 would mean the median picked a
    // different sample -- precisely what this test exists to catch.
    #[allow(clippy::float_cmp)]
    #[test]
    fn cycle_trend_takes_the_nearest_rank_median() {
        // 4 samples: ceil(4*0.5)-1 = index 1.
        let v = json!({
            "current": {"issueCount": 4, "nodes": [
                {"createdAt":"2026-08-19T00:00:00Z","mergedAt":"2026-08-19T01:00:00Z"},
                {"createdAt":"2026-08-19T00:00:00Z","mergedAt":"2026-08-19T02:00:00Z"},
                {"createdAt":"2026-08-19T00:00:00Z","mergedAt":"2026-08-19T03:00:00Z"},
                {"createdAt":"2026-08-19T00:00:00Z","mergedAt":"2026-08-19T04:00:00Z"}
            ]},
            "previous": {"issueCount": 1, "nodes": [
                {"createdAt":"2026-08-12T00:00:00Z","mergedAt":"2026-08-12T10:00:00Z"}
            ]}
        });
        let t = map_cycle_trend(&v);
        assert_eq!(t.current_hours, 2.0);
        assert_eq!(t.previous_hours, 10.0);
        assert_eq!(t.current_count, 4);
        assert!(!t.sampled, "neither window hit the page cap");
    }

    /// Above 100 merges in a week, the nodes are a SAMPLE of that week --
    /// presenting the median as the week's would be the same class of
    /// plausible-but-wrong number this page exists to avoid.
    #[test]
    fn cycle_trend_flags_a_truncated_window() {
        let v = json!({
            "current": {"issueCount": 183, "nodes": [
                {"createdAt":"2026-08-19T00:00:00Z","mergedAt":"2026-08-19T01:00:00Z"}
            ]},
            "previous": {"issueCount": 1, "nodes": [
                {"createdAt":"2026-08-12T00:00:00Z","mergedAt":"2026-08-12T02:00:00Z"}
            ]}
        });
        assert!(map_cycle_trend(&v).sampled);
    }

    // Same as above, and the stronger case for exactness: the figure
    // asserted is 0.0, and "absent is not zero" runs through this
    // codebase -- a margin here would accept a near-zero that came from
    // having silently mapped a missing timestamp onto a real duration.
    #[allow(clippy::float_cmp)]
    #[test]
    fn cycle_trend_survives_missing_timestamps() {
        let v = json!({
            "current": {"issueCount": 1, "nodes": [
                {"createdAt":"2026-08-19T00:00:00Z","mergedAt": null}
            ]},
            "previous": {"issueCount": 0, "nodes": []}
        });
        let t = map_cycle_trend(&v);
        assert_eq!(t.current_hours, 0.0);
        assert_eq!(t.previous_hours, 0.0);
    }

    #[test]
    fn maps_day_buckets_oldest_first() {
        // NOTE: no "data" key -- octocrab strips the envelope before we see it.
        let v = json!({
            "m0": {"issueCount": 5}, "o0": {"issueCount": 7},
            "m1": {"issueCount": 3}, "o1": {"issueCount": 4}
        });
        let now = DateTime::parse_from_rfc3339("2026-08-20T14:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let pts = map_history(&v, 2, now);
        // Chart reads left-to-right as time moving forward, so oldest first.
        assert_eq!(pts[0].date, "2026-08-19");
        assert_eq!(pts[0].merged, 3);
        assert_eq!(pts[1].date, "2026-08-20");
        assert_eq!(pts[1].merged, 5);
        assert_eq!(pts[1].opened, 7);
    }

    #[test]
    fn missing_buckets_become_zero_not_panic() {
        let v = json!({ "m0": {"issueCount": 2} });
        let now = DateTime::parse_from_rfc3339("2026-08-20T14:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let pts = map_history(&v, 1, now);
        assert_eq!(pts[0].merged, 2);
        assert_eq!(pts[0].opened, 0);
    }

    #[test]
    fn aggregates_merged_detail_and_cycle_times() {
        let v = json!({"merged": {"nodes": [
            {"createdAt":"2026-08-19T10:00:00Z","mergedAt":"2026-08-19T12:00:00Z",
             "additions":100,"deletions":20,"changedFiles":3,
             "reviews":{"totalCount":1},"comments":{"totalCount":2},
             "repository":{"nameWithOwner":"acme/alpha"}},
            {"createdAt":"2026-08-19T10:00:00Z","mergedAt":"2026-08-19T10:30:00Z",
             "additions":10,"deletions":5,"changedFiles":1,
             "reviews":{"totalCount":0},"comments":{"totalCount":0},
             "repository":{"nameWithOwner":"acme/alpha"}}
        ]}});
        let d = map_merged_detail(&v);
        assert_eq!(d.sample_size, 2);
        assert_eq!(d.additions, 110);
        assert_eq!(d.changed_files, 4);
        // Sorted ascending so percentile() can index directly.
        assert_eq!(d.cycle_time_hours, vec![0.5, 2.0]);
        // additions+deletions per PR, ascending: 10+5=15 then 100+20=120.
        assert_eq!(d.pr_sizes, vec![15, 120]);
        assert_eq!(
            d.repo_counts,
            vec![RepoCount {
                repo: "acme/alpha".into(),
                merged: 2
            }]
        );
    }

    #[test]
    fn skips_nodes_missing_timestamps() {
        let v = json!({"merged": {"nodes": [
            {"createdAt":"2026-08-19T10:00:00Z","mergedAt": null,
             "additions":1,"deletions":0,"changedFiles":1,
             "reviews":{"totalCount":0},"comments":{"totalCount":0},
             "repository":{"nameWithOwner":"acme/alpha"}}
        ]}});
        let d = map_merged_detail(&v);
        assert!(d.cycle_time_hours.is_empty());
        // Still counted for volume: the PR merged, we just cannot time it.
        assert_eq!(d.sample_size, 1);
        assert_eq!(d.additions, 1);
    }

    /// One open pull request with the given ready-for-review timeline, for
    /// the `ready_at` tests below. `timeline` is the `timelineItems` value
    /// exactly as the list query selects it.
    fn with_timeline(is_draft: bool, timeline: Value) -> Value {
        json!({"search": {"nodes": [{
            "number": 1, "title": "t", "url": "u", "isDraft": is_draft,
            "createdAt": "2026-08-01T09:00:00Z", "updatedAt": "2026-08-20T10:00:00Z",
            "repository": {"nameWithOwner": "octocat/hello-world"},
            "timelineItems": timeline
        }]}})
    }

    /// #1407: a pull request that sat in draft is measured from when it
    /// was marked ready, not from when it was opened.
    #[test]
    fn ready_at_is_the_ready_for_review_event() {
        let v = with_timeline(
            false,
            json!({"nodes": [{"createdAt": "2026-08-19T15:30:00Z"}]}),
        );
        let pr = &map_list(&v, "search")[0];
        assert_eq!(
            pr.ready_at,
            Some(ts(&json!({"t": "2026-08-19T15:30:00Z"}), "t").unwrap())
        );
        assert_ne!(pr.ready_at, Some(pr.created_at));
    }

    /// No event means it was never a draft, so it became reviewable the
    /// moment it was opened: the same instant, not a guess.
    #[test]
    fn ready_at_without_an_event_is_when_it_was_opened() {
        let v = with_timeline(false, json!({"nodes": []}));
        let pr = &map_list(&v, "search")[0];
        assert_eq!(pr.ready_at, Some(pr.created_at));
    }

    /// Present but unreadable is UNKNOWN, never "now" and never the
    /// opened time: either would print a confident age that is wrong.
    #[test]
    fn an_unparseable_ready_time_is_unknown() {
        for bad in [json!("yesterday"), json!(""), json!(null), json!(7)] {
            let v = with_timeline(false, json!({"nodes": [{"createdAt": bad}]}));
            assert_eq!(map_list(&v, "search")[0].ready_at, None, "{bad}");
        }
    }

    /// A connection GitHub did not return is not "no event". Reading it
    /// as never-a-draft would date a PR marked ready an hour ago from
    /// the week it spent in draft.
    #[test]
    fn a_missing_timeline_is_unknown_not_never_drafted() {
        for missing in [json!(null), json!({}), json!({"nodes": null})] {
            let v = with_timeline(false, missing.clone());
            assert_eq!(map_list(&v, "search")[0].ready_at, None, "{missing}");
        }
        // And a node shaped without the field at all, as a cached or
        // partial response would be.
        let mut v = with_timeline(false, json!({"nodes": []}));
        v["search"]["nodes"][0]
            .as_object_mut()
            .unwrap()
            .remove("timelineItems");
        assert_eq!(map_list(&v, "search")[0].ready_at, None);
    }

    /// A draft is not ready for review, whatever its history: one marked
    /// ready and then converted back still has the old event.
    #[test]
    fn a_draft_has_no_ready_time() {
        let v = with_timeline(
            true,
            json!({"nodes": [{"createdAt": "2026-08-19T15:30:00Z"}]}),
        );
        assert_eq!(map_list(&v, "search")[0].ready_at, None);
        let v = with_timeline(true, json!({"nodes": []}));
        assert_eq!(map_list(&v, "search")[0].ready_at, None);
    }
}
