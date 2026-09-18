/// TypeScript mirrors of the Rust model in `src-tauri/src/github/model.rs`.
/// Field names and enum values are wire-format, not TS convention: serde
/// renames `CiState`/`MergeState` to lowercase and `ReviewState` to
/// snake_case, and every struct field is already snake_case. Do not
/// "clean up" the casing here -- it must match what `invoke()` actually
/// receives, byte for byte.

export type CiState = "success" | "failure" | "pending" | "none";
export type MergeState = "mergeable" | "conflicted" | "checking";
export type ReviewState = "approved" | "changes_requested" | "review_required" | "none";

export interface Label {
  name: string;
  color: string;
}

export interface PullRequest {
  /// GraphQL node ID, so a row can act without opening the detail view.
  id: string;
  number: number;
  title: string;
  url: string;
  repo: string;
  author: string;
  is_draft: boolean;
  /// The branch being merged, and the branch it merges into.
  head_ref: string;
  /// The head commit the row was rendered from, so an "update branch"
  /// click can tell GitHub which commit the user was looking at.
  head_oid: string;
  /// The head branch's Ref node id, for deleting it after merge. `null`
  /// once the branch is gone -- which is how the UI tells "already
  /// cleaned up" from "still there".
  head_ref_id: string | null;
  base_ref: string;
  created_at: string;
  updated_at: string;
  ci: CiState;
  merge: MergeState;
  /// GitHub's own merge-readiness summary.
  ///
  /// Richer than `merge`, which only distinguishes conflicts. `clean` is
  /// what makes a merge button honest: any other value means GitHub would
  /// reject or block the merge. Inlined rather than exported as a named
  /// type, since nothing imports the name.
  merge_status:
    | "clean"
    | "dirty"
    | "blocked"
    | "unstable"
    | "behind"
    | "draft"
    | "unknown";
  review: ReviewState;
  in_merge_queue: boolean;
  labels: Label[];
  comment_count: number;
  /// Review conversations still open on the current code. Resolved and
  /// outdated threads are excluded.
  unresolved_threads: number;
  /// Logins whose review is still outstanding.
  ///
  /// Empty is ORDINARY: repositories that assign reviewers through a
  /// bot return nothing here, and so does a solo account.
  requested_reviewers: string[];
  /// Assignees, used as a fallback when no reviewer was requested.
  assignees: string[];
  /// Who has already reviewed, and what they said.
  latest_reviews: { author: string; state: string }[];
  /// How many entries each of the four lists above has, which those
  /// lists can be short of (#1089).
  ///
  /// NOT a bare `totalCount` -- see `connection_total` in `map.rs`. It
  /// counts the window's cut but not the mapper's drops, so a Team
  /// reviewer the row cannot name shrinks the list rather than reading
  /// as a person the window hid. Never 0 against a non-empty list: a 0
  /// would render "3 of 0", and absent is not zero.
  ///
  /// `latest_reviews_total` is the one that changes an ANSWER rather
  /// than a count: `pendingReviewers` subtracts the answered from the
  /// asked, so a reviewer beyond the window is named as still pending
  /// when they have already approved.
  requested_reviewers_total: number;
  assignees_total: number;
  latest_reviews_total: number;
  labels_total: number;
}

/// `merged_week`/`merged_month` are real. The other five derived fields
/// always come back zero from the Rust layer today -- Task 13 derives them
/// client-side from the PR list. Typed here so callers get the shape right;
/// do not rely on their values.
export interface Stats {
  merged_week: number;
  merged_month: number;
  in_merge_queue: number;
  needs_attention: number;
  awaiting_review: number;
  ready_to_queue: number;
  blocked_by_comments: number;
}

/// One day of PR activity. `date` is `YYYY-MM-DD` in UTC, matching the
/// GitHub search qualifiers the counts come from.
export interface HistoryPoint {
  date: string;
  opened: number;
  merged: number;
}

/// The daily series plus the period comparisons that drive the delta cards.
///
/// Every period window ENDS YESTERDAY: today is still accumulating, and
/// comparing a partial day against complete periods drags every delta
/// downward. `points` still includes today, because the chart's shape is
/// informative even when the last bar is short.
export interface History {
  points: HistoryPoint[];
  week_current: number;
  week_previous: number;
  opened_week_current: number;
  opened_week_previous: number;
  month_current: number;
  month_previous: number;
}

/// The period comparisons alone. Fetched separately from the daily series
/// so the delta cards can render while the chart is still loading.
export interface Periods {
  week_current: number;
  week_previous: number;
  opened_week_current: number;
  opened_week_previous: number;
  month_current: number;
  month_previous: number;
}

export interface RepoCount {
  repo: string;
  merged: number;
}

/// Aggregates over a SAMPLE of recently merged PRs, not a lifetime census
/// -- `sample_size` is how many were actually examined, and the UI labels
/// the figures with it. `cycle_time_hours` is sorted ascending so
/// `percentile()` can index it directly.
/// One merged PR, enough to name and open it.
export interface MergedPr {
  number: number;
  title: string;
  url: string;
  repo: string;
  cycle_time_hours: number;
  size: number;
}

export interface MergedDetail {
  cycle_time_hours: number[];
  /// additions+deletions per PR, sorted ascending for percentile lookup.
  pr_sizes: number[];
  additions: number;
  deletions: number;
  changed_files: number;
  comment_count: number;
  sample_size: number;
  repo_counts: RepoCount[];
  slowest: MergedPr[];
  largest: MergedPr[];
}

/// Median cycle time this week against last.
///
/// `sampled` is true when either window held more merges than GitHub
/// returns in one page (100), meaning the medians describe a sample of
/// that week rather than all of it.
export interface CycleTrend {
  current_hours: number;
  previous_hours: number;
  current_count: number;
  previous_count: number;
  sampled: boolean;
}

/// A cached pull-request list, and whether it is too old to present as
/// current.
///
/// `stale_secs` is null for a snapshot inside the freshness window --
/// the ordinary case, shown with no marker. A number means the rows were
/// true that many seconds ago and the view must say so.
///
/// This type exists because "too old to trust" and "there is nothing
/// here" used to be the same value, an empty array (#742). The list then
/// rendered a confident "nothing awaits your review" for as long as the
/// live fetch took -- seventeen seconds on the account that reported it.
export interface CachedSnapshot {
  prs: PullRequest[];
  stale_secs: number | null;
}

/// What is known about a worktree's lock, beyond the fact of it.
///
/// Mirrors `worktrees::model::Lock` on the Rust side. Every field
/// exists because the lock REASON, which #753 carried alone, turned out
/// not to answer the question a user has. Measured on the reporting
/// machine: 20 of 44 worktrees locked, every lock naming the same pid,
/// that pid alive only because it is the long-lived parent of workers
/// that finished days ago, and `lsof -d cwd` finding nothing at work in
/// any of them.
export interface Lock {
  /// Git's own lock reason, verbatim, or null for a lock taken without
  /// `--reason`. Still the locker's own words; it has simply stopped
  /// being the headline.
  reason: string | null;
  /// Whole days since the lock was taken, or null if unreadable.
  ///
  /// The field that actually discriminates, and the one the row leads
  /// with. Measured from git's `locked` file rather than from the
  /// `start` date inside the reason -- that one dates the process, and
  /// is identical across all 20 locks on the reporting machine, where
  /// the real ages span four days.
  age_days: number | null;
  /// Whether the pid named in the reason is running, or null when the
  /// reason names no pid.
  ///
  /// Weak evidence deliberately kept weak. True for all 20 locks on the
  /// reporting machine, every one abandoned, so the UI must never spend
  /// it as proof of a live claim. A false is the one decisive signal
  /// here: the named holder is gone.
  holder_running: boolean | null;
  /// What this worktree would be if the lock were cleared.
  ///
  /// The reason unlocking stops being a leap (#775). DISPLAY ONLY: the
  /// verdict that governs the button is still `locked`, and `isSafe`
  /// never looks inside -- a locked worktree that is merged underneath
  /// is still locked.
  underlying: Safety;
}

/// Why a worktree can or cannot be removed.
///
/// An enum rather than a boolean because the UI has to explain itself:
/// "3 uncommitted files" is actionable where a greyed-out button is not.
/// `never_pushed` is the dangerous one -- 52 of 295 worktrees on this
/// machine have no upstream, so their commits exist nowhere else.
export type Safety =
  | { kind: "safe" }
  | { kind: "main_checkout" }
  | { kind: "dirty"; detail: number }
  | { kind: "unpushed"; detail: number }
  | { kind: "never_pushed" }
  /// Merged, but the remote branch was deleted afterwards -- the usual
  /// end state of a squash-merged PR whose branch GitHub tidied up
  /// (#732). Removable: the work is on the default branch. Separate from
  /// `safe` so the row can say which evidence it used, because this one
  /// cannot be re-checked against a remote that no longer exists.
  | { kind: "merged_upstream_deleted" }
  /// A branchless checkout whose HEAD is already contained in the default
  /// branch (#819). Removable.
  ///
  /// `detail` is what the sha resolves to in ref-relative terms --
  /// `v1.13.0~30` -- or the bare word "detached" when no ref reaches it.
  /// That string is what makes the row actionable: "detached at
  /// v1.13.0~30" identifies the checkout, where "detached" only says what
  /// it lacks.
  ///
  /// Its own kind rather than `safe`, because `safe` means "merged,
  /// pushed" and there is no tracking config here to have established the
  /// second half from; and not `merged_upstream_deleted`, which
  /// specifically means the tracking config outlived the remote branch --
  /// evidence that never existed for a detached HEAD. These rows were
  /// `unknown` before #819, with no action at all: four on the reporting
  /// machine, every one provably an ancestor of the default branch.
  | { kind: "detached_merged"; detail: string }
  /// The branch was created and never committed to -- a scratch
  /// worktree. Distinct from `never_pushed`, which claims commits exist
  /// only here: for a branch with none, that claim is false, and the
  /// row said it beside "0 commits ahead".
  | { kind: "empty" }
  | { kind: "unmerged" }
  /// Someone locked the worktree, so `git worktree remove` refuses it
  /// whatever the branch's state (#753). 20 of 44 worktrees on the
  /// reporting machine are locked -- 45% of the list, not an edge case.
  ///
  /// `detail` grew from a bare reason string to a `Lock` in #775. The
  /// reason alone was meant to separate a live claim from a leftover
  /// one and measurably does not: every lock there names the same pid,
  /// that pid is alive because it is the surviving parent session
  /// rather than the worker that took the lock, and the `start` date
  /// embedded in the reason is identical on all 20 for the same reason.
  /// `Lock` carries the evidence that does discriminate.
  | { kind: "locked"; detail: Lock }
  /// The directory is gone and git knows the registration is stale;
  /// `git worktree prune` clears it. `detail` is git's reason. Formerly
  /// reported as `unknown: directory is missing`, which read as
  /// corruption rather than as resolvable bookkeeping (#753).
  | { kind: "prunable"; detail: string }
  /// The repository that owned this worktree is gone, so nothing about
  /// the checkout can be classified -- there is no git to run in it.
  | { kind: "orphaned" }
  /// Listed, but not yet classified. Distinct from `unknown`, which
  /// means the check ran and could not decide.
  | { kind: "pending" }
  | { kind: "unknown"; detail: string };

/// How a checkout stands against its tracked upstream, as of the last
/// fetch. Never live -- the scan reads refs on disk and does not fetch.
export type Upstream =
  | { kind: "current" }
  | { kind: "ahead"; n: number }
  | { kind: "behind"; n: number }
  | { kind: "diverged"; n: [number, number] }
  | { kind: "untracked" }
  | { kind: "detached" }
  | { kind: "unknown"; n: string };

export interface Worktree {
  path: string;
  branch: string;
  head: string;
  size_bytes: number | null;
  safety: Safety;
  is_main: boolean;
  /// `YYYY-MM-DD` when this branch landed in the default branch, when it
  /// can be determined. The date the work REACHED the default branch, not
  /// the branch tip's own commit date -- those diverge for a branch
  /// written weeks before it merged.
  merged_at: string | null;
  /// How this checkout stands against its upstream, for every row.
  upstream: Upstream | null;
  /// RFC 3339 timestamp of the branch tip's own commit. Not `merged_at`,
  /// which is when the work reached the default branch.
  last_commit: string | null;
  /// Git's lock reason, `""` for a lock taken without one, or null when
  /// the worktree is not locked (#753).
  ///
  /// Optional in the TYPE so the many existing fixtures need not
  /// enumerate it; `undefined` reads the same as `null` at every use.
  /// The safety verdict is the load-bearing copy of this fact -- these
  /// raw fields exist so a row can show git's own words rather than
  /// re-derive them.
  locked?: string | null;
  /// Git's prunable reason, or null when the registration is live.
  prunable?: string | null;
}

export interface WorktreeRepo {
  /// `owner/repo` from the git REMOTE, not the directory name -- this
  /// app's own directory is `ghstat` while its repository is
  /// `pktstorm/headstate`. `null` when there is no remote to ask.
  identity: string | null;
  name: string;
  path: string;
  worktrees: Worktree[];
  /// A repository with no working tree -- a bare clone or mirror
  /// (#1142). Optional because it is `#[serde(default)]` on the Rust
  /// side, so a cached scan written before this field existed
  /// deserialises rather than failing.
  bare?: boolean;
  /// When this repository's remote refs were last fetched, RFC 3339, or
  /// null if never fetched or unreadable.
  ///
  /// Every merge and upstream verdict below is computed against refs
  /// already on disk -- the scan never goes to the network on purpose.
  /// This is what lets the view say how old those answers are (#702).
  ///
  /// Optional in the TYPE so a fixture need not enumerate it, and
  /// `undefined` reads the same as `null` at every use: both mean the
  /// age is unknown, which is what the UI must say. The Rust side
  /// always sends the key.
  fetched_at?: string | null;
  /// The ref this repository's verdicts are measured against --
  /// `origin/main`, `origin/master`, or a bare local branch name where
  /// no remote-tracking ref resolves (#757, #1026).
  ///
  /// Mirrors `Repo::default_ref`. Carried rather than re-derived: a
  /// hardcoded `origin/main` is wrong for over 10% of the repositories
  /// on the reporting machine, and a fifth `default_branch` is exactly
  /// what `invariants.rs` guards against.
  ///
  /// `null` means the resolution did not happen -- an orphaned worktree
  /// has no repository to ask. Render that as unknown; substituting
  /// `main` would be a confident answer about which branch was compared,
  /// which is the one thing this field exists to stop.
  ///
  /// Optional in the TYPE so the existing fixtures need not enumerate
  /// it, and `undefined` reads the same as `null` at every use. The Rust
  /// side always sends the key.
  default_ref?: string | null;
}

/// What a worktree scan found, INCLUDING what it could not read (#951).
///
/// Mirrors `RepoScan` in `src-tauri/src/worktrees/scan.rs`. A repository
/// whose worktree listing failed used to be dropped from the payload, so
/// the page read it as "not a repository" -- and `RepoPickerSidebar` then
/// rendered "No repositories found in the scanned folders", a DIAGNOSIS
/// pointing at settings that were fine.
export interface WorktreeScan {
  repos: WorktreeRepo[];
  /// Paths the walk could not read, each with WHY.
  ///
  /// A message rather than a count, because "not a repository" and
  /// "permission denied" send the user to different places. Non-empty
  /// means the repo list beside it is a FLOOR, and an orphan count taken
  /// over it is not a verdict.
  ///
  /// Optional in the TYPE so a fixture need not enumerate it, and
  /// `undefined` reads the same as `[]` at every use. The Rust side
  /// always sends the key.
  unreadable?: string[];
}

