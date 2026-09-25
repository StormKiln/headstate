//! The PR model. Field names, enum variants, and serde casing here are
//! consumed by later tasks (the Tauri command layer, the store) and are
//! mirrored into TypeScript in a later milestone, so they are chosen to be
//! stable and are not to be changed casually.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CiState {
    Success,
    Failure,
    Pending,
    None,
}

/// GitHub's own summary of whether a PR can merge right now.
///
/// Richer than `MergeState`, which only distinguishes conflicts: this
/// separates "ready" from "blocked on a required review" from "checks
/// failing" -- the difference between a merge button that works and one
/// that lies.
///
/// Live distribution across 25 open PRs when this was added: Clean 11,
/// Dirty 7, Unstable 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeStateStatus {
    /// Mergeable now.
    Clean,
    /// Merge conflicts.
    Dirty,
    /// A required review or check is missing.
    Blocked,
    /// Non-required checks are failing.
    Unstable,
    /// The base has moved ahead; needs updating.
    Behind,
    /// Draft PRs cannot merge.
    Draft,
    /// Still being computed, or a state we do not model.
    #[default]
    Unknown,
}

/// Three states, not two. `Checking` exists because GitHub computes
/// mergeability lazily and reports UNKNOWN until it finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeState {
    Mergeable,
    Conflicted,
    Checking,
}

