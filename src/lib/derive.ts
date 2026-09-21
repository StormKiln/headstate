import type { PullRequest, Stats } from "../types/pr";
import type { AdviceGrouping } from "./adviceGrouping";

export const STALE_DAYS = 3;

export interface Filters {
  /// Free-text match over title, repo, and PR number. Case-insensitive.
  query?: string;
  /// Only PRs with review conversations still open.
  unresolvedOnly?: boolean;
  /// Only PRs awaiting MY review verdict. Review-view only.
  needsMyReviewOnly?: boolean;
  repo?: string;
  readyOnly?: boolean;
  draftsOnly?: boolean;
  ci?: PullRequest["ci"];
  review?: PullRequest["review"];
  includeLabels?: string[];
  excludeLabels?: string[];
  needsAttentionOnly?: boolean;
  staleOnly?: boolean;
  inMergeQueueOnly?: boolean;
  awaitingReviewOnly?: boolean;
  readyToQueueOnly?: boolean;
  sort?: "newest" | "oldest" | "recently-updated" | "least-recently-updated";
  /// How the "Ready for review" strip on the To Review page is ordered
  /// (#1277). Defaults to `"oldest-opened"` when absent.
  ///
  /// SEPARATE from `sort`, which orders the main PR list below the strip
  /// and defaults to newest-first. One key for both would mean the review
  /// queue could not default to oldest without flipping the list's default
  /// too, and a reviewer changing the strip's order would silently reorder
  /// the list they were reading.
  ///
  /// The values name the FIELD, not just the direction. "Oldest first" is
  /// ambiguous between "opened oldest" and "waiting for review longest",
  /// and those are different PRs whenever something sat in draft -- which
  /// is the ambiguity #1277 was filed about. `PullRequest` carries
  /// `created_at` and `updated_at` only, with no ready-for-review
  /// timestamp fetched anywhere, so this sorts on `created_at` and says so
  /// rather than implying an answer it does not have.
  readySort?: "oldest-opened" | "newest-opened";
  /// How the CLAUDE.md advice list is organised (#1291). Defaults to
  /// `"none"` -- the flat list -- when absent.
  ///
  /// Typed as `AdviceGrouping` from `@/lib/adviceGrouping`, imported as a
  /// type so this module keeps no runtime dependency on it.
  ///
  /// Here rather than in `useState` beside the panel for the reason
  /// `readySort` is: this is a view PREFERENCE, and the per-view store is
  /// where every other one lives and where `partialize` persists it
  /// without a second mechanism. Per-view rather than global because the
  /// panel renders only on `claude-md`.
  ///
  /// SEPARATE from `sort` and `readySort`, which order pull requests.
  /// Sharing a key with either would mean a grouping chosen on the advice
  /// panel reaching a list whose values it is not even in the union of.
  adviceGrouping?: AdviceGrouping;
  /// Safety verdicts to show on the Worktrees page (#1140).
  ///
  /// `undefined` and `[]` both mean "show everything", deliberately.
  /// An empty facet set is what a user gets by unticking the last box,
  /// and rendering an empty list for it would read as "this repository
  /// has no worktrees" -- the confident wrong answer #846 keeps having
  /// to remove. Narrowing to nothing is not a question anyone asks.
  ///
  /// Kinds rather than a `Safety`, because the verdicts carry payloads
  /// (`Dirty(n)`, `Locked{..}`) and a facet is about the CATEGORY. A
  /// user filtering for "dirty" wants all of them, not one file count.
  safety?: string[];
  /// Free-text match over a worktree's path and branch (#1140).
  ///
  /// Separate from `query`, which is PR-shaped and matches title, repo
  /// and number. One key for both would mean a search typed on the PR
  /// list silently narrowing the Worktrees page, and the store persists
  /// filters per view precisely so the two do not leak into each other.
  worktreeQuery?: string;
  /// Only worktrees a Claude Code session is working in (#1140).
  ///
  /// The one facet that is not a safety verdict: "who is using this" is
  /// a different question from "can I remove it", and it is the one
  /// that stops a user removing a tree out from under a running agent.
  occupiedOnly?: boolean;
  /// WHERE a PR Stats question is scoped, mirroring
  /// `github::stats::scope::Scope` (#827) rather than inventing a parallel
  /// representation (#825 requirement 7).
  ///
  /// `"repo"` carries `owner/name` in `statsScopeValue`, `"org"` and
  /// `"user"` carry a login, and `"all"` carries nothing. The kind and the
  /// value are two fields rather than one `"org:acme"`-style string because
  /// the Rust command takes them as two parameters (`scopeKind`,
  /// `scopeValue`) -- a packed string would be assembled here and split
  /// there, which is two places for the grammar to disagree.
  ///
  /// `undefined` means no scope has been chosen, which is NOT the same as
  /// `"all"`: the sidebar has to be able to tell "nothing clicked yet" from
  /// "the widest scope was deliberately clicked", because only the second
  /// should highlight a row.
  statsScopeKind?: "repo" | "org" | "user" | "all";
  statsScopeValue?: string;
  /// WHOSE PRs a PR Stats question is about -- a login, mirroring
  /// `Subject::Login`.
  ///
  /// `undefined` is `Subject::Viewer`, which is the default the stats page
  /// has always answered ("how am I doing?"). A login here is the second
  /// audience #823 names, reached by clicking a Members row.
  ///
  /// Deliberately INDEPENDENT of the scope fields: "this person, in this
  /// org" is the natural question a member row under an org heading asks,
  /// and collapsing the two axes into one selection would make it
  /// unaskable.
  statsSubject?: string;
}