/// One entry in a repository directory listing (#1031).
///
/// Mirrors `Entry` in `src-tauri/src/repos/mod.rs`. The listing comes
/// from the git INDEX rather than from `readdir`, measured at 928x fewer
/// entries in this repository's own checkout -- 672 tracked against
/// 623,488 on disk -- so what arrives here is the repository rather than
/// the build directory.
export interface RepoEntry {
  /// The entry's own name, with no path in it.
  name: string;
  /// The repository-relative path, which is what goes back to the
  /// commands to descend or to read.
  path: string;
  /// A directory to descend into.
  dir: boolean;
  /// Tracked by git as a symbolic link (mode `120000`).
  ///
  /// SHOWN rather than followed, which is what the GitHub code view does
  /// too. Measured across the 38 repositories on the development
  /// machine: 22 tracked symlinks is the ENTIRE population a browser
  /// listing from the index can ever display, 2 already broken, and 0
  /// resolving outside their own repository -- so following them would
  /// buy 20 working links and cost the containment guard its whole
  /// property.
  symlink: boolean;
  /// Where the link points, verbatim, or absent for anything else.
  ///
  /// The link's own text rather than a resolved path, because that is
  /// the fact the row exists to convey. 14 of the 22 are shared
  /// Terraform module files, where the target is exactly the thing the
  /// user opened the row to learn -- a link shown without one tells them
  /// less than the filename already did.
  target?: string;
  /// Whether the link points at a DIRECTORY.
  ///
  /// The 22 split 14 files / 6 directories / 2 broken, and the split has
  /// a UI consequence. A symlinked file can explain itself in the panel
  /// on click, in the slot the binary refusal uses; a symlinked
  /// DIRECTORY has no panel to explain itself in, so the row must carry
  /// it -- otherwise the row looks descendable and does nothing when
  /// clicked, which reads as broken.
  symlink_to_dir?: boolean;
}

/// One directory level of a repository, from the git index (#1031).
///
/// `entries` being empty means git listed the directory and it holds no
/// tracked files -- a real answer. A directory that could not be LISTED
/// rejects instead, and the two must render differently (#1036, #846).
export interface RepoTree {
  /// The repository-relative path listed, `""` for the root. Echoed back
  /// so a response cannot be rendered against the wrong request.
  path: string;
  entries: RepoEntry[];
}

/// One file's bounded contents (#1033).
///
/// Mirrors `FileRead` in `src-tauri/src/repos/mod.rs`. Three outcomes to
/// render distinctly, and a rejection is a fourth: an `Err` means the
/// file could not be READ; `binary` means it was read and is not text;
/// and empty `content` with `binary` false means the file is genuinely
/// empty, of which there are real ones (`.gitkeep`).
export interface RepoFile {
  path: string;
  /// The file's REAL size, not the window's. Read by `stat` before the
  /// file is, so the bound is a bound rather than a discard.
  size: number;
  /// The text, or empty when `binary`.
  content: string;
  /// Whether the 256 KB bound cut it short. Stated, never silent -- a
  /// window shown as if it were the whole file is worse than a refusal.
  truncated: boolean;
  /// Whether a NUL byte in the head makes this not text. Not an error;
  /// the GitHub code view says exactly this about a binary.
  binary: boolean;
}

/// Everything the detail view renders.
///
/// Separate from `PullRequest`, which is a list row fetched 100 at a time
/// on a poll loop -- carrying a body and comments there would make every
/// tick haul data almost no row needs.
/// One review conversation on a pull request.
export interface ReviewThread {
  /// The thread's node id, which the resolve and reply commands take --
  /// NOT the pull request's id.
  id: string;
  is_resolved: boolean;
  /// Whether the anchored line still exists after a force-push.
  ///
  /// Not the same question as resolved: an outdated thread can still hold
  /// an unanswered question, so the UI must never present "the code moved"
  /// as "this was dealt with".
  is_outdated: boolean;
  path: string;
  /// Null once the anchor is gone, which is when `is_outdated` is true.
  /// Render the path alone rather than `file.ts:null`.
  line: number | null;
  /// What THIS viewer may do, per thread. Separate permissions because
  /// GitHub grants them separately; a button shown without its permission
  /// fails with a 403 on click.
  viewer_can_reply: boolean;
  viewer_can_resolve: boolean;
  viewer_can_unresolve: boolean;
  comments: { author: string; created_at: string; body: string }[];
  /// The true total, which can exceed `comments.length` -- the query
  /// pages thread comments at 10.
  comment_count: number;
}

export interface PrDetail {
  /// GraphQL node ID. Every mutation takes this rather than a number, so
  /// a write can only follow a read of the thing being written.
  id: string;
  number: number;
  title: string;
  url: string;
  state: string;
  is_draft: boolean;
  body: string;
  author: string;
  repo: string;
  head_ref: string;
  /// The head commit the row was rendered from, so an "update branch"
  /// click can tell GitHub which commit the user was looking at.
  head_oid: string;
  head_ref_id: string | null;
  base_ref: string;
  merge_status: string;
  review: string;
  /// Every reviewer's latest review state, keyed by login.
  ///
  /// A different question from `review`, which is the pull request's
  /// AGGREGATE decision: it reads "changes_requested" when someone else
  /// blocked it. Matching the viewer's login against this is the only
  /// way to answer "did MY approval land".
  latest_reviews: { author: string; state: string }[];
  /// Whether this pull request's base branch uses a merge queue.
  ///
  /// Chooses between Merge and Add to merge queue, so the user is not
  /// asked to pick between two buttons only one of which can work.
  merge_queue_enabled: boolean;
  /// Whether it is currently queued (and not rejected by the queue).
  in_merge_queue: boolean;
  additions: number;
  deletions: number;
  changed_files: number;
  unresolved_threads: number;
  comment_count: number;
  comments: { author: string; created_at: string; body: string }[];
  /// The review conversations -- inline threads anchored to a file and
  /// line. A DIFFERENT object from `comments` above, which are flat
  /// top-level comments: only threads can be resolved, so merging the two
  /// into one list would imply a Resolve button on comments that have no
  /// such concept.
  review_threads: ReviewThread[];
  /// GitHub's own count of review threads, which `review_threads` can be
  /// SHORT of (#802).
  ///
  /// The query asks for the connection maximum of 100 and does not
  /// paginate (see `map_review_threads` for why a cursor loop was
  /// rejected), so above 100 threads the list arrives truncated. Read it
  /// the way `checks_total` is read: render what arrived, say what is
  /// missing. Before this existed the window was 20 and the shortfall was
  /// invisible, which let an unresolved blocking comment sit outside a
  /// view that looked complete.
  ///
  /// `unresolved_threads` is a FLOOR whenever this exceeds
  /// `review_threads.length` -- it is counted from the threads that
  /// arrived, and the total includes resolved and outdated ones so it
  /// cannot be used to correct it.
  ///
  /// Compare with `review_threads.length` using a saturating subtraction;
  /// a total below the length is possible and is not a negative
  /// shortfall.
  review_threads_total: number;
  /// `state` is `success`, `failure`, `pending`, `skipped`, or a raw
  /// GitHub value when unmodelled -- never coerced to success. Inlined
  /// rather than exported types, since nothing imports the names.
  checks: {
    name: string;
    state: string;
    url: string;
    /// The Actions workflow run, for re-running failed jobs. Null for a
    /// plain commit status or a non-Actions check -- neither can be
    /// re-run, so the button is offered only where this exists.
    run_id: number | null;
  }[];
  /// GitHub's own count of check contexts, which `checks` can be SHORT of.
  ///
  /// The Rust side pages the rollup up to a budget (#790 cut it from 20
  /// serial requests to 3, because that chain was the slow click), so a
  /// pull request with hundreds of contexts now arrives capped. Paired
  /// with `checks` the way `comment_count` is paired with `comments`, and
  /// read the same way: render what arrived, say what is missing.
  ///
  /// Compare with `checks.length` using a saturating subtraction -- the
  /// two numbers come from different pages of a rollup that can grow
  /// mid-fetch, so a total BELOW the length is possible and is not a
  /// negative shortfall.
  checks_total: number;
}

/// How an image's provenance was established. A recorded fact and a
/// resolved guess should not look identical in the UI.
type OriginSource = "build_history" | "tag_resolution";

interface DockerOrigin {
  repo_path: string;
  /// The build context, which for a worktree build IS the worktree.
  context: string | null;
  commit: string;
  subject: string;
  /// The branch landed, so nothing will ever want this image again.
  merged: boolean;
  source: OriginSource;
}

export interface DockerImage {
  id: string;
  repository: string;
  /// Every tag pointing at this ID -- `latest` and a SHA are one image.
  tags: string[];
  created: string;
  size_bytes: number;
  origin: DockerOrigin | null;
  /// `null` means we could not ask -- NOT "nothing is using it". An
  /// unknown answer renders as not-removable.
  in_use: boolean | null;
  superseded: boolean;
  /// Another image shares this repository, newer or older.
  ///
  /// Separates "the newest of several" from "the only one there is" --
  /// with one image per repository nothing can ever be superseded, so
  /// `current` appeared on every row and discriminated nothing.
  has_siblings: boolean;
}

export interface DockerDiskUsage {
  images_bytes: number;
  images_reclaimable_bytes: number;
  build_cache_bytes: number;
  volumes_bytes: number;
  volumes_reclaimable_bytes: number;
}

/// Docker is frequently OFF, unlike git. "We could not ask" is not "the
/// answer is zero".
export type DockerState =
  | { kind: "running" }
  | { kind: "not_running" }
  | { kind: "permission_denied" }
  | { kind: "not_installed" }
  | { kind: "unknown"; detail: string };

export interface DanglingVolume {
  name: string;
  size_bytes: number;
}

export interface ImageRemovalOutcome {
  id: string;
  error: string | null;
}

export interface DockerBuild {
  reference: string;
  name: string;
  /// `null` when buildx did not report one. Never `""`: an empty string
  /// is `!== "Completed"` and therefore read as a FAILED build (#963).
  status: string | null;
  started: string;
  duration_secs: number;
  /// `null` when buildx did not report the counts. Never 0, which
  /// `cachePercent` answers as "0% cached" -- the strongest alarm this
  /// figure raises, fabricated from an absent field (#963).
  total_steps: number | null;
  cached_steps: number | null;
  /// Resolved on demand: `inspect` is a subprocess per build.
  context: string | null;
  revision: string | null;
}

/// What the app already knows about a worktree's unmerged work.
///
/// Mirrors the Rust `Assessment`. Every field was already computed for
/// the Claude Code handoff and then discarded except the shell string.
export interface Assessment {
  path: string;
  branch: string;
  commits_ahead: number | null;
  files_changed: number | null;
  insertions: number | null;
  deletions: number | null;
  /// Relative, as git prints it: "3 weeks ago".
  last_activity: string | null;
  /// Never pushed means these commits exist only on this machine.
  ///
  /// Three states, not two (#976). `null` means git could not be asked
  /// whether there is an upstream -- missing from a GUI-launched app's
  /// PATH, refusing on `safe.directory` -- which is not the same claim
  /// as "this branch was never pushed".
  has_upstream: boolean | null;
  /// `null` when `git log` could not be read at all, which is not the
  /// same as a branch with no commits (`[]`).
  subjects: string[] | null;
  subjects_elided: number;
  /// Uncommitted paths in the working tree, or `null` when `git status`
  /// could not be read. Never coerced to 0: a fabricated zero here reads
  /// as a clean tree (#976).
  uncommitted: number | null;
  /// The ref the counts above were measured against, by name:
  /// `origin/main` with a remote, a bare `main` on a purely local repo.
  ///
  /// Carried so the Claudify prompt can NAME it. It used to say "the
  /// default branch", which an agent is free to resolve as the local
  /// `main`, the merge-base, or the remote ref -- three answers, one of
  /// which produced these numbers (#815).
  base: string;
  /// When this repository's remote refs were last fetched, RFC 3339, or
  /// `null` for never/unreadable.
  ///
  /// Nothing on the assessment path fetches, so every count here is only
  /// as current as this instant. `refAge` says so on the page; the
  /// prompt now says so to the agent (#815).
  fetched_at: string | null;
}

/// What kind of build output a directory holds.
///
/// Mirrors `ArtifactKind` in Rust. The membership rule is that a
/// documented command rebuilds it -- which is what makes removal cost a
/// rebuild rather than work, and why this is a closed set rather than a
/// user-supplied pattern.
export type ArtifactKind =
  | "cargo_target"
  | "node_modules"
  | "terraform"
  | "dotnet_build"
  | "build_output";

/// One directory of regenerable build output.
export interface Artifact {
  /// Absolute path. Removal takes this, never a name matched by pattern.
  path: string;
  kind: ArtifactKind;
  /// The checkout it belongs to, for grouping.
  repo_path: string;
  /// Bytes on disk, or null until measured.
  ///
  /// Discovery and sizing differ by three orders of magnitude (measured:
  /// ~1.5s to find 178 directories, ~56s to size them), so the list
  /// renders before this is known. Null rather than 0: "not measured
  /// yet" and "empty" are different facts, and showing 0 B for the
  /// former is a lie the user would act on.
  size_bytes: number | null;
}

/// The outcome of removing one artifact directory.
///
/// Per-directory rather than one verdict for the batch: a directory that
/// went active since the scan is refused while the rest succeed.
export interface ArtifactRemoval {
  path: string;
  /// Null on success. Shown verbatim -- it names WHY, and "could not
  /// remove" alone is not something a user can act on.
  error: string | null;
}

/// Why a Poetry virtualenv is reclaimable.
///
/// `unknown` is not a reason -- it is the absence of one. The project
/// walk that decides orphanhood stopped early, so this run cannot say
/// whether anything still owns the venv, and it is offered for removal
/// by neither the manual nor the unattended path (#747).
export type VenvState = "orphaned" | "stale" | "live" | "unknown";

/// One Poetry virtualenv.
export interface Venv {
  path: string;
  /// The project name Poetry encoded, e.g. `hello-world-delivery`.
  project: string;
  state: VenvState;
  /// The directory that produced it. Null for an orphan -- that IS the
  /// finding, not missing data.
  source: string | null;
  size_bytes: number | null;
  /// Seconds since the newest file INSIDE was written. Poetry touches a
  /// venv's root without writing inside, so its own mtime reports a
  /// year-old venv as days old.
  idle_secs: number | null;
}

export interface VenvRemoval {
  path: string;
  error: string | null;
}

/// One thing the automatic cleanup pass considered.
export interface LedgerEntry {
  at: string;
  /// `artifact` or `venv`.
  kind: string;
  target: string;
  /// An artifact's rebuild command, or a virtualenv's project.
  detail: string | null;
  bytes: number | null;
  /// `proposed`, `removed`, `refused`, or `skipped`.
  action: string;
  error: string | null;
}