#[derive(Debug, Clone, Default, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    ReviewRequired,
    /// No verdict. The default, so a partially-built value never claims
    /// an approval that does not exist.
    #[default]
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    /// GraphQL node ID, so a row can act without first opening the
    /// detail view. Rides along in the list query at no extra cost.
    ///
    /// `#[serde(default)]` here and on the fields below is about the
    /// SNAPSHOT CACHE, not the wire: these were added after v1.0.1, and a
    /// cache written by that version has no such key. Without a default,
    /// serde fails the whole payload and every pull request vanishes on
    /// upgrade -- which is what shipped in v2.0.0. An empty id means
    /// "cannot act on this row", and the next poll replaces it wholesale.
    #[serde(default)]
    pub id: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub repo: String,
    pub author: String,
    pub is_draft: bool,
    /// The branch being merged, and the branch it merges into.
    ///
    /// Carried for the list row: a PR stacked on another branch cannot
    /// merge until its base does, and is otherwise indistinguishable from
    /// one targeting the default branch.
    pub head_ref: String,
    /// The head commit, so an "update branch" click can tell GitHub which
    /// commit the user was actually looking at.
    #[serde(default)]
    pub head_oid: String,
    /// The head branch's Ref node id, for deleting it after merge.
    ///
    /// `None` once the branch is gone -- which is exactly how the UI
    /// tells "already cleaned up" from "still there".
    #[serde(default)]
    pub head_ref_id: Option<String>,
    pub base_ref: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// When the pull request became ready for review (#1407).
    ///
    /// The latest `ReadyForReviewEvent` when there is one. With NO such
    /// event the pull request was never a draft, so it became reviewable
    /// when it was opened: `created_at` is then the same moment, not a
    /// fallback guess.
    ///
    /// `None` means UNKNOWN, and must never render as zero or as "just
    /// now":
    /// - the timeline connection did not arrive, which is not the same as
    ///   arriving empty;
    /// - the event's time did not parse;
    /// - the pull request is a draft now, so it is not ready at all (an
    ///   event from before it was converted back is stale);
    /// - the snapshot was cached by a build older than this field, hence
    ///   `#[serde(default)]` -- the reason every field above carries one.
    #[serde(default)]
    pub ready_at: Option<DateTime<Utc>>,
    pub ci: CiState,
    pub merge: MergeState,
    /// GitHub's own merge-readiness summary. See `MergeStateStatus`.
    // Defaults to Unknown, never Clean: a merge button enabled on data the
    // app never fetched is the one wrong answer that costs something.
    #[serde(default)]
    pub merge_status: MergeStateStatus,
    pub review: ReviewState,
    pub in_merge_queue: bool,
    pub labels: Vec<Label>,
    pub comment_count: u64,
    /// Review conversations still open on the current code.
    ///
    /// Resolved threads are excluded, and so are OUTDATED ones -- a thread
    /// on a line that has since changed is not something the author still
    /// has to answer, and counting it would nag about work already done.
    ///
    /// Deliberately a count of open conversations, not a claim that the PR
    /// is blocked: whether a repo REQUIRES resolution before merging lives
    /// in `requiresConversationResolution`, which needs admin access on
    /// that repository and is unreadable for most of them.
    #[serde(default)]
    pub unresolved_threads: u64,
    /// Logins whose review is still outstanding.
    ///
    /// Empty is ORDINARY, not missing data: repositories that assign
    /// reviewers through a bot return nothing here, and so does a solo
    /// account. Anything the UI shows must read sensibly when empty.
    ///
    /// Users only. `requestedReviewer` is a union that also admits
    /// Team and Mannequin; those are dropped rather than given an
    /// invented name.
    #[serde(default)]
    pub requested_reviewers: Vec<String>,
    /// Assignees, used as a fallback when no reviewer was requested.
    ///
    /// Some repositories assign the reviewer rather than requesting a
    /// review -- rust-lang/rust does, and `reviewRequests` is empty on
    /// every one of its open pull requests as a result.
    #[serde(default)]
    pub assignees: Vec<String>,
    /// Who has already reviewed, and what they said.
    ///
    /// A different question from `review`, which collapses every
    /// reviewer into one verdict and names nobody.
    #[serde(default)]
    pub latest_reviews: Vec<ReviewerVerdict>,
    /// How many entries each of the four connections above has, which
    /// the lists beside them can be short of (#1089).
    ///
    /// NOT a bare `totalCount`: see `map.rs`'s `connection_total`. The
    /// window's cut and the mapper's drops are two different reductions,
    /// and adding them together reports a truncation on a connection
    /// that arrived whole -- measured on 1 of 25 live pull requests,
    /// where a Team reviewer inside the window would have read as a
    /// person the window hid.
    ///
    /// Never 0 against a non-empty list -- the same rule
    /// `review_threads_total` and `checks_total` state on `PrDetail`. A
    /// payload written before this field existed, or a partial response
    /// that dropped it, reads as "nothing missing"; a 0 would render the
    /// nonsense "3 of 0", which is #846 all over again.
    ///
    /// `latest_reviews_total` is the load-bearing one. `pendingReviewers`
    /// subtracts the answered from the asked, so a reviewer outside the
    /// window is reported as still pending when they have approved --
    /// a wrong answer, not merely a short list.
    #[serde(default)]
    pub requested_reviewers_total: u64,
    #[serde(default)]
    pub assignees_total: u64,
    #[serde(default)]
    pub latest_reviews_total: u64,
    #[serde(default)]
    pub labels_total: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub merged_week: u64,
    pub merged_month: u64,
    pub in_merge_queue: u64,
    pub needs_attention: u64,
    pub awaiting_review: u64,
    pub ready_to_queue: u64,
    pub blocked_by_comments: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HistoryPoint {
    pub date: String,
    pub opened: u64,
    pub merged: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepoCount {
    pub repo: String,
    pub merged: u64,
}

/// One merged PR, enough to name and open it.
///
/// The Stats page could show that something took four days or ran to ten
/// thousand lines, and then not say WHICH pull request that was -- every
/// figure was a scalar with no way back to the thing it described.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MergedPr {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub repo: String,
    pub cycle_time_hours: f64,
    pub size: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MergedDetail {
    pub cycle_time_hours: Vec<f64>,
    /// additions+deletions per PR, sorted ascending for percentile lookup.
    pub pr_sizes: Vec<u64>,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    /// Reviews are NOT here, deliberately (#851).
    ///
    /// `review_count` existed, was requested as `reviews { totalCount }`,
    /// summed, typed and fixtured -- and rendered nowhere. #851 asked for it
    /// to be displayed or dropped with a reason, and the reason is that the
    /// figure is a constant on the data it describes.
    ///
    /// MEASURED live 2026-09-12 over this account's 50 most recent merged
    /// pull requests: reviews totalled **0**, with no pull request carrying
    /// even one, against 4 comments across the same sample. These are
    /// self-merged pull requests, so a "reviews per PR" card is a permanent
    /// 0.0. `InsightCards.tsx` had already reached that conclusion and
    /// recorded it -- it shows PR size in that slot precisely because review
    /// counts "came back 0 across the whole sample" -- so what remained was
    /// a paid-for GraphQL field feeding a value the UI had deliberately
    /// decided not to show.
    ///
    /// Dropped rather than kept for a future consumer: the reviews-GIVEN
    /// board (`stats::load_reviewers`) is where review activity is actually
    /// measured, and it uses `reviewed-by:<login>` searches because a pull
    /// request node says how many reviews it has and never who wrote them.
    /// So this field could not have grown into that feature either.
    pub comment_count: u64,
    pub sample_size: u64,
    pub repo_counts: Vec<RepoCount>,
    /// The slowest and largest merges in the sample, so the outlier a
    /// figure describes is reachable rather than merely reported.
    pub slowest: Vec<MergedPr>,
    pub largest: Vec<MergedPr>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Periods {
    pub week_current: u64,
    pub week_previous: u64,
    pub opened_week_current: u64,
    pub opened_week_previous: u64,
    pub month_current: u64,
    pub month_previous: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct History {
    pub points: Vec<HistoryPoint>,
    pub week_current: u64,
    pub week_previous: u64,
    pub opened_week_current: u64,
    pub opened_week_previous: u64,
    pub month_current: u64,
    pub month_previous: u64,
}

impl PullRequest {
    /// Blocked on the author and nobody else: a real conflict, or failing
    /// CI. `Checking` is deliberately excluded -- GitHub reports UNKNOWN
    /// mergeability while it computes, and treating that as a conflict
    /// would fire a false warning on every push.
    ///
    /// This is the ONE Rust owner of the rule. The frontend has its own
    /// copy in `src/lib/derive.ts`; `needs_attention_rule_matches_frontend`
    /// pins them together so the tray badge can never disagree with the
    /// priorities strip.
    pub fn needs_attention(&self) -> bool {
        self.merge == MergeState::Conflicted || self.ci == CiState::Failure
    }
}

/// How many PRs are blocked on the author. Feeds the tray badge.
pub fn needs_attention_count(prs: &[PullRequest]) -> u64 {
    prs.iter().filter(|p| p.needs_attention()).count() as u64
}

/// One CI check on a pull request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CheckRun {
    pub name: String,
    /// `success`, `failure`, `pending`, or the raw value when unmodelled.
    pub state: String,
    /// Where to see the run itself. Empty when GitHub gives none.
    pub url: String,
    /// The Actions workflow RUN this check belongs to, for re-running.
    ///
    /// None for a check from an app that is not GitHub Actions, and for
    /// a plain commit status -- neither has a workflow run to re-run.
    /// Optional rather than defaulted, so the UI offers the button only
    /// where it can actually work rather than failing on click.
    pub run_id: Option<u64>,
}

/// One comment on a pull request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PrComment {
    pub author: String,
    pub created_at: String,
    pub body: String,
}

/// One review conversation: an inline comment thread anchored to a line.
///
/// A DIFFERENT object from `PrComment`, which is a flat top-level comment
/// on the pull request itself. Only threads can be resolved -- GitHub has
/// no such concept for issue comments -- which is why the two are modelled
/// separately rather than merged into one list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReviewThread {
    /// The thread's node id, which the resolve and reply mutations take.
    pub id: String,
    pub is_resolved: bool,
    /// Whether the line this thread was anchored to still exists.
    ///
    /// NOT the same as resolved: a force-push can strand a thread whose
    /// question is still open and still unanswered. The two are reported
    /// separately so the UI never presents "the code moved" as "this was
    /// dealt with".
    pub is_outdated: bool,
    /// The file the thread hangs off. Empty when GitHub gives none.
    pub path: String,
    /// The line, or None once the anchor is gone -- which is exactly when
    /// `is_outdated` is true. Optional rather than 0 so the UI can render
    /// the path alone instead of a `file.ts:0` that points nowhere.
    pub line: Option<u64>,
    /// What the VIEWER may do, asked per thread rather than assumed.
    ///
    /// Three separate permissions because GitHub grants them separately:
    /// replying to a thread and resolving it are different rights, and
    /// unresolving is a third. Without these the UI shows buttons that
    /// 403 on click.
    pub viewer_can_reply: bool,
    pub viewer_can_resolve: bool,
    pub viewer_can_unresolve: bool,
    pub comments: Vec<PrComment>,
    /// How many comments the thread really has, which can exceed
    /// `comments.len()` -- the query pages at 10.
    pub comment_count: u64,
}