/// How many filters are narrowing the list.
///
/// For the phone's Filters button, which has to say how much is hidden
/// behind it: the eight controls that sit visibly in a row on a desktop
/// are collapsed there, so without a count the user cannot tell a bar
/// that is filtering hard from one that is not filtering at all.
///
/// `query` is excluded because the search field stays visible beside the
/// button and speaks for itself, and `sort`, `readySort` and
/// `adviceGrouping` because arranging a list is not hiding any of it --
/// counting any of them would make the badge argue with what the user can
/// already see. Grouping in particular shows every finding the flat list
/// does, so a badge saying one filter was active would be a plain lie.
///
/// The three `stats*` keys are excluded for the reason `repo` is excluded
/// from the triage chips below: they are sidebar NAVIGATION, not filters
/// narrowing a list. They live in `pr-stats`'s filter set because that is
/// where the per-view store keeps a view's selection, and this bar never
/// renders on that view -- but a count that would be wrong if it ever did
/// is a trap, and excluding them costs a line.
export function activeFilterCount(filters: Filters): number {
  const navigation = new Set<keyof Filters>([
    "statsScopeKind",
    "statsScopeValue",
    "statsSubject",
  ]);
  return (Object.entries(filters) as [keyof Filters, unknown][]).filter(([key, value]) => {
    if (
      key === "query" ||
      key === "sort" ||
      key === "readySort" ||
      key === "adviceGrouping" ||
      navigation.has(key)
    )
      return false;
    if (Array.isArray(value)) return value.length > 0;
    return value !== undefined && value !== false;
  }).length;
}

/// Blocked on the author and nobody else: a real conflict, or failing CI.
/// `checking` is deliberately excluded -- GitHub reports UNKNOWN
/// mergeability while it computes, and treating that as a conflict would
/// fire a false warning on every push. This mirrors the equivalent rule in
/// the Rust mapping layer -- keep the two consistent.
export function needsAttention(pr: PullRequest): boolean {
  return pr.merge === "conflicted" || pr.ci === "failure";
}

/// Reviewers who have been asked and have not yet answered.
///
/// `requested_reviewers` alone is not the answer: GitHub keeps a
/// reviewer in that list after they respond in some workflows, and a
/// re-request after a change puts an already-approved reviewer back
/// into it. Subtracting anyone with a recorded verdict is what turns
/// "who was asked" into "who you are actually waiting on" -- the only
/// version worth putting on a row.
///
/// A COMMENTED review counts as answered. They looked and said
/// something without blocking; chasing them is not what the row should
/// suggest.
export function pendingReviewers(pr: PullRequest): string[] {
  return pendingReview(pr).names;
}