/// Preferences for the automatic pass.
///
/// `mode` carries a `remove` variant so the stored shape does not change
/// in Phase 2, but this build refuses to store it: a setting that can be
/// turned on and does nothing is worse than one that does not exist.
export interface CleanupPrefs {
  enabled: boolean;
  mode: "preview" | "remove";
  artifacts: boolean;
  venvs: boolean;
  /// Whether an unattended pass may propose STALE virtualenvs, not just
  /// orphans. An orphan is a fact; stale is a threshold about a project
  /// that still exists, and that is what needs the opt-in here.
  venvs_stale: boolean;
  /// Merged branches. Parent of the two below.
  branches: boolean;
  /// Merged by ancestry — a graph fact, the strongest claim available.
  branches_ancestor: boolean;
  /// Merged by squash, found by comparing patch-ids. A CONTENT
  /// comparison rather than a graph one, so it gets its own opt-in for
  /// the same reason `venvs_stale` does — and it is the common case
  /// (489 of 536 on a real repository), so enabling it is not a small
  /// decision.
  branches_squash: boolean;
  worktrees: boolean;
  /// Merged, clean, and pushed — nothing is lost by removing one.
  worktrees_safe: boolean;
  docker: boolean;
  /// Untagged and referenced by nothing.
  docker_dangling: boolean;
  max_per_run: number;
}

export type Ecosystem =
  | "npm"
  | "yarn"
  | "poetry"
  | "uv"
  | "dotnet"
  | "cocoapods"
  | "terraform"
  | "swift"
  | "cargo";

/// How large a version jump is.
///
/// `unknown` is a real answer, not a fallback. Version schemes here are
/// not all semver -- .NET ships four parts, PEP 440 has epochs -- and a
/// version silently called major hides from a "minors only" filter while
/// one silently called minor is offered as safe.
export type Bump = "patch" | "minor" | "major" | "unknown";

export interface Outdated {
  name: string;
  current: string;
  latest: string;
  bump: Bump;
  ecosystem: Ecosystem;
  /// The manifest to edit, so an agent does not have to find it.
  manifest: string;
  /// Which PROJECT in the repository this row came from, relative to
  /// the repository root. Empty at the root.
  ///
  /// Attached by `PackagesPage` rather than sent by the backend: the
  /// grouping already knows it (`ProjectReport.label`) and flattening
  /// the groups for the wizard is what threw it away. An apply needs it
  /// -- in THIS repository every Rust row lives under `src-tauri` or
  /// `src-mobile` and there is no `Cargo.toml` at the root at all, so a
  /// request without it has nothing to edit.
  project?: string;
}

/// One package the user asked to update.
export interface UpdateRequest {
  name: string;
  version: string;
  ecosystem: Ecosystem;
  /// The project directory, relative to the repository root. Omitted or
  /// empty means the root itself, which is what every caller meant
  /// before this field existed.
  project?: string;
}

/// What happened to one requested update.
///
/// Per-package rather than one status for the run: updates apply in
/// sequence, and a failure in the third must not erase the report of the
/// two that worked.
interface UpdateOutcome {
  name: string;
  /// The version ASKED FOR.
  requested: string;
  /// Files git reports as changed. Empty means the command succeeded and
  /// changed nothing -- usually a manifest constraint pinning the
  /// package below the requested version, which is worth showing.
  changed_files: string[];
  /// The tool's own output, kept on success too: resolvers warn about
  /// peer conflicts while still succeeding.
  output: string;
  /// The constraint the manifest holds AFTERWARDS.
  ///
  /// Not the same as `requested`, and that is the point: npm rewrites a
  /// pinned `4.17.21` request into `^4.17.21`, a range rather than a
  /// pin. `null` when it could not be read, which is shown as unknown
  /// rather than assumed to match.
  resolved_constraint: string | null;
  /// Set when this package failed; the others still report.
  error: string | null;
}

/// The result of an update run.
export interface RunReport {
  /// Which ecosystems the run touched. Opening a pull request is only
  /// offered where the resolved constraint can be read back.
  ecosystems: Ecosystem[];
  /// The worktree holding the changes. Phase 1 does not push, so this
  /// path IS the deliverable.
  worktree: string;
  branch: string;
  results: UpdateOutcome[];
}

/// What one ecosystem reported for one repository.
///
/// `error` exists because "no updates" and "the check did not run" are
/// opposite answers, and rendering both as an empty list reports failure
/// as good news.
export interface EcosystemReport {
  ecosystem: Ecosystem;
  outdated: Outdated[];
  error: string | null;
}

export type UpdateFilter = "patch" | "minor" | "all";

/// One imported file in a CLAUDE.md tree.
export interface ImportNode {
  /// What the file wrote, verbatim.
  raw: string;
  /// Where it resolved to, when it did.
  path: string | null;
  bytes: number;
  tokens: number;
  /// Why this node is unusable, when it is. A broken or circular import
  /// is SHOWN rather than dropped -- omitting it makes the tree look
  /// complete when it is not.
  problem: string | null;
  /// Whether this node's weight could not be MEASURED (#972).
  ///
  /// Separate from `problem`, because only ONE of the three problems makes
  /// a total inexact. "file not found" and "circular import" both
  /// contribute a correct zero -- nothing to weigh, and already counted
  /// once respectively -- while "could not read" hides real weight. A page
  /// that inferred partiality from `problem` would put "at least" in front
  /// of two totals that are exact.
  unreadable: boolean;
  children: ImportNode[];
}

/// One CLAUDE.md and the tree it pulls in.
export interface ClaudeFile {
  path: string;
  bytes: number;
  /// ESTIMATED tokens for this file alone. Characters divided by four,
  /// not a real tokeniser -- every label says so.
  tokens: number;
  /// Estimated tokens for this file plus everything it imports. The
  /// number that matters: a 2 KB file pulling in 40 KB of imports is the
  /// case this view exists to surface.
  total_tokens: number;
  /// Whether `total_tokens` is a FLOOR rather than a value: some import's
  /// weight could not be counted, so the real total is higher by an
  /// unknown amount (#972). Rendered with the app's existing "at least"
  /// idiom.
  total_partial: boolean;
  imports: ImportNode[];
}

/// What a CLAUDE.md scan found, INCLUDING what it could not read (#972).
///
/// A bare `ClaudeFile[]` could not tell "this repository has none" from
/// "we could not look", so an unreadable file rendered as #846's own
/// sentence -- "No CLAUDE.md files in this repository" -- about a file on
/// disk. The `Scan` shape is `ClaudeCodePage`'s, one view over.
/// Which scope a CLAUDE.md came from (#1131). Mirrors
/// `claudemd::Scope`.
type ClaudeMdScope = "global" | "repo" | "local";

interface ClaudeMdScopedFile {
  scope: ClaudeMdScope;
  file: ClaudeFile;
}

/// The repo scan plus the scopes a session actually loads (#1131).
///
/// The repo page answered "what is in this repository", which is not the
/// context a session loads: `~/.claude/CLAUDE.md` goes into every
/// session on the machine, so the token total was short by that amount
/// with nothing saying so.
export interface ClaudeMdEffectiveScan {
  /// Unchanged from `scanClaudeMd`. Its totals keep their exact meaning.
  repo: ClaudeMdScan;
  extra: ClaudeMdScopedFile[];
  /// Scopes that exist and could not be read. Non-empty means the
  /// combined figure below is a floor.
  unreadable: string[];
}

export interface ClaudeMdScan {
  /// What DID read. Never blanked because something else did not.
  files: ClaudeFile[];
  /// Directories that could not be listed, with why. Each hides an
  /// unknown number of files.
  unreadable_dirs: string[];
  /// CLAUDE.md files proven to exist and not readable, with why.
  unreadable_files: string[];
  /// Directories deliberately not walked -- the skip list and the
  /// worktree prune. NOT failures: counted separately so a correct
  /// exclusion can never be mistaken for something going wrong.
  skipped_dirs: number;
}

/// Whether a Claude Code session's process is running (#917).
///
/// THREE states, never two, and the discriminated union is the point: a
/// consumer has to switch on `state` and cannot coerce this to a
/// boolean. `"unknown"` is what a check that could not be COMPLETED
/// returns -- an unreadable `~/.claude/sessions` (mode `0700`), an
/// unparseable `procStart`, or a session whose process we never watched,
/// which is the entire imported history.
///
/// Rendering `"unknown"` as "not running" is #841's fail-open: "not
/// running" is what offers Resume, and resuming a session that is in
/// fact alive starts a SECOND copy of it. `SystemHealthPage`'s
/// `HealthConditions` is the house pattern for keeping the three
/// distinct on screen.
export type Liveness =
  /// Alive, and its start time matches what was recorded -- so it is the
  /// same process and not a reused pid.
  | {
      state: "running";
      pid: number;
      /// `busy` / `idle` as the session last published it. A REFINEMENT
      /// of an answer already derived, never the answer itself: it is a
      /// stored status that a killed session never corrects. Carried
      /// only on `running`, so there is no way to show "busy" for a
      /// process we did not find.
      status: string | null;
    }
  /// Not running. `why` says which of the two ways we established it,
  /// because one of them -- an orphaned registry entry -- is a crash.
  | { state: "dead"; why: string }
  /// The check could not be completed. NOT a shade of `dead`.
  | { state: "unknown"; why: string };

/// Whether a path a session recorded still exists (#918, #919).
///
/// Tri-state for the same reason `Liveness` is: `"gone"` and
/// `"unknown"` have different remedies. A permission error means the
/// tree may well be there and the `cd` would have worked; `"gone"`
/// means it certainly is not.
///
/// Used for BOTH paths a session records, which survive at opposite
/// rates. Measured over 1,461 real sessions for #919:
///
/// ```text
/// cwd         exists  248  gone 1213   (83.0% gone)
/// transcript  exists 1461  gone    0   ( 0.0% gone)
/// ```
///
/// So `"gone"` is the NORMAL rendering for a cwd and must not look
/// broken, while for a transcript it is genuinely rare. The two states
/// are never derived from one another: 1,213 rows (83.0%) have a dead
/// cwd and a live transcript, and gating the transcript's action on the
/// cwd's state would disable the button that works on almost every row.
export type CwdState =
  | { state: "exists" }
  | { state: "gone" }
  | { state: "unknown"; why: string }
  /// No cwd was ever recorded. Distinct from `gone`: there is no path to
  /// report as missing.
  | { state: "not-recorded" };

/// The resume command to copy, and what to say before pasting it (#918).
///
/// Built on the backend because the existence check behind it is a
/// filesystem read the webview cannot do, and splitting the check from
/// the string it produces is how the two drift into a command whose
/// caveat no longer matches it.
/// Not exported: it is reached only through `ClaudeSession.resume`, and
/// `yarn knip` is right that a second name for the same shape earns
/// nothing. Exporting it the moment something else needs it is one word.
interface ResumeCommand {
  /// The text to copy. Always correct to run: `claude --resume <id>`
  /// resolves by id and works after the recorded directory is deleted.
  command: string;
  /// What the user must know before pasting, or `null` when there is
  /// nothing to warn about. Non-null for every case that omits the `cd`.
  caveat: string | null;
  /// Whether the command carries its own `cd`. Decides whether Resume
  /// is the primary action or a secondary one.
  anchored: boolean;
}

/// What a transcript rescan read, and what it could not read (#914).
///
/// The unreadable lists are the point: an empty session table with a
/// non-empty `unreadable_dirs` means "we could not read your history",
/// which must not render as "you have no sessions". Opposite remedies,
/// and the second is alarming when it is false.
export interface ClaudeImported {
  /// Sessions written (inserted or updated).
  sessions: number;
  /// Rows the database refused, with why.
  write_failures: string[];
  /// `.jsonl` files correctly excluded because they are subagent
  /// transcripts, not sessions. Just under half the corpus -- counted so
  /// the exclusion stays visible rather than invisible.
  subagent_files_skipped: number;
  /// Bytes across the session transcripts (#1135). Optional so a cached
  /// import from before this field existed deserialises rather than
  /// failing.
  session_bytes?: number;
  /// Bytes across the subagent transcripts, kept apart: roughly half the
  /// `.jsonl` files on disk are subagent ones, and the two mean
  /// different things.
  subagent_bytes?: number;
  /// Files whose size could not be read, making both totals FLOORS. A
  /// size we could not take is not a size of zero.
  unsized_files?: number;
  unreadable_dirs: string[];
  unreadable_files: string[];
  metadata_beyond_first_record: number;
  elapsed_ms: number;
  /// `~/.claude/projects` itself, when it does not exist (#970).
  ///
  /// The THIRD answer, and not one of the two above: a root that is not
  /// there is the honest empty result for a machine that has never run
  /// Claude Code, so it must not be counted among the things that could
  /// not be read. It used to arrive in `unreadable_dirs`, which is what
  /// put "0 sessions read, but 1 could not be — this list is incomplete by
  /// an unknown amount" above the empty list on a brand-new machine.
  ///
  /// `null` means the directory is there, whatever is in it. So a page
  /// with zero sessions and `absent_root === null` is a user who cleared
  /// their history, not one who has never run `claude`, and the two get
  /// different copy.
  absent_root: string | null;
}

/// A liveness exactly as it arrives on the wire, with its reason
/// interned (#985).
///
/// The same three states as `Liveness`; the only difference is that
/// `why` is an index into `ClaudeSessionList.reasons` rather than the
/// sentence. `hydrateClaudeSessions` in `api/hooks.ts` resolves it, and
/// the rest of the app only ever sees `Liveness`.
///
/// The reason is interned rather than dropped: the list RENDERS it, as
/// each row's `title`, so "Not running" always carries its grounds on
/// hover. It was 18.9% of the old payload and the same 150-character
/// sentence on 1,474 of 1,474 real rows -- paid for once here, preserved
/// exactly.
type WireLiveness =
  | { state: "running"; pid: number; status: string | null }
  | { state: "dead"; why: number }
  | { state: "unknown"; why: number };

/// Whether a session is waiting on the user, and how sure we are (#1067).
///
/// Rust side: `claude/signals.rs`, whose `Waiting` enum this mirrors
/// exactly, tag and kebab-case variant names included.
///
/// # Why this is three states and not a boolean
///
/// A notification is a POINT IN TIME. Between the record being written
/// and this row being drawn, the session may have been answered, exited,
/// or SIGKILLed -- and a SIGKILL leaves no record at all, so nothing ever
/// arrives to correct the file. A `waiting: boolean` on the wire would be
/// a value nobody can refresh, presented as a fact about now. That is
/// #841's fail-open shape, and #1067 states the consequence plainly: a
/// stale waiting indicator is WORSE than no indicator, because it sends
/// the user to a session that does not need them.
///
/// So the present-tense variant is not something the view decides. Rust's
/// `waiting()` is the only constructor and it can only produce `now`
/// against a `Liveness::Running`. The view's whole job is to render each
/// arm in its own tense:
///
/// | state | tense | why |
/// |---|---|---|
/// | `now` | present -- "Waiting for you" | a live process backs the claim |
/// | `last-seen` | past -- "Last seen waiting at 09:00" | it DID ask; we cannot say it still is |
/// | `no` | nothing at all | see `ClaudeNotWaitingReason` |
///
/// `last-seen` is a real answer and not an absence: a dead session that
/// stopped to ask a question is worth seeing on a row you are deciding
/// whether to resume. It just may never be stated in the present tense.
export type ClaudeWaiting =
  /// Waiting on the user right now, with a live process behind it.
  | {
      state: "now";
      /// The notification type as Claude Code sent it, VERBATIM.
      ///
      /// `idle_prompt` and `permission_prompt` are #1067's two names and
      /// they are different stories -- one session idling and one
      /// repeatedly asking permission -- which is why this is the value
      /// and not a boolean. Any other value renders as ITSELF: there is
      /// no "other" bucket anywhere in this epic.
      kind: string;
      /// When the notification was recorded, as an ISO timestamp.
      at: string;
    }
  /// It asked for input at `at`, and we cannot say whether it still
  /// needs it. Never rendered in the present tense.
  | {
      state: "last-seen";
      kind: string;
      at: string;
      /// Why the present tense could not be claimed -- the liveness
      /// reason, carried through so the indicator's `title` says which of
      /// "the process is gone" and "we could not tell" applies. Without
      /// it the past tense looks like an arbitrary hedge.
      why: string;
    }
  /// Not waiting, for one of three genuinely different reasons.
  | { state: "no"; reason: ClaudeNotWaitingReason };