/// One reviewer's latest review on a pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewerVerdict {
    pub author: String,
    /// GitHub's raw review state: APPROVED, CHANGES_REQUESTED,
    /// COMMENTED, DISMISSED or PENDING. Passed through rather than
    /// narrowed to a bool -- a DISMISSED approval is not an approval,
    /// and collapsing it here would silently claim otherwise.
    pub state: String,
}

/// Everything the detail view renders.
///
/// A separate type from `PullRequest`: that one is a LIST row fetched 100
/// at a time on a 2-minute loop, and adding a body and comments to it
/// would make every poll carry data almost none of the rows need.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PrDetail {
    /// GraphQL node ID. Every mutation takes this rather than a number,
    /// so the detail view is what makes a PR actionable.
    pub id: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub is_draft: bool,
    /// When the pull request was opened (#1457). `None` when GitHub's
    /// value is missing or does not parse -- the header then omits the
    /// age rather than printing one measured from nothing.
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    /// When it became ready for review (#1457): the same derivation, from
    /// the same selection, as `PullRequest::ready_at` -- see that field for
    /// what `None` means. `None` also when `created_at` is, since a pull
    /// request never drafted became ready when it was opened.
    #[serde(default)]
    pub ready_at: Option<DateTime<Utc>>,
    /// The head commit's `committedDate` (#1457). The committer's clock,
    /// NOT the push time: a rebase or a late push leaves it earlier than
    /// the push, which is why the view says "last commit". `None` when
    /// there is no head commit or its date is missing.
    #[serde(default)]
    pub last_commit_at: Option<DateTime<Utc>>,
    pub body: String,
    pub author: String,
    pub repo: String,
    pub head_ref: String,
    /// The head commit, so an "update branch" click can tell GitHub which
    /// commit the user was actually looking at.
    #[serde(default)]
    pub head_oid: String,
    /// The head branch's Ref node id, for deleting it after merge.
    ///
    /// `None` once the branch is gone -- which is exactly how the UI
    /// tells "already cleaned up" from "still there".
    #[serde(default)]
    pub head_ref_id: Option<String>,
    pub base_ref: String,
    // Defaults to Unknown, never Clean: a merge button enabled on data the
    // app never fetched is the one wrong answer that costs something.
    #[serde(default)]
    pub merge_status: MergeStateStatus,
    pub review: ReviewState,
    /// Every reviewer's latest review state, keyed by login.
    ///
    /// A different question from `review`, which is the pull request's
    /// AGGREGATE decision: it reads `changes_requested` when someone
    /// else blocked the PR and `review_required` when a second approval
    /// is outstanding. Neither tells the user whether their own approval
    /// landed -- which is the whole reported confusion.
    ///
    /// The viewer is matched by login on the frontend, which already
    /// knows who it is (`useViewer`), rather than threading a login down
    /// through the client and mapper for one field.
    #[serde(default)]
    pub latest_reviews: Vec<ReviewerVerdict>,
    /// Whether this pull request's base branch uses a merge queue.
    ///
    /// Decides whether the primary action merges or enqueues. Defaults
    /// to false when absent, which is the safe direction: it offers a
    /// plain Merge, and GitHub refuses that if the branch really does
    /// require the queue. Defaulting the other way would hide Merge on
    /// every repo the field could not be read for.
    #[serde(default)]
    pub merge_queue_enabled: bool,
    /// Whether it is currently queued (and not rejected by the queue).
    #[serde(default)]
    pub in_merge_queue: bool,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    #[serde(default)]
    pub unresolved_threads: u64,
    pub comment_count: u64,
    pub comments: Vec<PrComment>,
    /// The review conversations, resolved and unresolved alike.
    ///
    /// `unresolved_threads` above stays the single source of the COUNT
    /// (it is what the list view and the header already read), and is
    /// still derived by the same function -- storing the number twice
    /// would let the header disagree with the list beneath it.
    #[serde(default)]
    pub review_threads: Vec<ReviewThread>,
    /// GitHub's own count of review threads, so a TRUNCATED list can say
    /// what it is missing (#802).
    ///
    /// Paired with `review_threads` exactly as `checks_total` is paired
    /// with `checks`, and for the same reason -- render what arrived and
    /// annotate the gap rather than presenting a subset as complete. The
    /// failure this guards is the one `append_remaining_checks` documents
    /// one level up: a list that does not LOOK truncated. Twenty of
    /// twenty-five threads reads as the whole conversation, so an
    /// unresolved blocking comment can sit outside the window while the
    /// view looks finished.
    ///
    /// NOT a replacement for `unresolved_threads`, which stays the source
    /// of the header's count. That number is derived from the threads
    /// that arrived and is therefore a FLOOR when this total exceeds
    /// `review_threads.len()` -- which is the honest reading, and why the
    /// view annotates the list rather than adjusting the header to a
    /// number it cannot know.
    ///
    /// A count SMALLER than `review_threads.len()` is possible in
    /// principle (a thread resolved and hidden between GitHub computing
    /// the total and serialising the nodes), so readers must treat the
    /// difference as saturating rather than trusting it to be
    /// non-negative -- the same care `checks_total` and
    /// `poll::truncation_payload` take.
    #[serde(default)]
    pub review_threads_total: u64,
    pub checks: Vec<CheckRun>,
    /// GitHub's own count of check contexts on the head commit, so a
    /// CAPPED list can say what it is missing (#790).
    ///
    /// Paired with `checks` the way `comment_count` is paired with
    /// `comments`, and for the same reason: the panel renders what
    /// arrived and annotates the gap rather than presenting a subset as
    /// complete. `append_remaining_checks` explains why this matters
    /// more here than for comments -- a short check list reads as a
    /// green pull request.
    ///
    /// A count SMALLER than `checks.len()` is possible (the two numbers
    /// come from different pages of a rollup that can grow mid-fetch),
    /// so every reader must treat the difference as saturating rather
    /// than trusting it to be non-negative -- the same care
    /// `poll::truncation_payload` takes with `issueCount`.
    #[serde(default)]
    pub checks_total: u64,
}