/// Whether a pull request's verdict list arrived whole.
///
/// `latest_reviews` is paged. When GitHub reports more verdicts than
/// arrived, the answered set is INCOMPLETE -- and every name this
/// function would otherwise subtract from could belong to someone who
/// has already approved.
///
/// `>` rather than a subtraction, the same care `ReviewThreads` takes:
/// the count and the nodes can come from slightly different moments, so
/// a total smaller than the list is legitimate and is not a shortfall.
function verdictsTruncated(pr: PullRequest): boolean {
  const fetched = pr.latest_reviews?.length ?? 0;
  // Absent falls back to `fetched`, which reads as "nothing missing" --
  // a snapshot cached before #1089 added the field must not be declared
  // truncated forever.
  //
  // Honest note, because a comment claiming more than it does is how a
  // guard rots: under this `>` the fallback is not what SAVES the case.
  // `?? 0` would answer identically, since `0 > fetched` is false for
  // any non-negative `fetched`. The comparison is the real protection;
  // the fallback states the intent. Sabotage-proved during #1089 --
  // swapping it for `?? 0` failed nothing, which is why this says so
  // rather than pretending the line is load-bearing.
  return (pr.latest_reviews_total ?? fetched) > fetched;
}

/// Who you are waiting on, and whether that is the whole answer.
///
/// # Why this is not just a list
///
/// `pendingReviewers` subtracts the answered from the asked. The
/// answered set is built from `latest_reviews`, which is PAGED -- so on
/// a pull request with more verdicts than the window, a reviewer who has
/// already APPROVED never enters the set and is named as someone you are
/// still waiting on (#1089).
///
/// That is a wrong answer, not a short list: the row names a person and
/// tells you to chase them, when they are done. It is strictly worse
/// than saying nothing.
///
/// # What each field means
///
/// - `names` -- who to chase, and EMPTY when they cannot be vouched for.
///   The repo's rule is "qualify, or suppress": only-low qualifies,
///   possibly-wrong suppresses. Every name is possibly-wrong once the
///   verdicts were cut, so the list is emptied rather than printed with
///   a caveat. Suppressing in the data rather than in the renderer is
///   deliberate -- a correctness rule enforced at one call site is
///   enforced nowhere.
/// - `certain` -- whether `names` is the whole answer. False means the
///   verdict list was cut and the names were withheld, so a caller can
///   say a review is outstanding without saying whose.
/// - `atLeast` -- how many reviewers are outstanding, as a FLOOR.
///   `requested_reviewers` and `assignees` are paged too, so counting
///   the names that arrived gives a precise-looking number computed from
///   truncated input: `slice(0, 2)` was honest about its own cut and
///   silently inherited a dishonest one (#1089). Reviewers beyond the
///   window cannot have answered either, so they are counted here.
/// - `exact` -- whether `atLeast` is the true total rather than a floor.
export function pendingReview(pr: PullRequest): {
  names: string[];
  certain: boolean;
  atLeast: number;
  exact: boolean;
} {
  const answered = new Set(pr.latest_reviews?.map((r) => r.author) ?? []);
  // Assignees are a FALLBACK, not an addition. Some repositories assign
  // the reviewer rather than requesting a review -- measured: 25 of 25
  // rust-lang/rust pull requests have no `reviewRequests` at all, and
  // reading assignees rescues 19 of them.
  //
  // Only when there are no requests, never merged with them: on a repo
  // that uses both, an assignee is often the AUTHOR triaging their own
  // pull request, and listing them as someone you are waiting on would
  // be worse than saying nothing.
  const requested = pr.requested_reviewers ?? [];
  const usingRequests = requested.length > 0;
  const source = usingRequests ? requested : (pr.assignees ?? []);
  const outstanding = source.filter((login) => !answered.has(login) && login !== pr.author);

  // How many the SOURCE list claims, so a "+N" is not computed from a
  // list that was cut. Absent is not zero: a snapshot cached before
  // these totals existed falls back to the list's own length.
  const sourceTotal =
    (usingRequests ? pr.requested_reviewers_total : pr.assignees_total) ?? source.length;
  // Everyone the window did not show is unaccounted for, so count them
  // as still outstanding: they are by definition not in `answered`
  // either. This makes `atLeast` a floor rather than a guess.
  const unseen = Math.max(0, sourceTotal - source.length);

  // Truncated verdicts only matter when a name SURVIVED the subtraction.
  // With nobody left to chase there is nothing that could be wrong, and
  // warning about an empty list would put a caveat on every quiet row.
  const certain = outstanding.length === 0 || !verdictsTruncated(pr);

  return {
    // EMPTIED, not merely flagged, when the verdicts were cut.
    //
    // The flag alone would fix only the one caller that reads it. Every
    // other consumer of `pendingReviewers` -- and the next one somebody
    // writes -- would keep the wrong name, which is the shape of defect
    // this repo keeps re-learning: a correctness rule enforced in the
    // renderer is enforced nowhere. So the suppression lives in the
    // DATA. A name that might belong to someone who has already approved
    // is not a worse version of the answer, it is a different and wrong
    // one, and the honest value for it is nothing.
    //
    // Nothing is lost: `atLeast` still says how many are outstanding, so
    // the caller can report that a review IS pending without naming the
    // person it cannot vouch for.
    names: certain ? outstanding : [],
    certain,
    atLeast: outstanding.length + unseen,
    exact: unseen === 0,
  };
}