/// The ways a session can fail to be waiting, which are not one way.
///
/// Serialised flattened into the `no` arm above, so the wire shape is
/// `{"state":"no","reason":"never-observed"}`.
///
/// - `never-observed` -- no notification record has EVER arrived for this
///   session. Absent, not zero: it is the answer for every session that
///   predates the hook install, and it means "we were not watching",
///   never "it never waited".
/// - `superseded` -- something newer happened after the notification, so
///   the session moved on and the earlier waiting is spent. This is the
///   expiry rule that makes the indicator clear itself.
/// - `not-a-prompt` -- the newest notification reported an event rather
///   than requesting an answer (`auth_success` and friends).
///
/// All three render nothing on the row, which is deliberate rather than
/// lazy: an indicator that appeared on 1,500 rows saying "we were not
/// watching this one either" would be the noise that gets the whole
/// feature ignored. The distinction is kept in the type because the
/// detail pane's copy can use it and because collapsing it into a
/// boolean is how "we did not ask" becomes "they did not answer" (#1050).
///
/// Not exported, for the reason `ResumeCommand` and `WireLiveness` above
/// are not: it is reached only through `ClaudeWaiting`, and `yarn knip`
/// is right that a second name nothing imports earns nothing. Exporting
/// it the moment something else needs it is one word.
type ClaudeNotWaitingReason = "never-observed" | "superseded" | "not-a-prompt";

/// How hard a session pushed against its context window (#1065).
///
/// Counted from `PreCompact` records. Rust side: `claude/signals.rs`.
///
/// # `manual` and `auto` are counted apart, and unknown is neither
///
/// `manual` is a user CHOICE and `auto` is the session hitting a wall, so
/// one summed total would answer neither question -- the same discipline
/// that keeps `ClaudeUsage` at four counters rather than one.
///
/// The whole object being absent (`ClaudeSessionDetail.compactions ===
/// null`) is a THIRD thing again, and the one that matters most on the
/// corpus this ships onto: it means no compaction was ever recorded for
/// this session, which is true of every session that ran before the hook
/// existed. Rendering that as zeros would be 1,500 confident wrong
/// answers with a credible shape.
export interface ClaudeCompactions {
  /// Compactions the user asked for.
  manual: number;
  /// Compactions the session ran into.
  auto: number;
  /// Triggers that are neither, VERBATIM and counted.
  ///
  /// A list of `[trigger, count]` pairs rather than a map, so the
  /// serialised shape is stable and an unknown value can never collide
  /// with a field name. A future trigger named `emergency` appears here
  /// as `emergency` and is rendered as `emergency` -- never folded into
  /// `auto` and never relabelled "other", which is this epic's rule about
  /// enum values.
  unknown: [string, number][];
  /// Compactions whose record carried no `trigger` field at all.
  ///
  /// Distinct from an unknown VALUE: this one says the payload SHAPE
  /// moved, not that the vocabulary grew. Different causes, so they are
  /// counted apart rather than summed into one "we did not understand it"
  /// bucket.
  untriggered: number;
}

/// What a session STATED it spawned, against what we INFER it spawned
/// (#1066).
///
/// Rust side: `claude/signals.rs`.
///
/// # Two sources, and the hook is the ADDITIONAL one
///
/// `subagent.rs` classifies a session as a subagent from its `cwd`, and
/// that inference is the only source for every session that already
/// exists. Nothing here replaces it. What the hook ADDS is the agent's
/// TYPE, which the directory rule cannot produce at all: a cwd of
/// `.claude/worktrees/agent-<hex>` yields an opaque id and no hint of
/// whether that was an `Explore` or a `code-reviewer`. So in the normal
/// case the two are not rivals -- the hook answers a question the
/// inference never could.
///
/// # Where they overlap, and why the view surfaces it
///
/// They make one claim in common: whether this session spawned subagents
/// at all. When `stated` totals more than zero and `inferred_children` is
/// zero, the layout assumption has moved -- subagents are running
/// somewhere the cwd rule does not look. Rust computes that sentence in
/// `AgentTypes::disagreement`, but it is a METHOD and not a field, so it
/// never crosses the wire; `subagentDisagreement` in
/// `src/lib/subagentDisagreement.ts` is the TypeScript twin, with the
/// asymmetry argued there.
export interface ClaudeAgentTypes {
  /// Each stated `agent_type` and how many were spawned, commonest
  /// first then alphabetically, so the order is total and the pane does
  /// not reshuffle between polls.
  ///
  /// This is what turns "3 subagents" into "2 general-purpose, 1
  /// code-reviewer".
  stated: [string, number][];
  /// Spawns whose record carried no usable `agent_type`.
  ///
  /// The hook fired and named no type. Counted, never guessed at, and
  /// never folded into a named bucket -- a blank rendered as a type would
  /// be a fabricated answer.
  untyped: number;
  /// Subagent sessions the INFERENCE attributed to this one.
  ///
  /// Carried alongside `stated` rather than merged, because the two count
  /// different things: this counts child SESSIONS that exist, `stated`
  /// counts spawn EVENTS that were observed. The disagreement check needs
  /// both numbers, and merging them would destroy the only signal that
  /// says the directory rule has stopped finding things.
  inferred_children: number;
}

/// One row exactly as `claude_sessions` sends it (#985).
///
/// Not what components consume -- see `ClaudeSession` below, which is
/// this with `liveness` resolved. Separated so the interning is a
/// transport detail that stops at the hook boundary.
///
/// Not exported, for the reason `WireLiveness` above is not: it is
/// reached only through `WireClaudeSessionList`.
interface WireClaudeSession {
  session_id: string;
  name: string | null;
  cwd: string | null;
  git_branch: string | null;
  last_activity_at: string | null;
  liveness: WireLiveness;
  cwd_state: CwdState;
  kind: ClaudeSessionKind;
  subagents: number;
  /// Not interned, unlike `liveness`. The `no` arm -- 41 bytes, and the
  /// answer for the whole corpus on a machine without the hook -- carries
  /// no free-text sentence, so there is nothing here for an intern table
  /// to deduplicate.
  waiting: ClaudeWaiting;
  context_pressure: boolean | null;
}

/// One row of the Claude Code session list, as the components see it
/// (#917, #985).
///
/// # What is here, and what moved (#985)
///
/// Exactly what the list renders, searches, filters or counts on. The
/// rest of what a session knows -- its resume command, transcript path
/// and stat, version, start time and run count -- is
/// `ClaudeSessionDetail`, fetched for the one selected row.
///
/// That split was measured rather than guessed. The old row was 990
/// bytes and 65% of it was read only by the detail pane, for one session
/// at a time, while the whole 1.35 MB crossed the pairing transport every
/// ten seconds. Splitting by FIELD rather than by row is what makes it
/// free: search still covers the whole corpus, the chip counts are still
/// over every row, and the stated total is still just `sessions.length`.
export interface ClaudeSession {
  /// The `claude --resume` handle, and the row's identity. Verified to
  /// survive `--resume` and `--continue` unchanged, so it never goes
  /// stale. One of the four fields search covers.
  /// The first thing the user asked (#1133).
  ///
  /// The ask, which a generated title cannot carry: 286 of 1,438 real
  /// sessions share their `aiTitle` with another. Optional so a cached
  /// list from before this field existed deserialises rather than
  /// failing. `null` renders as NOTHING -- never the title repeated,
  /// never the UUID.
  opening_prompt?: string | null;
  session_id: string;
  /// Claude's own `aiTitle`, present for 1,436 of 1,438 real sessions.
  /// `null` for the two that never got one -- never the UUID in
  /// disguise, because a fabricated name cannot be told from a real one.
  /// One of the four fields search covers.
  name: string | null;
  /// One of the four fields search covers, and what the row prints
  /// beneath the title.
  cwd: string | null;
  /// One of the four fields search covers.
  git_branch: string | null;
  /// The newest record in the transcript, NOT when we scanned it.
  last_activity_at: string | null;
  /// Derived on every poll, for EVERY row -- which is why no approach
  /// that dropped rows was acceptable. A session dies without any write
  /// to its row, so a limit or an `updated_since` cursor would leave rows
  /// outside its window claiming "running" indefinitely.
  liveness: Liveness;
  /// Stays on the row because the Resumable and Directory-gone chips
  /// count it, and those counts are over the whole list.
  cwd_state: CwdState;
  /// Whether this session ran in an agent worktree (#1002).
  ///
  /// On the row rather than the detail because the list FILTERS on it and
  /// the chip's count is over the whole corpus: 391 of 1,524 measured
  /// rows are subagents, and a chip that could only count the rows it had
  /// already drawn would be a number that lies.
  kind: ClaudeSessionKind;
  /// How many subagent sessions were traced to this one (#1002).
  ///
  /// `0` is a real answer: it is a count over rows the app holds, so "no
  /// subagents were attributed to this session" is something we know.
  /// Contrast the token rollup, which is a measurement and therefore
  /// absent-or-known.
  subagents: number;
  /// Whether this session is waiting on the user (#1067).
  ///
  /// On the LIST and not the detail, against #985's split, because this
  /// is the one fact in epic #1060 that is about NOW: a user with several
  /// sessions open wants to see which of them is blocked from the row
  /// itself, and a field only the open detail pane carried would answer
  /// the question for the one session they had already chosen.
  ///
  /// Cheap enough to justify the exception. The serialised `no` arm is 41
  /// bytes, and it is what every session on a machine without the hook
  /// sends -- the only case that multiplies by 1,500.
  waiting: ClaudeWaiting;
  /// Whether this session repeatedly hit its context limit (#1065).
  ///
  /// Three renderings, not two. `null` means no compaction was ever
  /// RECORDED for this session -- absent, not zero, and the state every
  /// pre-hook session is in. `false` means we watched and it did not.
  /// `true` is the flag. A row that drew `null` and `false` alike would
  /// be claiming a measurement it never took, on the whole corpus.
  ///
  /// A boolean rather than the full `ClaudeCompactions` because the row
  /// shows a FLAG and the breakdown belongs to the detail pane -- #985's
  /// split by field, applied to a new field rather than discovered later.
  context_pressure: boolean | null;
}

/// Whether a session is the user's own work or an agent's (#1002).
///
/// Structural, from the session's `cwd`: a session whose directory is
/// `<repo>/.claude/worktrees/agent-<id>` ran inside an agent worktree.
/// Rust side: `src-tauri/src/claude/subagent.rs`, which carries the
/// measurement -- 391 of 1,524 rows, across 107 agents, and the two
/// markers it rejected (`git_branch` matches only 102 of the 391;
/// `isSidechain` is false on all of them).
///
/// A subagent session is STILL A REAL SESSION: hidden by default, never
/// deleted, still resumable by id. Several did substantial work.
type ClaudeSessionKind =
  | { kind: "own" }
  | { kind: "subagent"; agent_id: string };

/// What one SELECTED Claude Code session knows (#985).
///
/// The heavy half of the old row, fetched on selection instead of pushed
/// for every session on every poll. `claude_session_detail` resolves to
/// `null` when the store has no such id -- a session deleted between two
/// polls -- which is a different answer from a rejected read and must not
/// render as one (#846).
/// One pull request a session produced (#1132). Mirrors
/// `claude::subagent::PrLink`.
export interface ClaudePrLink {
  session_id: string;
  repo: string;
  number: number;
  url: string;
  first_seen_at: string | null;
}

export interface ClaudeSessionDetail {
  session_id: string;
  claude_version: string | null;
  transcript_path: string | null;
  first_seen_at: string;
  /// Derived on THIS read, not copied from the list's. The detail pane is
  /// where the reason is shown, so it states one as fresh as the verdict
  /// it explains.
  liveness: Liveness;
  /// Whether the transcript file is still on disk (#919).
  ///
  /// A SEPARATE reading from `cwd_state`, never derived from it: 0% of
  /// transcripts are gone against 83% of cwds, so a transcript action
  /// gated on the cwd would be disabled on 1,213 of 1,461 real rows.
  transcript_state: CwdState;
  resume: ResumeCommand;
  /// Runs the hook recorded. `0` for every imported session, which lets
  /// the UI say "never observed" rather than implying we watched and
  /// lost it.
  runs: number;
  /// The pull requests this session produced (#1132).
  ///
  /// Optional so a cached detail from before this field existed
  /// deserialises rather than failing. Empty is a real answer: most
  /// sessions open no pull request.
  pull_requests?: ClaudePrLink[];
  /// Why the live registry could not be listed, for this read. Carried
  /// for the reason the list carries it: a `liveness` that is `unknown`
  /// because the registry was unreadable must be able to say so rather
  /// than present a shrug as a finding.
  registry_failure: string | null;
  /// Whether this session ran in an agent worktree (#1002).
  kind: ClaudeSessionKind;
  /// The subagent sessions traced to this one, newest first.
  ///
  /// On the detail and not the row: empty for the overwhelming majority
  /// of sessions, and 1,524 rows carrying a vector each to serve the
  /// handful with children is the per-row cost #985 measured and removed.
  subagents: ClaudeSubagentChild[];
  /// Which session spawned THIS one, when it is an attributed subagent.
  ///
  /// `null` on an ordinary session AND on an unattributed subagent.
  /// `unattributed` below is what tells those two apart -- "not a
  /// subagent" and "a subagent whose parent we could not tell" are
  /// different facts, and only the second needs saying.
  parent: ClaudeSubagentChild | null;
  /// Why this subagent could not be traced to a parent (#1002).
  ///
  /// `Some` ONLY for a subagent the map looked at and could not decide,
  /// and it carries the evidence -- which sessions tied, and when -- so
  /// the reader can see the app looked rather than that it shrugged. A
  /// wrong rollup is worse than no rollup.
  unattributed: string | null;
  /// How often this session compacted, split by trigger (#1065).
  ///
  /// `null` is "no compaction was ever recorded for this session", which
  /// is NOT the same claim as "none happened" -- and it is the state of
  /// every session that ran before the hook was installed, which is all
  /// of them on the day this ships. The pane renders the two differently
  /// or it is inventing a measurement.
  compactions: ClaudeCompactions | null;
  /// What this session STATED it spawned, against what we infer (#1066).
  ///
  /// `null` when neither source has anything to say. A value with an
  /// EMPTY `stated` and a non-zero `inferred_children` is the normal
  /// pre-hook session and must not read as an error -- it is the ordinary
  /// case in which the directory rule is the only source there has ever
  /// been.
  agent_types: ClaudeAgentTypes | null;
  /// Whether this session is waiting on the user (#1067).
  ///
  /// Derived from the same liveness this pane's own badge shows, on this
  /// read -- so the detail can never disagree with itself about whether
  /// the process is there. Carried on both halves for that reason rather
  /// than reused from the row: the row's was derived on the list's poll.
  waiting: ClaudeWaiting;
}

/// One subagent session, as its parent's detail lists it (#1002).
interface ClaudeSubagentChild {
  session_id: string;
  name: string | null;
  agent_id: string;
}