#[cfg(test)]
mod attention_tests {
    use super::*;

    fn pr(merge: MergeState, ci: CiState) -> PullRequest {
        let t = DateTime::parse_from_rfc3339("2026-08-20T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        PullRequest {
            id: "PR_test".into(),
            number: 1,
            title: "t".into(),
            url: "u".into(),
            repo: "acme/repo".into(),
            author: "a".into(),
            is_draft: false,
            head_ref: "feature/x".into(),
            head_oid: "deadbeef".into(),
            head_ref_id: None,
            base_ref: "main".into(),
            created_at: t,
            ready_at: Some(t),
            updated_at: t,
            ci,
            merge,
            merge_status: MergeStateStatus::Clean,
            review: ReviewState::None,
            in_merge_queue: false,
            labels: vec![],
            comment_count: 0,
            unresolved_threads: 0,
            requested_reviewers: Vec::new(),
            assignees: Vec::new(),
            latest_reviews: Vec::new(),
            // Zero totals against empty lists: nothing was cut.
            requested_reviewers_total: 0,
            assignees_total: 0,
            latest_reviews_total: 0,
            labels_total: 0,
        }
    }

    #[test]
    fn conflicted_or_failing_needs_attention() {
        assert!(pr(MergeState::Conflicted, CiState::Success).needs_attention());
        assert!(pr(MergeState::Mergeable, CiState::Failure).needs_attention());
        assert!(pr(MergeState::Conflicted, CiState::Failure).needs_attention());
    }