/// Waiting on a reviewer and nobody else.
///
/// `ci === "none"` counts, for the same reason `readyForReview` accepts
/// it: a repository with no checks configured has nothing to wait for.
/// Requiring `success` silently dropped those pull requests out of BOTH
/// triage chips -- they were not blocked on the author and not counted
/// as awaiting review either, so a repo showing 13 in the sidebar
/// offered chips adding to 7 with no explanation for the rest.
///
/// `pending` still does not count, also matching `readyForReview`: a
/// run in progress may go red, and "awaiting review" would be the wrong
/// thing to tell someone about a pull request that is about to need
/// their attention instead.
export function awaitingReview(pr: PullRequest): boolean {
  return (
    !pr.is_draft &&
    // A conflicted pull request is blocked on the AUTHOR, not on a
    // reviewer -- the diff a reviewer would read is not the diff that
    // will land. Without this, a conflicted PR with green CI counted in
    // BOTH chips at once, which is how "4 need attention · 3 awaiting
    // review" could describe overlapping sets and reconcile with
    // nothing. `readyForReview` already excludes conflicts for exactly
    // this reason.
    pr.merge !== "conflicted" &&
    (pr.ci === "success" || pr.ci === "none") &&
    (pr.review === "none" || pr.review === "review_required")
  );
}

export function readyToQueue(pr: PullRequest): boolean {
  return !pr.is_draft && pr.ci === "success" && pr.review === "approved" && !pr.in_merge_queue;
}

/// Ready to be reviewed right now.
///
/// The review queue's counterpart to `needsAttention`: what a reviewer
/// can pick up without wasting anyone's time.
///
/// Deliberately NOT `awaitingReview`, which asks whether the AUTHOR is
/// waiting on a review. This asks whether the review is worth GIVING
/// yet, which is a different question with a different answer on a
/// conflicted or queued pull request.
///
/// `ci === "none"` counts as ready: a repository with no checks
/// configured has nothing to wait for, and excluding it would empty
/// this section entirely for anyone not running CI. But `pending` does
/// NOT -- "ready" means the checks passed, not that they have not failed
/// yet, and a run in progress may still go red.
///
/// Conflicts exclude it because the diff a reviewer reads is not the
/// diff that will land. An existing review verdict excludes it because
/// this section is what is still WAITING.
export function readyForReview(pr: PullRequest): boolean {
  return (
    !pr.is_draft &&
    (pr.ci === "success" || pr.ci === "none") &&
    pr.merge !== "conflicted" &&
    pr.review !== "approved" &&
    pr.review !== "changes_requested" &&
    !pr.in_merge_queue
  );
}