/// One named thing that went wrong, with how often (#1062, #1063, #1064).
///
/// `name` is carried VERBATIM from the hook payload and is never mapped to
/// a known set: an `error_type` or `tool_name` this app has never seen
/// renders as itself. Folding an unrecognised value into "other" would
/// destroy the one thing the record carries, and a new Claude Code release
/// would silently start hiding its own new failure modes.
export interface ClaudeTally {
  /// The `error_type` or `tool_name`, exactly as recorded. `null` when the
  /// payload carried none -- render that as "not recorded", never as a
  /// blank row.
  name: string | null;
  /// How many records carried it.
  count: number;
  /// One example of the free-text detail, capped by the writer at 160
  /// bytes. `null` when none was recorded, which #1064 requires be said
  /// rather than filled in with an invented sentence.
  ///
  /// UNTRUSTED vendor text. Display only: never log it, never put it
  /// anywhere a committed file could pick it up.
  detail: string | null;
}

/// What went wrong in a session, or across all of them.
///
/// Failures and denials are counted APART and must never be summed. A
/// denial is a guardrail working; a failure is something that broke. One
/// number covering both would answer neither question and would present
/// the guardrail as damage (#1064).
export interface ClaudeProfile {
  /// `StopFailure` by `error_type` -- why turns died. Commonest first.
  turn_failures: ClaudeTally[];
  /// `PostToolUseFailure` by `tool_name`. Commonest first.
  tool_failures: ClaudeTally[];
  /// `PermissionDenied` by `tool_name`. Commonest first. NOT a failure
  /// list -- see above.
  denials: ClaudeTally[];
}

/// How much of a session's failure history the app can state (#1062).
///
/// THREE states, because two would lie, and the discriminated union is the
/// mechanism: there is no `profile` to read on `unobserved`, so a zero
/// cannot be rendered for a session nobody watched.
///
/// | state | meaning | rendering |
/// |---|---|---|
/// | `unobserved` | no hook ever saw this session | "not recorded" — NEVER 0 |
/// | `partial` | some events were not installed | a FLOOR, naming what is missing |
/// | `observed` | every event was recording | a total; a 0 here is real |
///
/// `unobserved` is the normal state for history that predates the install,
/// which on a machine that adopted Headstate after using Claude Code is
/// almost all of it. Rendering it as "0 failures" is the absent-is-not-zero
/// defect (#846) in its most convincing form: the session looks clean when
/// the truth is nobody was watching.
export type ClaudeObservation =
  | { state: "unobserved" }
  | { state: "partial"; missing: string[]; profile: ClaudeProfile }
  | { state: "observed"; profile: ClaudeProfile };

/// The cross-session failure and denial profile, with its denominators
/// (#1062, #1063, #1064).
///
/// The denominators are not decoration. A profile over 3 observed sessions
/// out of 1,461 stored is a very different statement from the same profile
/// over all of them, and without them the two render identically.
export interface ClaudeCorpus {
  /// What was recorded, and whether it is a total or a floor.
  observation: ClaudeObservation;
  /// Every stored session, as the denominator.
  sessions: number;
  /// Of those, how many a hook ever observed. The rest predate the install
  /// and can contribute nothing.
  sessions_observed: number;
  /// Of the observed, how many recorded at least one of these events.
  ///
  /// A SMALL number here is the GOOD news -- most observed sessions had
  /// nothing go wrong -- and the UI must frame it that way. Presented bare
  /// it reads as a coverage problem.
  sessions_with_events: number;
}

/// What one session's subagents cost, as a figure of its own (#1002).
///
/// NEVER added into the parent's own `ClaudeUsage`. A parent's own tokens
/// answer "how much work happened in this session"; these answer "how
/// much happened underneath it". One summed figure would answer neither:
/// a parent that delegated everything would show a large number
/// describing work it did not do, indistinguishable from one that did the
/// work itself. The same discipline that keeps `ClaudeUsage` at four
/// counters rather than one.
export interface ClaudeSubagentRollup {
  /// Subagent sessions attributed to this parent.
  sessions: number;
  /// Of those, how many yielded a usage sum. The denominator that makes
  /// the totals readable as a floor rather than a total. `0` with
  /// `sessions > 0` means "it has subagents and we could not total them",
  /// which must render as "could not tell" and never as zeros.
  measured: number;
  /// Children whose transcript carried no usage block at all. A settled
  /// answer -- we read it and it had none -- and NOT the same as a
  /// failure to read.
  without_usage: number;
  /// Children whose transcript could not be read, with why. Non-empty
  /// means every total is short by an unknown amount.
  unreadable: string[];
  /// Children whose sum stopped at the 8 MB budget, so the figures are a
  /// floor.
  truncated: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  messages: number;
}


/// How much work happened inside one session (#959).
///
/// Summed from the session's own transcript, per message, on demand.
/// Rust side: `src-tauri/src/claude/usage.rs`, which carries the
/// measurement that reopened #910's "usage is not in the data" cut --
/// 1,478 of 1,502 real transcripts carry it.
///
/// # Four counters, not one total
///
/// Cache reads run two to three orders of magnitude above fresh input on
/// every real session measured, so a single summed "tokens" figure would
/// be a cache-read count wearing a misleading name.
///
/// # Tokens, never dollars
///
/// A dollar figure needs per-model rates, those rates change, and this
/// app cannot keep a hardcoded table true. A quietly wrong cost with a
/// currency symbol in front of it is the confident-wrong-answer failure
/// #941 is about.
export interface ClaudeUsage {
  /// Assistant messages carrying a usage block. `0` means NONE WAS FOUND
  /// -- 24 of 1,502 real transcripts -- and must never render as four
  /// measured zeros. It is the gate for the whole panel.
  messages: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  /// Models seen, with how many messages each wrote, most first. `model`
  /// is per-MESSAGE and the corpus is mixed, so "which model was this
  /// session" has no single answer.
  models: { model: string; messages: number }[];
  /// Whether the read stopped at the byte budget before the end of the
  /// file. `true` makes every figure above a FLOOR, and the UI must say
  /// so -- an unlabelled partial sum cannot be told from a complete one.
  truncated: boolean;
  bytes_read: number;
  file_bytes: number;
}

/// One content block of a previewed message (#982).
///
/// A tagged union rather than a flattened string, because the kinds
/// answer different questions and render differently: text is what was
/// said, a tool call is what was done, and a tool result is usually far
/// too long to show whole.
export type ClaudePreviewBlock =
  | { kind: "text"; text: string; truncated: boolean }
  | { kind: "thinking"; text: string; truncated: boolean }
  | { kind: "tool_use"; name: string }
  | { kind: "tool_result"; text: string; truncated: boolean }
  /// A block kind this build does not know. Reported rather than
  /// dropped: Claude Code owns this format, and a pane that silently
  /// omitted a future kind would show an exchange with an invisible hole
  /// in it.
  | { kind: "other"; block_type: string };

/// One previewed message.
///
/// Not exported: it is reached only through `ClaudePreview.messages`, and
/// `yarn knip` is right that a second name for the same shape earns
/// nothing. The same call `ResumeCommand` above makes, and exporting it
/// the moment something else needs it is one word.
interface ClaudePreviewMessage {
  /// `"assistant"` or `"user"`.
  role: string;
  /// RFC 3339, or `null` for a record that carried none. Never
  /// substituted: a fabricated time cannot be told from a real one.
  timestamp: string | null;
  model: string | null;
  blocks: ClaudePreviewBlock[];
}

/// The tail of one transcript, as conversation (#982).
///
/// Rust side: `src-tauri/src/claude/preview.rs`, which argues the 256 KB
/// window, the record-type allowlist, and why both are reported.
export interface ClaudePreview {
  /// Oldest first, so it reads as a conversation.
  messages: ClaudePreviewMessage[];
  /// Whether anything before these messages was NOT read. The pane must
  /// say so: a reader who cannot tell a short conversation from a
  /// truncated one has been told something false by omission (#846).
  truncated: boolean;
  bytes_read: number;
  file_bytes: number;
  /// Records in the window that were machinery rather than conversation.
  /// 44.2% of real records are, so a pane showing six messages out of a
  /// 300-record window has to say where the rest went.
  non_conversation_records: number;
  /// Lines in the window that would not parse at all. DISTINCT from the
  /// count above: one is a record we understood and chose not to show,
  /// the other is one we could not read.
  unparseable_records: number;
}

/// The session list, INCLUDING what could not be read (#917).
///
/// `registry_failure` is the reason this is a envelope rather than a
/// bare array: a list built from a registry we could not read is a list
/// in which every liveness is `unknown`, and the view must say so rather
/// than show rows that look like settled answers.
export interface ClaudeSessionList {
  /// EVERY stored session, always -- never a page or a window. The
  /// stated total is `sessions.length`, so there is no separate count
  /// that could drift from the rows beside it. `RENDER_CAP` bounds what
  /// is DRAWN and says so; nothing bounds what arrives.
  sessions: ClaudeSession[];
  /// Why the live registry could not be listed. `null` means it was read
  /// -- so an absence of running sessions is a real answer.
  registry_failure: string | null;
  /// Registry files that could not be parsed. Each one hides a session
  /// whose liveness cannot be stated.
  registry_unreadable: string[];
}

/// The session list exactly as it arrives, before the reasons are
/// resolved (#985).
///
/// `hydrateClaudeSessions` turns this into `ClaudeSessionList` at the
/// hook boundary, which is the only place either shape is known.
export interface WireClaudeSessionList {
  sessions: WireClaudeSession[];
  /// Every distinct liveness reason, once. A `WireLiveness`'s `why` is
  /// an index into this. ONE entry on the measured corpus of 1,474.
  reasons: string[];
  registry_failure: string | null;
  registry_unreadable: string[];
}

/// The headline figures on the Claude Code overview (#921).
///
/// Every field is a COUNT over sessions Headstate has a row for. The
/// three cwd states are mutually exclusive and sum with `running` to
/// `sessions`, so a reader can check the page's arithmetic -- which is
/// the point of carrying `cwd_unknown` at all rather than folding it into
/// the larger bucket.
///
/// Rust side: `src-tauri/src/claude/overview.rs`, which argues the
/// resurrection predicate.
export interface ClaudeCounts {
  /// Every session in the cache. Context for the figures below, NOT a
  /// hero number -- "1,461 sessions ever" answers nothing on its own,
  /// which is why #921's total-sessions tile is cut.
  sessions: number;
  /// Running right now, derived from the live registry checked against
  /// the process table. Trustworthy only while `live_failure` is null.
  running: number;
  /// Not running, and the recorded directory still exists.
  ///
  /// **The page's headline.** 248 of 1,461 on the development machine:
  /// the sessions that can be resumed back into the tree they came from.
  resumable: number;
  /// Not running, and the recorded directory is gone. 1,213 of 1,461, so
  /// this is the NORMAL state and must not be rendered as damage.
  archived: number;
  /// Not running, and the directory could not be CHECKED. Neither
  /// `resumable` nor `archived`: a permission error means the tree may
  /// well be there, and counting it as archived tells the user to give up
  /// on work that was never lost.
  cwd_unknown: number;
  /// Runs the hook saw start and never saw end, for sessions that are not
  /// running. #921 proposes this as the headline figure; it is **0** on
  /// every machine whose history predates the hook, which is why the
  /// headline is `resumable` instead. See the Rust module comment.
  orphaned_runs: number;
  /// Sessions the hook never observed at all. Without this a reader
  /// cannot tell "nothing crashed" from "nothing was watched".
  never_observed: number;
}

/// One day of the activity chart (#921).
export interface ClaudeDayCount {
  /// `YYYY-MM-DD`, UTC.
  day: string;
  /// Sessions whose FIRST activity fell on this day. A session resumed
  /// over four days counts once, on the day it began, so the series reads
  /// as intake rather than as touches.
  started: number;
}

/// One row of the resumable list (#921).
///
/// Deliberately thinner than #917's `ClaudeSession`: the resume command
/// and its caveat live there, and a second definition of the same shape
/// would drift. This carries the id so the page hands off to the list.
export interface ClaudeResumable {
  session_id: string;
  /// Claude's own `aiTitle`, or `null` -- never the UUID dressed up as a
  /// name.
  name: string | null;
  cwd: string | null;
  git_branch: string | null;
  last_activity_at: string | null;
}

/// The resume command and what to say before pasting it, as the restart
/// export carries it (#1071).
///
/// Structurally identical to `ResumeCommand` above -- both are the same
/// Rust type -- and not exported, for the same reason that one is not:
/// it is reached only through `ClaudeRestartEntry.resume`, and `yarn
/// knip` is right that a second exported name for the same shape earns
/// nothing. Two declarations rather than one shared alias because the two
/// travel in different payloads and each documents its own; exporting
/// either the moment something else needs it is one word.
interface ClaudeResumeCommand {
  /// The text to paste. Always correct to run: `claude --resume <id>`
  /// resolves by id and works after the recorded directory is deleted.
  command: string;
  /// `null` only when the command carries its own `cd`. Every unanchored
  /// line has one, and the export renders it as a `#` comment ABOVE the
  /// line: a bare `claude --resume` pasted blind resumes in whatever
  /// directory the user happens to be in.
  caveat: string | null;
  anchored: boolean;
}

/// One line of the restart list (#1071).
///
/// Carries the BUILT command rather than the parts. A frontend that
/// rebuilt `cd <dir> && claude --resume <id>` from `cwd` and `session_id`
/// would have to redo the quoting and the four-way cwd treatment, and the
/// id is `path.file_stem()` of an arbitrary `*.jsonl` with no format check
/// -- exactly the case a review of #918 caught.
export interface ClaudeRestartEntry {
  session_id: string;
  /// Claude's own `aiTitle`, or `null`. Never the UUID dressed up as a
  /// name.
  name: string | null;
  cwd: string | null;
  resume: ClaudeResumeCommand;
}

/// A session whose liveness could not be decided, with the grounds.
///
/// INCLUDED in the export rather than dropped. `Unknown` is not a shade
/// of `Dead` -- #984 misclassified 183 of 1,491 sessions by treating it
/// as one -- and in a restart list that mistake is work the user rebooted
/// away.
export interface ClaudeUncertainEntry extends ClaudeRestartEntry {
  /// `liveness::derive`'s own sentence. A "could not tell" heading with
  /// no grounds is the shrug the reason field exists to prevent.
  why: string;
}

/// Every session to restart after a reboot, and what qualifies the list
/// (#1071).
///
/// Two lists rather than one with a flag: "this is running" and "we could
/// not tell" are different claims rendered under different headings, and a
/// single list would let a reader paste past the boundary without seeing
/// it.
export interface ClaudeRestartList {
  /// Positively established as running: the process is there and its
  /// start time matches what was recorded.
  running: ClaudeRestartEntry[];
  /// Could not be decided. Included on purpose.
  uncertain: ClaudeUncertainEntry[];
  /// Why the live registry could not be listed. Non-null means nothing
  /// could be positively established as running, so an empty `running` is
  /// NOT "nothing is running".
  registry_failure: string | null;
  /// Records present but unusable. Each hides a session that may be
  /// running, so the list is a floor.
  registry_unreadable: string[];
}