    /// The rule that keeps the badge honest. GitHub reports UNKNOWN
    /// mergeability while it computes, so counting `Checking` would badge
    /// the tray on every push and train the user to ignore it.
    #[test]
    fn checking_is_not_attention() {
        assert!(!pr(MergeState::Checking, CiState::Success).needs_attention());
        assert!(!pr(MergeState::Checking, CiState::Pending).needs_attention());
    }

    #[test]
    fn pending_and_none_ci_are_not_attention() {
        assert!(!pr(MergeState::Mergeable, CiState::Pending).needs_attention());
        assert!(!pr(MergeState::Mergeable, CiState::None).needs_attention());
    }

    #[test]
    fn counts_only_the_blocked_ones() {
        let prs = vec![
            pr(MergeState::Conflicted, CiState::Success),
            pr(MergeState::Mergeable, CiState::Failure),
            pr(MergeState::Mergeable, CiState::Success),
            pr(MergeState::Checking, CiState::Pending),
        ];
        assert_eq!(needs_attention_count(&prs), 2);
    }

    /// Pins the Rust rule to the TypeScript one by reading the actual
    /// source. The two implementations are unavoidably separate -- one
    /// badges a hidden tray, the other renders a strip -- so a drift here
    /// would silently make the badge disagree with the UI.
    #[test]
    fn needs_attention_rule_matches_frontend() {
        let ts = include_str!("../../../src/lib/derive.ts");
        let start = ts
            .find("export function needsAttention")
            .expect("needsAttention not found in derive.ts");
        let body = &ts[start..start + 200];
        assert!(
            body.contains(r#"pr.merge === "conflicted""#),
            "frontend rule changed: {body}"
        );
        assert!(
            body.contains(r#"pr.ci === "failure""#),
            "frontend rule changed: {body}"
        );
        assert!(
            !body.contains("checking"),
            "frontend now counts checking; Rust does not: {body}"
        );
    }
}

/// Median cycle time for the current week against the previous one.
///
/// `sampled` is true when either window exceeded the 100-node page size,
/// meaning the medians are over a sample of that week rather than all of
/// it. The UI says so rather than implying a census.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CycleTrend {
    pub current_hours: f64,
    pub previous_hours: f64,
    pub current_count: u64,
    pub previous_count: u64,
    pub sampled: bool,
}