/// The "Ready for review" strip's order, oldest OPENED first by default.
///
/// A review queue is the one list where oldest-first is the right
/// default: a pull request that has been waiting three days is the one
/// whose author is blocked, and newest-first buries it exactly when it
/// matters most. That is a different question from the session list and
/// the main PR list, which are newest-first so they answer "what was I
/// just doing" -- hence `readySort` rather than reusing `sort` (#1277).
///
/// Sorts on `created_at`, which is WHEN THE PULL REQUEST WAS OPENED, not
/// when it was marked ready for review. Those differ whenever something
/// sat in draft, and the second is the better answer to "how long has
/// this been waiting" -- but `PullRequest` carries `created_at` and
/// `updated_at` only, and no ready-for-review timestamp is fetched
/// anywhere in the app. Rather than guess, the control's labels name the
/// field ("Oldest opened first"), so a reader can tell which question
/// they are getting an answer to. Fetching `readyForReviewAt` is a
/// separate change.
///
/// A missing or unparseable `created_at` sorts LAST in both directions,
/// never first. `new Date("")` is `NaN`, and a `NaN` comparator result
/// is treated as 0 by `Array.prototype.sort` -- which leaves such a row
/// wherever it happened to be, including the top. At the top of a review
/// queue it would claim to be the longest-waiting work and push genuinely
/// old pull requests down, which is the one outcome this ordering exists
/// to prevent. Last is the honest place for "we do not know": it is
/// visible, and it is not making a claim.
///
/// Pure and non-mutating, like `sortPrs`.
export function sortReadyForReview(
  prs: PullRequest[],
  sort: NonNullable<Filters["readySort"]> = "oldest-opened",
): PullRequest[] {
  const opened = (pr: PullRequest) => new Date(pr.created_at).getTime();
  return [...prs].sort((a, b) => {
    const x = opened(a);
    const y = opened(b);
    // Undated rows go last regardless of direction, and keep a stable
    // order among themselves.
    const xBad = Number.isNaN(x);
    const yBad = Number.isNaN(y);
    if (xBad || yBad) return xBad && yBad ? 0 : xBad ? 1 : -1;
    return sort === "newest-opened" ? y - x : x - y;
  });
}

/// Needs MY attention as a reviewer.
///
/// Deliberately NOT `needsAttention`, which means blocked on you as the
/// AUTHOR: someone else's failing CI or merge conflict is not yours to
/// fix, and treating it as attention-worthy would badge the tray with
/// other people's broken builds.
///
/// What is actually yours: a review was requested, you have not given a
/// verdict yet, and it is not a draft. A PR you have already reviewed
/// stays in the list -- the author may still be responding -- but stops
/// demanding anything.
export function needsMyReview(pr: PullRequest): boolean {
  return !pr.is_draft && pr.review !== "approved" && pr.review !== "changes_requested";
}

/// A reviewer formally asked for changes.
///
/// Renamed from `blockedByComments`, which was a misnomer: this is a
/// review VERDICT, not comment resolution. The two are genuinely
/// different -- a PR can have six open conversations and no verdict, or a
/// changes-requested verdict with every thread resolved. Unresolved
/// conversations are counted separately in `pr.unresolved_threads`.
export function changesRequested(pr: PullRequest): boolean {
  return pr.review === "changes_requested";
}

/// Review conversations still open on the current code.
///
/// Not a claim that the PR is BLOCKED: whether a repo requires resolution
/// before merging is only readable with admin access on that repository,
/// so the UI reports the count and lets the reader draw the conclusion.
export function hasUnresolvedThreads(pr: PullRequest): boolean {
  return pr.unresolved_threads > 0;
}