/// Everything the Claude Code overview draws, plus what it could not
/// establish (#921).
///
/// `live_failure` is why this is an envelope. A live registry we could not
/// read leaves every count over stored history valid, so the honest
/// rendering is the page WITH a banner rather than no page -- but
/// `running` is then not an answer, and showing "0 running" as though it
/// were is #841's fail-open in the one place a user acts on it: "nothing
/// is running" is what makes a Resume button look safe.
///
/// A failed DATABASE read is not in this type at all; it is a rejected
/// query the page renders as `QueryError`. A struct of zeros would draw 30
/// chart columns and a "0 resumable" tile that look exactly like a
/// measured quiet month -- the worst form of absent-is-not-zero, because
/// a flat line does not look absent (#846 is the same defect one view
/// over).
export interface ClaudeOverview {
  counts: ClaudeCounts;
  /// Exactly `ACTIVITY_DAYS` buckets, oldest first, INCLUDING days with
  /// no sessions. The empty days are the point: a series of only the days
  /// that had activity draws a dense chart in which a week off reads as a
  /// week of steady work.
  activity: ClaudeDayCount[];
  /// The newest few of `counts.resumable`. A stated subset, never a
  /// silently short list.
  resumable: ClaudeResumable[];
  /// Why the live session registry could not be listed. `null` means it
  /// WAS read, so `counts.running` is real -- including when it is zero.
  live_failure: string | null;
  /// Registry records present but unusable. Each one hides a session that
  /// may be running, so a non-empty list makes `counts.running` a floor.
  live_unreadable: string[];
}

/// One project's worth of reports.
///
/// The unit the UI groups by. A repository can hold several -- a
/// frontend and a backend are separate manifests and often separate
/// ecosystems, so their updates are separate pieces of work.
export interface ProjectReport {
  /// Absolute path to the project directory.
  path: string;
  /// Relative to the repository root. Empty at the root itself.
  label: string;
  reports: EcosystemReport[];
}

/// Why a branch may or may not be deleted.
///
/// Reports the fact rather than a verdict, so the UI can say WHY a
/// branch is not deletable instead of only greying out a control.
export type Deletable =
  /// `squash` comes from comparing patch-ids -- a content comparison,
  /// not a graph one. Measured on a real repository, 489 of 536 merged
  /// branches were squashes, so it is the common case, not the exotic
  /// one, and the UI says which.
  | { kind: "merged"; how: "ancestor" | "squash" }
  | { kind: "defaultBranch" }
  | { kind: "checkedOut"; path: string }
  /// `ahead` is `number | null`: `%(ahead-behind:)` needs git 2.41, and
  /// on an older git the count is simply unavailable. `null` renders as
  /// "commit count unavailable", never as "0 commits" -- which read as a
  /// branch with nothing to lose, beside a delete checkbox (#967).
  | { kind: "unmerged"; ahead: number | null }
  | { kind: "pending" }
  | { kind: "unknown"; reason: string };

export interface Branch {
  name: string;
  /// The three cases clean up differently, which is why this is one
  /// value rather than a pair of booleans: deleting a tracked pair is
  /// two operations against two different things.
  /// `gone` is a local branch whose upstream is configured but no
  /// longer on the remote -- the ordinary state after a PR merges and
  /// the head branch is deleted (#1139). Distinct from `local`, which it
  /// used to collapse into: `local` means the work exists only here,
  /// `gone` means it was pushed, merged and cleaned up. Opposite claims,
  /// and they used to share one word.
  location: "local" | "remote" | "tracked" | "gone";
  upstream: string | null;
  /// `null` when the count could not be read -- git older than 2.41, git
  /// missing from a GUI-launched app's PATH, a `safe.directory` refusal.
  /// Never coerced to 0 (#967).
  ahead: number | null;
  behind: number | null;
  /// ISO 8601, as git reports it.
  committed: string;
  author: string;
  tip: string;
  deletable: Deletable;
}

export interface DeleteOutcome {
  name: string;
  /// `null` on success; the reason otherwise.
  error: string | null;
}

/// One frame of a running branch scan, mirroring the Rust
/// `BranchScanFrame` in `src-tauri/src/commands.rs`.
///
/// Two shapes on one event name because it is one stream. `listed`
/// arrives once, with every row and — crucially — the TOTAL; then
/// `classified` frames carry verdicts as the eight classification
/// threads settle them.
///
/// The total is what separates a dead stream from a finished one. Fed
/// only verdicts, a page that stopped receiving them at 47 would look
/// exactly like a page that had received all of them; knowing 512 were
/// promised, it can say so (#657).
///
/// `repo` is on every frame and is load-bearing: the events are
/// app-global while a scan is per-repository, so a page that switched
/// repository mid-scan would otherwise fold the old repository's
/// verdicts into the new one's rows.
export type BranchScanFrame =
  | { kind: "listed"; repo: string; total: number; branches: Branch[] }
  | { kind: "classified"; repo: string; verdicts: [string, Deletable][] };

/// One frame of PR Stats backfill progress, mirroring the Rust
/// `StatsBackfillFrame` in `src-tauri/src/commands.rs` (#1093).
///
/// ONE shape, unlike `BranchScanFrame`'s two, because this stream has one
/// kind of news: the coverage moved. Every frame carries the whole state
/// rather than a delta, so a listener that joined late -- or missed a
/// frame while the page was closed -- renders correctly from the next one
/// instead of accumulating from a start it never saw.
export interface StatsBackfillFrame {
  /// The scope this describes, as the Rust side keys it. Compared before
  /// anything is rendered: the event is app-global while the work is
  /// per-scope, so a page that changed scope mid-walk would otherwise
  /// show another scope's coverage under its own heading.
  scopeKey: string;
  /// Days of the window that have actually been retrieved.
  daysCovered: number;
  /// Days in the window.
  daysTotal: number;
  /// Pull requests held for those days.
  collected: number;
  /// GitHub's exact total, or `null` when nothing has measured it.
  ///
  /// **Never treat `null` as 0.** "400 of 0" is nonsense and "400 of 400,
  /// complete" is worse, because it reads as reassuring. `null` means the
  /// denominator is unknown, and the only honest render of it says so.
  /// The same discipline `Branch.ahead`/`behind` keep (#967).
  total: number | null;
  /// What the worker is doing about this scope right now.
  ///
  /// The page must distinguish "still collecting" from "stopped": a
  /// caveat that reads identically in both cases is an indefinite
  /// skeleton at page level, where the reader cannot tell waiting from
  /// broken (#1042).
  ///
  /// A `boolean` could not say WHY the wait was happening, and #1103 is
  /// what that costs: a board sat at "0 of 30 days measured" for ten
  /// minutes while the worker was alive, solvent and deliberately
  /// waiting, and the page had no way to say so.
  phase: BackfillPhase;
  /// When the next batch is due, as Unix milliseconds, or `null` when
  /// none is scheduled.
  ///
  /// From the BACKEND. Never computed as `now + 60s` on the page: the
  /// worker rotates across registered scopes, so one scope's next tick is
  /// N intervals away. A page-computed countdown would reach zero,
  /// nothing would happen, and the page would look broken in a new way --
  /// with a timer to make it look deliberate.
  nextTickAtMs: number | null;
}

/// What the PR Stats backfill is doing for a scope, mirroring the Rust
/// `BackfillPhase` in `src-tauri/src/github/stats/backfill.rs` (#1103).
///
/// Five states rather than a boolean, because "nothing is changing" has
/// five different meanings and only one of them is a bug. `Paused` and
/// `Stalled` are deliberately distinct: one lifts on its own at a known
/// time, the other may not.
export type BackfillPhase =
  | { kind: "working" }
  | { kind: "waiting" }
  /// `remaining` is `null` on a cold start -- nothing has reported a
  /// budget yet. Never render that as 0.
  | { kind: "paused"; remaining: number | null }
  | { kind: "converged" }
  | { kind: "stalled" };

/// One frame of a running branch DELETION, mirroring the Rust
/// `BranchDeleteFrame` in `src-tauri/src/commands.rs`.
///
/// Two shapes because a deletion is two phases with different
/// meanings, and collapsing them into one counter is the bug this
/// exists to fix. The safety re-check is a full uncached scan at ~64ms
/// per branch, so on the 562-branch batch that was reported a single
/// counter would read 0/562 for MINUTES — indistinguishable from a
/// hang — before a single ref came off (#724).
///
/// `checking` therefore counts branches the re-check has classified,
/// and its total is every branch in the repository, since that is what
/// the gate scans. `deleting` counts the selected batch, and carries
/// `failed` on every frame so refusals are visible while the run is
/// still going rather than only in the toasts afterwards.
///
/// Counts only: no branch names, no paths. Unlike the scan's frames,
/// which are filling a list of names in, nothing here needs a join key.
export type BranchDeleteFrame =
  | { kind: "checking"; repo: string; done: number; total: number }
  | { kind: "deleting"; repo: string; done: number; total: number; failed: number };

/// One moment of the machine's health, mirroring the Rust
/// `health::Sample` in `src-tauri/src/health/mod.rs`.
///
/// # Absent is not zero
///
/// Every optional field here is `null` when the platform does not
/// expose it, never `0`. The UI must render those as "not measured":
/// a zero that means "we could not look" reads as a real reading, and
/// "0% CPU" and "we did not measure the CPU" are opposite claims. This
/// is the same rule the Rust side states in its module docs.
/// One health condition that is true right now (#864).
///
/// `health_alerts` returns every CURRENTLY TRUE condition, not the ones
/// that just became true. That is deliberate on the Rust side: a
/// transition-only answer would depend on who asked last, so two clients
/// would each see half the alerts. It is also what lets a page render
/// them, which is the gap #864 closes -- the command existed and was
/// registered for both the desktop and the phone, and no TypeScript
/// called it, so the rules evaluated into nothing.
export interface AlertReport {
  /// The stable condition identity, keyed on the condition and never on
  /// the numbers -- so a standing alert keeps one identity while its
  /// figures wander. Use it as the React key and for dismissal, never
  /// the title.
  key: string;
  title: string;
  body: string;
  /// The process this condition is about, or `null` where it is not about
  /// exactly one (#943).
  ///
  /// The CPU watch notice's body ends "Worth a look if you did not start
  /// something long-running", and until this field existed the page had
  /// nothing to look WITH: the notice named a process by name, and a name
  /// does not identify one of forty. The pid reached `ProcessObservation`
  /// and was dropped when the notice was built.
  ///
  /// `null` for every machine-wide condition -- battery, thermal,
  /// oversubscription, and the aggregate CPU alert whose whole content is
  /// that no single process explains the load -- and also for a watch
  /// notice that collapsed several processes of one name. That last case is
  /// absent-is-not-zero in its sharpest form here: a "3 node processes"
  /// row carries no pid rather than one of the three, because a pid that
  /// names the wrong process is indistinguishable from one that names the
  /// right one. Never coerced to 0: pid 0 is a real process on every
  /// platform this app runs on.
  pid: number | null;
}

export interface HealthSample {
  /// RFC 3339, matching every other timestamp this app stores.
  sampled_at: string;
  /// 1, 5 and 15 minute load averages, or `null` on a platform with no
  /// equivalent (Windows).
  load: [number, number, number] | null;
  /// Whole-machine CPU use, 0-100.
  cpu_percent: number | null;
  /// Per-core, 0-100, in the platform's own core order. Empty rather
  /// than null when unavailable, matching the Rust `Vec`.
  cpu_per_core: number[];
  memory: HealthMemory;
  /// Every GPU the platform would describe, which on several is none.
  ///
  /// Empty is "nothing discoverable" -- a platform Headstate cannot
  /// read unprivileged (Windows, and Intel/NVIDIA on Linux), or a
  /// machine with no GPU. The UI draws NO PANEL for an empty list
  /// rather than a panel of zeroes: a 0% GPU is a claim about an idle
  /// GPU, and "we did not look" is the opposite claim.
  gpus: HealthGpu[];
  disks: HealthVolume[];
  battery: HealthBattery | null;
  /// `nominal` / `fair` / `serious` / `critical`.
  ///
  /// NOT a temperature. The SMC needs elevated privileges on macOS, so
  /// what is actually readable is the platform's thermal PRESSURE --
  /// a coarse label. The UI says so; see `SystemHealthPage`.
  thermal: string | null;
  networks: HealthInterface[];
  /// Seconds since boot.
  uptime_secs: number;
}

/// The four unexported types below match `DockerOrigin` and
/// `UpdateOutcome` above: nothing outside this file names them, since
/// every consumer reaches them through `HealthSample`. Exporting a name
/// no one imports is a name that has to be kept correct for no reader.
interface HealthMemory {
  total: number;
  used: number;
  /// What the OS believes is reclaimable, which is NOT `total - used`
  /// on any modern platform: cache counts as used and is available.
  available: number;
  swap_total: number;
  swap_used: number;
}

/// One GPU, mirroring the Rust `health::Gpu` in
/// `src-tauri/src/health/gpu.rs`.
///
/// Exported, unlike the other `HealthSample` helpers, for the same
/// reason as `FootprintProcess`: `SystemHealthPage` renders a component
/// per GPU and therefore has to name the type.
///
/// Every field but `name` is nullable, because the platforms disagree
/// about which they answer. macOS reports all of them; AMD on Linux
/// reports all of them; a machine that reports only utilization leaves
/// the memory pair null, and the UI renders that as "Not measured"
/// rather than as 0 bytes.
export interface HealthGpu {
  /// The adapter as the platform names it -- "Apple M2 Max", or the
  /// DRM card on Linux. Never a path.
  name: string;
  /// Whole-device utilization, 0-100.
  utilization_percent: number | null;
  memory_used: number | null;
  /// On a unified-memory machine this is the share currently allocated
  /// to the GPU, NOT a dedicated pool. See `unified_memory`.
  memory_total: number | null;
  /// True when the GPU shares the system's memory rather than having
  /// its own, which is every Apple Silicon Mac.
  ///
  /// The UI MUST say so where this is true. The Memory panel reports
  /// the same physical pool, so without that sentence the two panels
  /// look like they disagree about how much memory the machine has --
  /// and a reader would reasonably add the GPU's gigabytes to the
  /// system's and conclude the machine has more RAM than it does.
  unified_memory: boolean;
  /// The RENDERER stage's own utilization, 0-100, where the platform
  /// splits the pipeline (#717).
  ///
  /// macOS reports `Renderer Utilization %` and `Tiler Utilization %`
  /// beside the device figure. `utilization_percent` is the device
  /// number and is the one the overview shows; these two are on the GPU
  /// detail page, because a GPU pinned by geometry setup and one pinned
  /// by shading are the same row on the overview and different problems
  /// underneath.
  ///
  /// `null` on every platform that does not split them, which is every
  /// platform but macOS — and deliberately not filled in from the
  /// device figure, which would report a measurement nobody took.
  ///
  /// OPTIONAL as well as nullable, for the same version-skew reason as
  /// `Footprint.top_cpu`: a stored sample from before this shipped, or
  /// a desktop released before it, carries no such key at all.
  renderer_percent?: number | null;
  /// The TILER stage's utilization, on the same terms.
  ///
  /// Apple's GPUs are tile-based deferred renderers: the tiler bins
  /// geometry into screen tiles and the renderer shades them. They are
  /// separate hardware stages that saturate independently, which is why
  /// one device number cannot stand for both.
  tiler_percent?: number | null;
}

interface HealthVolume {
  mount: string;
  total: number;
  available: number;
  /// True for the volume the app itself lives on, which is the one a
  /// user filling their disk cares about first.
  is_root: boolean;
}