/// Green, approved, and untouched for `days`+: the single most
/// nudge-worthy state, and the one no other filter surfaces.
export function isStale(pr: PullRequest, now: Date, days = STALE_DAYS): boolean {
  if (!readyToQueue(pr)) return false;
  const age = now.getTime() - new Date(pr.updated_at).getTime();
  return age > days * 86_400_000;
}

const hasLabel = (pr: PullRequest, names: string[]) =>
  pr.labels.some((l) => names.includes(l.name));

/// Whether any narrowing filter is active.
///
/// `repo` is excluded on purpose: it is sidebar navigation rather than a
/// filter chip (the store's `reset` treats it the same way), so a repo page
/// with no PRs should still explain what the app tracks rather than
/// blaming filters the user did not set.
export function hasActiveFilters(f: Filters): boolean {
  return Boolean(
    f.query ||
      f.unresolvedOnly ||
      f.needsMyReviewOnly ||
      f.readyOnly ||
      f.draftsOnly ||
      f.ci ||
      f.review ||
      f.includeLabels?.length ||
      f.excludeLabels?.length ||
      f.needsAttentionOnly ||
      f.staleOnly ||
      f.inMergeQueueOnly ||
      f.awaitingReviewOnly ||
      f.readyToQueueOnly,
  );
}

/// Does a PR match a free-text query?
///
/// Title, repository, and number, because those are what a person
/// remembers about a PR they are looking for. A bare "#123" or "123" both
/// match the number.
function matchesQuery(pr: PullRequest, q: string): boolean {
  const needle = q.trim().toLowerCase();
  if (needle === "") return true;
  const bare = needle.startsWith("#") ? needle.slice(1) : needle;
  return (
    pr.title.toLowerCase().includes(needle) ||
    pr.repo.toLowerCase().includes(needle) ||
    String(pr.number) === bare
  );
}

export function applyFilters(
  prs: PullRequest[],
  f: Filters,
  now: Date = new Date(),
): PullRequest[] {
  return prs.filter((pr) => {
    if (f.repo && pr.repo !== f.repo) return false;
    if (f.query && !matchesQuery(pr, f.query)) return false;
    if (f.unresolvedOnly && !hasUnresolvedThreads(pr)) return false;
    if (f.needsMyReviewOnly && !needsMyReview(pr)) return false;
    if (f.readyOnly && pr.is_draft) return false;
    if (f.draftsOnly && !pr.is_draft) return false;
    if (f.ci && pr.ci !== f.ci) return false;
    if (f.review && pr.review !== f.review) return false;
    if (f.includeLabels?.length && !hasLabel(pr, f.includeLabels)) return false;
    if (f.excludeLabels?.length && hasLabel(pr, f.excludeLabels)) return false;
    if (f.needsAttentionOnly && !needsAttention(pr)) return false;
    if (f.inMergeQueueOnly && !pr.in_merge_queue) return false;
    // Delegate to the same predicates deriveStats counts with, rather than
    // re-expressing "review is none OR review_required" or "approved AND
    // not already queued" as scalar equality filters -- a scalar `review`
    // field can only match one value at a time and has no way to express
    // "not in queue," so re-encoding these compound conditions as
    // combinations of existing scalar fields silently drops or admits PRs
    // the count did not (Task 17 fix round 1: cards 5 and 6 opened a list
    // that disagreed with their own numbers).
    if (f.awaitingReviewOnly && !awaitingReview(pr)) return false;
    if (f.readyToQueueOnly && !readyToQueue(pr)) return false;
    if (f.staleOnly && !isStale(pr, now)) return false;
    return true;
  });
}