/// The battery, which carries TWO different percentages.
///
/// Mirrors the Rust `health::Battery`, including the distinction that
/// struct exists to keep: `percent` is CHARGE (how full the cell is now,
/// moving minute to minute) and `capacity_percent` is HEALTH (how much
/// it can still hold relative to when it was made, moving over years).
///
/// The UI renders them in SEPARATE panels with different words. A
/// battery at 100% charge and 71% capacity is completely normal for an
/// older laptop, and a reader who sees "84%" beside a charge bar
/// concludes their battery is draining when in fact it has aged.
interface HealthBattery {
  /// CHARGE, 0-100.
  percent: number;
  on_ac: boolean;
  /// CAPACITY relative to design, 0-100 -- the figure usually called
  /// "battery health".
  ///
  /// `null` on every platform but macOS, and null on macOS where
  /// `ioreg` does not publish the pair it comes from. Never 0 for "not
  /// measured": a battery at 0% of its design capacity is a dead cell,
  /// the opposite claim from "we did not look".
  capacity_percent: number | null;
  /// Charge cycles, `null` where the platform does not publish it.
  ///
  /// The context that makes capacity readable: 84% after 400 cycles is
  /// ordinary ageing, and after 40 it is a fault.
  cycle_count: number | null;
  /// How fast power is moving in or out of the cell right now (#773).
  ///
  /// The THIRD distinct number this type carries, and the only one that
  /// is a rate rather than a level: `percent` is how full,
  /// `capacity_percent` is how full it can get, and this is which way
  /// and how fast it is moving.
  ///
  /// `null` where the platform does not publish it (every platform but
  /// macOS and Linux), and never a 0 standing in for that -- zero watts
  /// is a real reading, since a full battery on mains draws nothing.
  ///
  /// OPTIONAL as well as nullable, for the same version-skew reason as
  /// `HealthGpu.renderer_percent`: a sample stored before this shipped,
  /// or a desktop released before it, carries no such key at all.
  power?: HealthPowerFlow | null;
}

/// The power moving in or out of the battery at one instant (#773).
///
/// Mirrors the Rust `health::PowerFlow`. Exported because
/// `SystemHealthPage` renders a card for it and therefore has to name
/// the type.
///
/// # The sign is the whole point
///
/// `watts` is POSITIVE charging and NEGATIVE discharging. The two
/// platforms encode that very differently -- macOS prints a
/// two's-complement integer as unsigned, Linux publishes a magnitude
/// with the direction in a separate string -- and both are normalised
/// to this convention in Rust, so nothing here needs to know which
/// machine it is describing.
export interface HealthPowerFlow {
  /// Watts. Positive into the cell, negative out of it.
  watts: number;
  /// Milliamps, on the same sign convention as `watts`.
  milliamps: number;
  /// Millivolts at the terminals, always positive.
  ///
  /// Carried alongside the wattage because the wattage is a PRODUCT: a
  /// reader who sees an implausible figure has no way to tell which
  /// half is wrong without both factors.
  millivolts: number;
}

interface HealthInterface {
  name: string;
  /// Cumulative since boot, not since the last sample.
  rx_bytes: number;
  tx_bytes: number;
}

/// One process's network totals, mirroring the Rust
/// `health::NetProcess` in `src-tauri/src/health/netproc.rs` (#718).
///
/// Exported, unlike the four `HealthSample` helpers above, for the same
/// reason as `FootprintProcess` and `HealthGpu`: `SystemHealthPage`
/// renders a row per process and therefore has to name the type.
///
/// # These are NOT part of `HealthSample`, deliberately
///
/// Every other reading on this page rides the five-second health poll.
/// This one costs ~5 SECONDS per reading on macOS -- `nettop` samples
/// for a whole interval before printing anything -- which is the entire
/// poll interval, so it has its own command and its own slower cadence
/// and runs only while the Network detail page is open. Folding it into
/// `HealthSample` would put a five-second subprocess on a five-second
/// timer, which is #661's failure in its worst available form.
///
/// # Cumulative, so ONE reading is not a rate
///
/// `bytes_in` and `bytes_out` are totals since each PROCESS started --
/// the same contract as `HealthInterface`, whose counters are totals
/// since boot. A rate needs two readings differenced, which is why the
/// page is roughly TWENTY seconds from opening to its first rate — one
/// ~5s reading, the 15s cadence, then a second ~5s reading — and why it
/// has to say so rather than looking broken for that long.
export interface NetProcess {
  /// The process as the platform names it, PID stripped off. It matches
  /// the names the CPU and Memory pages list, which is what lets a
  /// reader follow one busy process across the three pages.
  name: string;
  /// The PID, or `null` when the platform's label carried no parseable
  /// one. Never invented: a row whose identity could not be established
  /// still has real byte counts worth showing.
  pid: number | null;
  /// Bytes received since the process started. Cumulative.
  bytes_in: number;
  /// Bytes sent since the process started. Cumulative.
  bytes_out: number;
}

/// What is using this machine at one instant, mirroring the Rust
/// `health::Footprint` in `src-tauri/src/health/footprint.rs`.
///
/// # Why it is still called a "footprint"
///
/// It began as Headstate's OWN cost (#665): three fields -- `app`,
/// `children`, `docker_daemon` -- behind a "What Headstate is costing"
/// panel on the System Health overview. #795 removed that panel and
/// those fields, for the reason argued in the Rust module: a
/// once-a-second sample of our own processes almost never catches the
/// bursty `git` fan-out that is the actual cost, so a calm 2% row told
/// users we were cheap, confidently and wrongly.
///
/// The NAME did not change with the fields. Renaming the command would
/// break the remote surface, whose allowlist names `system_footprint` as
/// a literal string in two separate copies (desktop and phone), and that
/// is a real compatibility cost for a word. So this reads as a misnomer
/// and this paragraph is the fix.
///
/// # Absent is not zero, again
///
/// The same rule as `HealthSample`, and the shape the absence takes here
/// is a failed MEASUREMENT rather than a missing process: every row below
/// is a process that demonstrably exists. `ProcessGroup.cpu_unmeasured`
/// is that rule's only remaining expression on this type -- a group whose
/// sum is over fewer processes than its count says so, rather than
/// folding an unreadable one in as a zero.
export interface Footprint {
  /// RFC 3339, stamped by the Rust side at the moment of the reading.
  sampled_at: string;
  /// The biggest CPU consumers on the WHOLE machine, biggest first
  /// (#687).
  ///
  /// The machine's processes, Headstate's own included on the same terms
  /// as everything else. This is what the CPU detail page shows, because
  /// "CPU is at 80%" is a symptom and "these are the processes" is the
  /// answer.
  ///
  /// A bounded TOP N (eight), never the full list: 1400-odd rows is not
  /// an answer to "what is using my CPU", it is the same filtering
  /// problem handed back to the reader. `process_count` says how many
  /// there were, so a short list never has to be mistaken for the whole
  /// machine.
  ///
  /// OPTIONAL, and the optionality is load-bearing rather than
  /// defensive. This was challenged on review, checked against the
  /// version machinery, and the answer is that the machinery does not
  /// reach this case. Worth spelling out, because the two mechanisms
  /// that look like they cover it are real -- they just bite elsewhere:
  ///
  /// - **The version gate is on WRITES.** `connection.rs`'s `blocked()`
  ///   refuses a desktop below `PROTOCOL_VERSION`, but its one caller
  ///   (`companion.rs`) guards it with `matches!(class, Class::Write |
  ///   Class::Destructive)` and says why: reads go through whatever the
  ///   state, because the attempt is how the phone learns the desktop
  ///   is back. `system_footprint` is `Class::Read`.
  /// - **Cert pinning refuses protocol 1, not "older".** ML-DSA-65
  ///   certificates were the 1-to-2 change (#521), so a 5.0 desktop
  ///   fails the handshake. A protocol-2 desktop from before this
  ///   feature completes it normally.
  ///
  /// What remains is not a protocol mismatch at all. The companion
  /// ships on its own tag, independent of the desktop's
  /// (`docs/mobile-release-process.md`), and compatibility is the wire
  /// protocol's integer -- which adding fields to a response does not
  /// bump, because doing so is backward-compatible. So a phone carrying
  /// this feature paired with a desktop released before it is a
  /// protocol-2-to-protocol-2 pairing: allowed, unblocked, and missing
  /// these fields. Two release pipelines make that ordering ordinary.
  ///
  /// The absence is a different fact from an empty list -- "that desktop
  /// cannot tell us" versus "nothing is running", the latter impossible
  /// on a booted machine -- and the UI renders the two differently.
  /// `call<Footprint>` is an unchecked cast, so a required type here
  /// would have the compiler certify a guarantee the wire does not give.
  top_cpu?: FootprintProcess[];
  /// The same, by resident size. A SEPARATE list rather than `top_cpu`
  /// re-sorted: the process pinning a core is rarely the one holding
  /// 8 GB, and re-sorting one list by the other metric would show the
  /// top of a set that was chosen by the wrong measure.
  ///
  /// Optional for the same version-skew reason as `top_cpu`.
  top_memory?: FootprintProcess[];
  /// How many processes were running when the two lists were taken.
  ///
  /// So the UI can say what it is not showing. Eight of 1436 is a
  /// defensible answer; eight presented as everything is not.
  ///
  /// Optional for the same reason as the two lists. When it is absent
  /// the UI omits the "of N running" sentence rather than inventing a
  /// total -- a count that does not exist must not be rendered as one
  /// that does.
  process_count?: number;
  /// The same two questions, asked of processes SUMMED BY NAME (#721).
  ///
  /// Computed on the Rust side over the full process list, and that is
  /// the point rather than an implementation detail. The UI only ever
  /// receives eight rows, so grouping them here could only merge names
  /// that already ranked individually -- which is exactly the case
  /// where grouping changes nothing. Measured on the reporting machine:
  /// 26 processes of one name held 13.0% of CPU and 6.6% of memory
  /// between them while the largest single one was 1.2%, so not one of
  /// them was in the individual top eight.
  ///
  /// Optional for the same version-skew reason as `top_cpu`: a desktop
  /// released before this feature answers a `system_footprint` read
  /// without these fields, and the UI must render that as "this desktop
  /// cannot group" rather than as "nothing grouped".
  top_cpu_grouped?: FootprintProcessGroup[];
  /// The same, by summed resident size. Separate from
  /// `top_cpu_grouped` for the same reason `top_memory` is separate
  /// from `top_cpu`.
  top_memory_grouped?: FootprintProcessGroup[];
}

/// Every process of one name, summed — one row of the Grouped view
/// (#721).
///
/// Grouped by NAME rather than by process ancestry. The reasoning is
/// recorded in full on the Rust `ProcessGroup`; the short version is
/// that the name is what a user recognises, that most macOS processes
/// reparent to `launchd` so a tree root names nothing, and that this
/// heuristic's error is visible in the output because the row carries
/// its count.
export interface FootprintProcessGroup {
  /// The shared process name, exactly as the OS reported it.
  name: string;
  /// How many processes carry this name — at least 1. Rendered beside
  /// the name (`acme-agent (26)`) so a grouped row can never be
  /// mistaken for a single process.
  count: number;
  /// Summed CPU as a percentage of ONE core, so a group of twenty-six
  /// busy processes legitimately reads far above 100. The UI must not
  /// clamp it, for the same reason it does not clamp a single process.
  cpu_percent: number;
  /// Summed resident set in bytes. Over-counts shared pages exactly as
  /// the individual rows do — a library mapped into all 26 is counted
  /// 26 times — which is why the "resident sets do not add up" note
  /// matters more under grouping, not less.
  memory: number;
  /// How many members reported an unusable CPU figure and were left
  /// OUT of `cpu_percent`.
  ///
  /// Zero on an ordinary machine. Non-zero means the sum covers fewer
  /// processes than `count` claims, which the UI says rather than
  /// presenting a partial total as a complete one — the "absent is not
  /// zero" rule, applied inside a sum.
  cpu_unmeasured: number;
}

/// One process in a `Footprint`.
///
/// Exported, unlike the four `HealthSample` helpers above, because
/// `SystemHealthPage` renders a row component that takes one of these
/// directly and therefore has to name the type.
export interface FootprintProcess {
  pid: number;
  /// The executable's own name as the OS reports it -- `git`, or
  /// `git.exe` on Windows. Never a full path.
  name: string;
  /// CPU use as a percentage of ONE core, so legitimately above 100 for
  /// a process using more than one -- a parallel build or a compiler
  /// routinely does. The UI must not clamp this the way it can clamp
  /// `HealthSample.cpu_percent`: clamping would report the busiest
  /// process on the machine as merely saturated, on the page whose whole
  /// job is to name it.
  cpu_percent: number;
  /// Resident set size in bytes: physical RAM held right now. Not
  /// virtual size, which on anything linking a webview is a large
  /// number that means nothing to a reader.
  memory: number;
}

/// What a stats load cost in GitHub rate-limit points.
///
/// Mirrors `github::stats::budget::Spend`. camelCase here, unlike most of
/// this file, because that type carries `#[serde(rename_all = "camelCase")]`
/// -- the whole stats layer from #827 does, and matching the Rust attribute
/// is what keeps these names honest rather than aspirational.
///
/// `points` is a FLOOR rather than a total when `unmetered` is non-zero: a
/// response that carried no `rateLimit` is counted as unmetered instead of
/// guessed at 1, because a guess recorded as a measurement is the defect
/// `budget.rs` exists to prevent.
///
/// Not exported: it is reached through `StatsTree.spend`, and `knip` fails
/// the lint on a type nothing imports by name. #826 will export it the
/// moment a component takes a spend as a prop.
interface Spend {
  points: number;
  requests: number;
  unmetered: number;
  /// The lowest remaining budget GitHub reported. `null` means nothing
  /// reported one, which is NOT the same as zero.
  remaining: number | null;
  resetAt: string | null;
}

/// One repository a stats question can be scoped to (#825).
export interface RepoRow {
  /// `owner/name` -- exactly what the `repo` scope value needs, so a clicked
  /// row needs no reassembly.
  nameWithOwner: string;
  /// When anything was last pushed, or `null` for a repository never pushed
  /// to.
  ///
  /// The rows are ordered by this, descending, server-side. It is shown
  /// because that ordering is otherwise invisible: a user cannot tell
  /// whether the twelfth row is a week stale or three years dead.
  ///
  /// CAVEAT, carried from the Rust side so it is not lost in translation:
  /// this is ANY push, not pull-request activity. A repository whose only
  /// recent commit was a dependency bot outranks one with a week-old human
  /// PR. Ordering by recent PR count instead would need one search per
  /// repository before the user clicked anything, which is the opposite of
  /// cheap discovery -- see `github::stats::tree`.
  pushedAt: string | null;
  isArchived: boolean;
}

/// One person a stats question can be scoped to.
export interface MemberRow {
  /// The login. The identity statistics are actually keyed on
  /// (`author:<login>`), which is why it is shown even when `name` is
  /// present -- a board of display names alone is unverifiable against
  /// GitHub's own UI.
  login: string;
  name: string | null;
  avatarUrl: string | null;
}

/// One organisation in the scope hierarchy.
export interface OrgTree {
  login: string;
  name: string | null;
  /// Repositories, most-recently-pushed first. May be shorter than
  /// `reposTotal` -- see that field.
  repos: RepoRow[];
  /// What GitHub says the true count is. Greater than `repos.length` means
  /// the list is a SAMPLE, and the UI must say so rather than present a
  /// truncated list as complete (#802 and #790 both shipped that bug).
  reposTotal: number;
  members: MemberRow[];
  membersTotal: number;
  /// Whether the organisation's contents could be read AT ALL.
  ///
  /// `false` means the token listed the org and was then refused its detail
  /// -- typically a SAML-SSO authorisation not granted, or a token without
  /// `read:org`. Both lists are empty in that case, and rendering that as
  /// "no members" is the #769 failure: silence read as success. The UI must
  /// branch on this flag, never on `members.length === 0`.
  readable: boolean;
}