/// PrList used to hardcode newest-first internally; that hid the ordering
/// from callers who need a different one (e.g. a future "least recently
/// updated" nudge view) and made it untestable independent of rendering.
/// Pure and non-mutating -- callers pass the result straight to PrList.
export function sortPrs(prs: PullRequest[], sort: Filters["sort"] = "newest"): PullRequest[] {
  const byCreated = (a: PullRequest, b: PullRequest) =>
    new Date(a.created_at).getTime() - new Date(b.created_at).getTime();
  const byUpdated = (a: PullRequest, b: PullRequest) =>
    new Date(a.updated_at).getTime() - new Date(b.updated_at).getTime();

  switch (sort) {
    case "oldest":
      return [...prs].sort(byCreated);
    case "recently-updated":
      return [...prs].sort((a, b) => -byUpdated(a, b));
    case "least-recently-updated":
      return [...prs].sort(byUpdated);
    case "newest":
    default:
      return [...prs].sort((a, b) => -byCreated(a, b));
  }
}

/// Five of the seven `Stats` fields come from the PR list already in
/// memory, at zero API cost -- the Rust `get_stats` command always returns
/// zero for these because they are not derivable from a single GitHub
/// search. Only `merged_week`/`merged_month` are real and stay Rust-side.
/// Task 17's dashboard must render this, not `get_stats` directly.
export function deriveStats(
  prs: PullRequest[],
): Omit<Stats, "merged_week" | "merged_month"> {
  return {
    in_merge_queue: prs.filter((p) => p.in_merge_queue).length,
    needs_attention: prs.filter(needsAttention).length,
    awaiting_review: prs.filter(awaitingReview).length,
    ready_to_queue: prs.filter(readyToQueue).length,
    blocked_by_comments: prs.filter(changesRequested).length,
  };
}

/// Which PRs sit on top of another open PR, and which one.
///
/// Keyed by `PullRequest.id` (the GraphQL node id, unique across repos)
/// and valued with the PARENT's number, so a row can say `on #1234`
/// rather than only that it is stacked at all.
///
/// Structural detection, deliberately (#743): a PR is stacked when its
/// `base_ref` is the `head_ref` of another open PR in the SAME repo.
/// `gh stack`, Graphite, `spr` and hand-made stacks all produce that
/// exact shape, so keying on any one tool's branch-naming convention
/// would recognise one workflow and quietly miss the other three -- and
/// would also invent stacks out of branches merely NAMED that way. Two
/// PRs matching head-to-base is a fact about the graph, not a guess
/// about the author's tooling.
///
/// This cannot be a per-row property, which is why it lives here rather
/// than beside `needsAttention`: resolving a parent needs the whole
/// list. `base_ref` alone tells a row its base is some branch; only the
/// list can say that branch is another open PR.
///
/// Scoped per repo because branch names are only unique within one --
/// without it, a `develop` open in two repositories would make each
/// repo's PRs read as stacked on the other's.
///
/// Note what this deliberately does NOT do: infer from the base name
/// alone. A base that is not the default branch may be a release train,
/// a long-lived integration branch, or a fork's `develop` -- none of
/// them stacks, and the list has no default branch to rule them out
/// (it is not fetched; see `src/types/pr.ts`). Requiring a matching open
/// PR is both the stricter test and the only one that can name the
/// parent. The cost is that a stack whose parent is filtered out of the
/// list, or has already merged, reads as standalone -- silence, which is
/// the right failure for a marker whose whole value is being trusted.
export function deriveStacked(prs: PullRequest[]): Map<string, number> {
  // head_ref -> the PR that opened it, per repo. Built once rather than
  // rescanning the list per row: this runs on every render, and GitHub
  // caps a search at 100 per query but nothing caps the total across the
  // repositories a user has open.
  const byHead = new Map<string, PullRequest>();
  for (const pr of prs) {
    if (pr.head_ref) byHead.set(`${pr.repo} ${pr.head_ref}`, pr);
  }

  const stacked = new Map<string, number>();
  for (const pr of prs) {
    if (!pr.base_ref) continue;
    const parent = byHead.get(`${pr.repo} ${pr.base_ref}`);
    // The identity guard is for the degenerate case of a PR whose head
    // and base are the same branch. GitHub does not create those, but
    // the list is whatever the API returned, and a row reading "on #42"
    // while being #42 is worse than no marker at all.
    if (parent && parent.id !== pr.id) stacked.set(pr.id, parent.number);
  }
  return stacked;
}