/// The whole scope hierarchy the PR Stats sidebar renders (#825).
///
/// Enumerated from GitHub, never from a local checkout: where you happen to
/// have cloned something has no bearing on whose statistics you may want to
/// read. Costs 2 rate-limit points and carries NO statistics -- discovery is
/// cheap, measurement happens on click (`hooks.ts:712-717`).
export interface StatsTree {
  /// The authenticated login. The Personal section's scope value.
  viewer: string;
  orgs: OrgTree[];
  orgsTotal: number;
  /// The viewer's OWN repositories -- owner-affiliated, so this does not
  /// repeat the organisation sections.
  personal: RepoRow[];
  personalTotal: number;
  refusedFields: number;
  spend: Spend;
}

/// A complete count of pull requests for one subject and scope (#824).
///
/// Mirrors the Rust `github::stats::Outcome`. There is deliberately no way
/// to read `total` without the facts about whether it is exact sitting
/// beside it -- anything capped, sliced or assembled says so in the same
/// object, which is the requirement #824 item 8 states.
export interface StatsOutcome {
  /// The exact count, summed across every slice. Exact even when
  /// `retrievable` is false: the 1,000-result cap limits retrieval, not
  /// counting.
  total: number;
  /// Whether every pull request in the window could be RETRIEVED, not
  /// merely counted. False means per-PR detail is over a sample.
  retrievable: boolean;
  /// How many pull requests sit in slices whose nodes could not all be
  /// fetched.
  unretrievable: number;
  /// How many slices the window was cut into. > 1 means the total is
  /// assembled from more than one request.
  slices: number;
  /// Probe rounds the plan took.
  rounds: number;
  /// Whether the answer came from the uncapped `repository.pullRequests`
  /// connection or from capped, sliced `search`. Reported so a reader can
  /// tell WHICH completeness guarantee they have.
  viaConnection: boolean;
  spend: Spend;
  /// Fields GitHub refused on the responses behind this total. Non-zero
  /// means some data is missing, which is not the same as zero.
  refusedFields: number;
}

/// One person's activity in one scope and window (#826).
///
/// Every figure is a count over the pull requests actually RETRIEVED, which
/// is why completeness lives on `StatsBoard` rather than on a row: a row
/// cannot say whether it is short, because a short row looks exactly like a
/// smaller one. That is the whole failure mode of a ranking, and it is why
/// a leaderboard is less forgiving of missing data than a count -- a total
/// 5% short is a slightly wrong number, while a top-five 5% short can have
/// the wrong person in first place.
export interface AuthorRow {
  /// The GitHub login, which is the identity the search qualifier uses
  /// (`author:<login>`) and therefore the one a reader can check against
  /// GitHub's own UI.
  login: string;
  prs: number;
  /// Lines ADDED, summed. Raw `additions`, INCLUDING generated files --
  /// the label is the mitigation, not a fix (#823). See
  /// `LINES_CHANGED_LABEL`.
  additions: number;
  deletions: number;
  /// `changedFiles`, summed. The honest companion to the line count:
  /// 40,000 lines across 3 files is a generated diff and 40,000 across 300
  /// is a refactor, and only the pair distinguishes them.
  changedFiles: number;
  /// Reviews RECEIVED on this author's pull requests.
  ///
  /// Received, not given. It comes off `reviews { totalCount }` on a PR the
  /// author WROTE, so it measures how much review their work attracted. A
  /// board of reviews GIVEN would need `reviewed-by:<login>` -- one search
  /// per person, a different and far more expensive question. The label
  /// must say "received" or the figure reads as the opposite of what it is.
  reviewsReceived: number;
  /// Hours from open to merge for each of this author's MERGED pull
  /// requests, sorted ascending so `percentile()` can index it directly --
  /// the same contract `MergedDetail.cycle_time_hours` has.
  ///
  /// SHORTER than `prs` whenever the window holds open pull requests: an
  /// unmerged one has no cycle time, because measuring it against "now"
  /// would report unfinished work as slow. So this length must not be
  /// divided into `prs` or treated as the author's pull request count.
  cycleTimeHours: number[];
}

/// One pull request on a board, enough to name and open it.
///
/// Mirrors `MergedPr` so the scoped outliers render through the SAME
/// `Outliers` component rather than a second one.
export interface BoardPr {
  number: number;
  title: string;
  url: string;
  /// `owner/name`.
  repo: string;
  author: string;
  cycleTimeHours: number;
  /// Additions plus deletions -- the same gameable measure, and it carries
  /// the same label wherever it is shown.
  size: number;
}

/// A slice of the window whose pull requests could not all be retrieved.
///
/// Carried with its sizes rather than as a count, because "3 slices were
/// short" does not tell a reader whether the board is missing four pull
/// requests or four hundred.
export interface ShortSlice {
  from: string;
  to: string;
  /// What GitHub said the slice holds.
  issueCount: number;
  /// How many pull requests actually came back.
  retrieved: number;
}

/// Per-author aggregates for one scope, plus every way they could be wrong.
///
/// `viewer` travels WITH the board rather than being fetched separately,
/// because the two have to agree: a board fetched for one account and split
/// by a login cached from another -- two accounts on one machine, which the
/// Rust `Subject::cache_key` doc records as a real case -- would put the
/// viewer's own work under "Others" and show "no activity" for Mine.
export interface StatsBoard {
  /// The authenticated login. What splits the board into Mine and Others.
  viewer: string;
  /// The key this board's stored rows are filed under (#1093).
  ///
  /// Travels with the board for `viewer`'s reason: backfill progress
  /// events are app-global while the work is per-scope, so the page needs
  /// this scope's own key to tell its frames from another scope's.
  /// Re-deriving it here would be a second spelling of a key the Rust side
  /// already computes, and a disagreement would silently show no progress.
  scopeKey: string;
  /// One row per author who appears, in no ranking order -- the UI ranks by
  /// whichever measure its chart is about.
  rows: AuthorRow[];
  /// Pull requests GitHub says the window holds. Exact even when the rows
  /// are short: the 1,000-result cap limits retrieval, not counting.
  ///
  /// `null` means NOBODY HAS MEASURED IT (#1092) -- a board assembled from
  /// stored rows over a window the ledger has never probed. Never render it
  /// as 0: "400 of 0" is nonsense, and "400 of 400, complete" is worse
  /// because it reads as reassuring. The same discipline `Branch.ahead`
  /// keeps (#967), on the one field where the lie is invisible, since every
  /// ratio built from a wrong denominator still looks plausible.
  total: number | null;
  /// Pull requests actually aggregated into the rows.
  retrieved: number;
  /// Whether every pull request in the window made it into a row.
  ///
  /// The flag a ranking must branch on. False when anything was capped,
  /// sliced short, or refused -- so a new partiality channel added later
  /// cannot be forgotten at one call site.
  complete: boolean;
  truncatedSlices: ShortSlice[];
  /// Fields GitHub refused across the detail responses. Non-zero means some
  /// data is missing, which is NOT the same as being zero.
  refusedFields: number;
  /// How many slices the window was cut into. > 1 means assembled.
  slices: number;
  rounds: number;
  spend: Spend;
  /// The slowest MERGED pull requests in scope, slowest first. At most five.
  ///
  /// Merged only: "slowest to merge" is undefined for a pull request that
  /// has not merged, and including open ones would make the list a ranking
  /// of how long things have been open rather than how long they took.
  slowest: BoardPr[];
  /// The largest merged pull requests by lines changed. At most five.
  largest: BoardPr[];
  /// Merged pull requests per repository, most first. The scoped
  /// counterpart to `MergedDetail.repo_counts`.
  repoCounts: { repo: string; merged: number }[];
  /// Pull requests stored for this window across EVERY load (#1004).
  ///
  /// `retrieved` is what this one load managed; this is what the answer is
  /// assembled from. The pair is what distinguishes a shortfall that is
  /// CONVERGING from one that is STUCK -- today both read identically, and
  /// the reporter's complaint is that repeated loads never improve.
  accumulated: number;
  /// Whether pull requests are being written down for this window.
  ///
  /// False when storage was unavailable, so the caveat does not promise
  /// that another load will help when nothing is being kept.
  accumulating: boolean;
  /// Days of the window that have actually been retrieved (#1092).
  ///
  /// The figure a pull request count cannot give. "40% of the pull
  /// requests" is equally consistent with 40% of every day and with 100% of
  /// 40% of the days, and only the second tells a reader WHICH PART of the
  /// chart to trust -- usually the part they are looking at.
  daysCovered: number;
  /// Days in the window, the denominator of "34 of 90 days measured".
  daysTotal: number;
}

/// One day of scoped pull-request activity.
///
/// Field names match `HistoryPoint` so the scoped series renders through
/// the SAME `ActivityChart` rather than a second charting idiom (#826).
///
/// Not exported, like `UpdateOutcome` above: nothing outside this file names
/// it, since callers reach it through `StatsSeries.points`.
interface ScopedPoint {
  date: string;
  opened: number;
  merged: number;
}

/// The scoped daily series behind a scope page's activity chart.
export interface StatsSeries {
  points: ScopedPoint[];
  /// Days whose counts did not come back, NAMED rather than counted and
  /// never defaulted to zero. A missing day rendered as `0` would draw a
  /// trough that reads as a quiet Tuesday -- the most legible possible lie,
  /// because a chart invites the eye to read shape.
  failedDays: string[];
  refusedFields: number;
  spend: Spend;
  /// Why days are missing, when the reason is not GitHub's (#1050).
  ///
  /// Absent means the days that failed did so AT GitHub -- a document went
  /// unanswered, or a field was refused. Present means this process declined
  /// to issue the request at all, which is a different fact and the one the
  /// page previously could not tell: a budget-exhausted load matched the
  /// "GitHub did not answer ... usually clears on its own" branch while being
  /// wrong in both halves.
  unmeasured?: Unmeasured;
}

/// Why a load stopped short for a reason that is not GitHub's (#1050).
///
/// Mirrors the Rust `github::stats::fetch::Unmeasured`. A tagged union rather
/// than a boolean, so a second non-GitHub reason adds a variant instead of a
/// parallel flag nothing forces anyone to read.
export type Unmeasured = {
  kind: "budgetExhausted";
  /// The lowest remaining budget GitHub reported. `null` means nothing
  /// reported one, which is NOT the same as zero.
  remaining: number | null;
  reserve: number;
  resetAt: string | null;
};

/// One person's reviews GIVEN in a scope and window.
///
/// The counterpart to `AuthorRow.reviewsReceived`, and deliberately NOT a
/// field on it: the two come from different searches and name different
/// people. `reviewsReceived` reads `reviews { totalCount }` off a pull
/// request the row's author WROTE; this reads `reviewed-by:<login>`, which
/// finds pull requests by anyone that this person reviewed. MEASURED live
/// 2026-09-11: the two pull requests crediting the viewer as REVIEWER in an
/// org window were both authored by somebody else, so the author leads one
/// board and the reviewer the other on the same two rows of data.
export interface ReviewerRow {
  /// The GitHub login, which is the identity the search qualifier uses
  /// (`reviewed-by:<login>`) and so the one a reader can check against
  /// GitHub's own UI.
  login: string;
  /// Pull requests in scope, merged in the window, that this person
  /// reviewed.
  ///
  /// A MEASURED zero when it is zero. An unmeasured login is absent from
  /// `rows` and named in `unmeasured` instead -- never a `0` here, because a
  /// failed query rendered as zero would rank a colleague last on the
  /// strength of nothing.
  reviews: number;
}

/// The reviews-given leaderboard for one scope (#826).
export interface StatsReviewers {
  /// One row per login successfully counted, ranked highest first with ties
  /// broken on login. Includes measured zeroes; the UI is what declines to
  /// rank them (`Leaderboard.tsx`'s "a zero has no rank" rule).
  rows: ReviewerRow[];
  /// Logins whose count did not come back, NAMED rather than counted.
  ///
  /// The same rule `StatsSeries.failedDays` follows, and it binds harder on a
  /// ranking: "2 people could not be measured" does not say whether the
  /// leader might be one of them.
  unmeasured: string[];
  /// Fields GitHub refused. Its own channel rather than folded into
  /// `unmeasured`, because a refusal suggests a SAML authorization to fix
  /// while a missing alias suggests a retry.
  refusedFields: number;
  spend: Spend;
}

/// What a plugin is CAPABLE of contributing (#1075).
///
/// Read from the install path, never from usage. `read: false` means the
/// flags are absences of KNOWLEDGE, not absences of features, so the UI
/// must not turn them into a claim.
export interface PluginContribution {
  mcp: boolean;
  skills: boolean;
  agents: boolean;
  commands: boolean;
  /// Whether the install path could be read at all.
  read: boolean;
}

/// One installed plugin, from `installed_plugins.json` (#1075).
export interface InstalledPlugin {
  name: string;
  /// Where it came from. The source is the `@<marketplace>` half of the
  /// inventory key, not a field of the entry.
  marketplace: string;
  scope: string | null;
  version: string | null;
  install_path: string | null;
  installed_at: string | null;
  last_updated: string | null;
  contribution: PluginContribution;
}

/// One plugin's counted usage (#1075).
///
/// Every count here is of `tool_use` records ONLY. A plugin's tool names
/// also appear in every session's availability list, and counting those
/// reports unused plugins as the busiest -- see `claude/plugins.rs`.
export interface PluginUsage {
  name: string;
  mcp_calls: number;
  skill_calls: number;
  agent_calls: number;
  command_calls: number;
  failures: number;
  last_called_at: string | null;
  /// Whether a scan covered this plugin at all.
  ///
  /// `false` is "we have no reading", NOT "zero calls". The page renders
  /// the two differently, because a false zero here argues for
  /// uninstalling something the user relies on.
  measured: boolean;
  /// Engagement: work done on what this plugin owns (#1082).
  ///
  /// A SECOND reading beside the call counts, never added to them.
  footprint: PluginFootprint;
}

/// A plugin's engagement -- work done on what it owns (#1082).
///
/// Invocations are exact but are not a measure of value: `remember` has
/// 0 invocations and 406 memory files it wrote, because its whole
/// contribution is instructions the model then follows. This counts the
/// other thing, and the page shows both without ever blending them.
export interface PluginFootprint {
  /// Tool calls whose input touched a directory this plugin owns.
  calls: number;
  /// Those calls by the tool that made them. The shape is the argument:
  /// all-`Read` is the model consulting the plugin's material, all-
  /// `Write` is the plugin's output being produced.
  by_tool: Record<string, number>;
  /// Reads of files inside the plugin's own install path.
  install_reads: number;
  /// Whether this plugin has any owned directory declared at all.
  ///
  /// `false` means a zero above is "we cannot trace this plugin's
  /// footprint", NOT "it has none" -- so the UI must render words, never
  /// a bare `0`. Absent is not zero, at the level of the measurement's
  /// own applicability.
  owned_known: boolean;
}

/// One day of the plugin activity chart (#1075).
export interface PluginDayCount {
  /// `YYYY-MM-DD`, UTC.
  day: string;
  calls: number;
}

/// Installed plugins and what they were actually used for (#1075).
export interface PluginsReport {
  installed: InstalledPlugin[];
  usage: PluginUsage[];
  activity: PluginDayCount[];
  /// Transcripts that could not be read, `<path>: <why>`. While this is
  /// non-empty every count above is a FLOOR, and the page says
  /// "at least N".
  unreadable: string[];
  /// The inventory file could not be read, with why. Distinct from an
  /// empty inventory.
  inventory_failure: string | null;
  /// There is no inventory file: nothing is installed. A settled empty
  /// answer, not a failure.
  inventory_absent: boolean;
  /// How many transcripts were read this time; the rest were unchanged
  /// and came from the cache.
  scanned: number;
  elapsed_ms: number;
}
