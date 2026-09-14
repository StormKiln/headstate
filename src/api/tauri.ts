/// Typed wrappers around the Tauri command surface in
/// `src-tauri/src/commands.rs`. Every command returns `Result<T, String>` on
/// the Rust side, which Tauri surfaces as a *rejected* promise (not a
/// resolved `Err` value) when the Rust side returns `Err`. Callers that need
/// to distinguish "not authenticated" from a network failure should inspect
/// the rejection message; `AuthGate` covers the common case by gating
/// render on `get_auth_state` before anything else calls in.
///
/// Each wrapper goes through `transport.call` rather than Tauri's
/// `invoke` directly, so the same signatures can be served by a network
/// client on the mobile companion. On the desktop the transport IS
/// `invoke`; see `transport.ts`.

import { call } from "./transport";
import type {
  AlertReport,
  CachedSnapshot,
  Artifact,
  Branch,
  DeleteOutcome,
  ClaudeMdScan,
  ClaudeImported,
  ClaudeOverview,
  ClaudePreview,
  ClaudeSessionDetail,
  WireClaudeSessionList,
  ClaudeUsage,
  ClaudeSubagentRollup,
  ProjectReport,
  UpdateRequest,
  UpdateFilter,
  CleanupPrefs,
  LedgerEntry,
  Venv,
  VenvRemoval,
  ArtifactRemoval,
  Assessment,
  CycleTrend,
  DanglingVolume,
  DockerBuild,
  DockerDiskUsage,
  DockerImage,
  DockerState,
  Footprint,
  HealthSample,
  History,
  ImageRemovalOutcome,
  MergedDetail,
  NetProcess,
  Periods,
  PrDetail,
  PullRequest,
  Stats,
  StatsBoard,
  StatsOutcome,
  StatsSeries,
  StatsReviewers,
  StatsTree,
  Worktree,
  WorktreeScan,
} from "../types/pr";

export interface AuthState {
  ok: boolean;
  message: string;
}

/// The cached snapshot. Never talks to GitHub. Returns `[]` both when
/// nothing has ever been polled and when auth failed at startup -- callers
/// must consult `getAuthState` to tell those apart.
export const getCached = () => call<PullRequest[]>("get_cached");

/// A user-initiated, out-of-band fetch. Does not persist to SQLite and does
/// not affect the poll loop's cadence.
export const refreshNow = () => call<PullRequest[]>("refresh_now");

/// Interface preferences. Mirrors the Rust `UiPrefs`.
export interface UiPrefs {
  hidden_views: string[];
  close_hides_to_tray: boolean;
  announce_updates: boolean;
  /// Whether the Claude Code integrations are switched on (#916).
  ///
  /// A CAPABILITY, not a `hidden_views` entry. That list means "I do not
  /// want to see this" and `ViewSwitcher` honours it loosely on purpose --
  /// `ALWAYS_OFFERED` and the current-view hatch both override it. A
  /// switched-off integration must not survive either, because there is no
  /// page behind the entry. See the note at the `capabilityOff` check.
  claude_integrations_enabled: boolean;
  /// Whether to write the verbose `[diag]` timing log.
  ///
  /// Kept as a switch rather than removed after v3.5.3: the next
  /// "it is slow on my machine" report wants exactly this log, and
  /// asking someone to install a special build to produce it is much
  /// worse than a checkbox.
  diagnostic_logging: boolean;
  /// Days idle before a virtualenv counts as stale. 0 means the default.
  stale_venv_days: number;
  /// Charge below which the battery alert fires, in percent (#720).
  ///
  /// 0 means "never set" and resolves to 25 in Rust
  /// (`health::alerts::low_percent`), exactly like `stale_venv_days` --
  /// a stored 0 from an upgrade must not silently disable the alert.
  ///
  /// This is CHARGE, not capacity. See `HealthBattery`.
  battery_low_percent: number;
}

export const getUiPrefs = () => call<UiPrefs>("get_ui_prefs");
export const setUiPrefs = (prefs: UiPrefs) => call<void>("set_ui_prefs", { prefs });

/// Whether the app starts at login.
///
/// Asked of the OS rather than stored: the user can turn it off in
/// System Settings, and a stored flag would then disagree with reality.
export const getAutostart = () => call<boolean>("get_autostart");
export const setAutostart = (enabled: boolean) =>
  call<void>("set_autostart", { enabled });
/// Whether phones can connect to this desktop right now.
///
/// The live state rather than the stored setting, for the same reason
/// as autostart: the two differ when the port could not be bound at
/// startup, and the checkbox should say what is true.
export const getRemoteEnabled = () => call<boolean>("get_remote_enabled");
export const setRemoteEnabled = (enabled: boolean) =>
  call<void>("set_remote_enabled", { enabled });
/// What the app already knows about a worktree's unmerged work.
///
/// Per row rather than per scan: several git calls each, so this is
/// asked only when a user actually opens a row.
export const assessWorktree = (repoPath: string, worktreePath: string, branch: string) =>
  call<Assessment>("assess_worktree", { repoPath, worktreePath, branch });

/// Which desktop notifications the user wants.
///
/// Mirrors the Rust `NotifyPrefs`. Absent on the Rust side means
/// everything on, matching what the app did before the setting existed.
export interface NotifyPrefs {
  enabled: boolean;
  ci_failed: boolean;
  conflicted: boolean;
  /// Notify when a pull request enters the "Ready for review" set: green
  /// checks, no blockers, and the user is a requested reviewer.
  ready_to_review: boolean;
  /// Notify when a pull request APPEARS that was not there before
  /// (#789). Unlike `ready_to_review` this says nothing about state: a
  /// red, conflicted pull request appearing is still news.
  new_pr: boolean;
  /// Notify about this machine's battery: low charge, fast discharge, or
  /// discharging while plugged in (#720).
  ///
  /// One category for all three, because they are one subject to a
  /// person. Until #789 these notified unconditionally, with only the
  /// threshold below to adjust WHEN -- there was no way to want pull
  /// request notifications and not machine ones.
  health_battery: boolean;
  /// Notify when this machine's CPU is busy with nothing in particular
  /// (#791).
  health_cpu: boolean;
  /// Notify when a watched Claude Code session dies (#979).
  ///
  /// Gated TWICE on the desktop: by this and by
  /// `claude_integrations_enabled`, which defaults off. That second gate
  /// is why this one defaults ON despite being new -- nobody can be
  /// interrupted by it without having first asked the app to watch their
  /// Claude sessions. `poll.rs`'s field doc argues it.
  claude_crashed: boolean;
}

export const getNotifyPrefs = () => call<NotifyPrefs>("get_notify_prefs");

export const setNotifyPrefs = (prefs: NotifyPrefs) =>
  call<void>("set_notify_prefs", { prefs });

/// Re-run the failed jobs of a pull request's CI.
///
/// Takes the workflow RUN id, not a check id: one call re-runs every
/// failed job in that run, and a per-check call could not restart a job
/// that never started because an earlier one failed.
export const rerunChecks = (repo: string, number: number, runId: number) =>
  call<void>("rerun_checks", { repo, number, runId });

/// The platform and architecture this build was compiled for.
///
/// From Rust compile-time constants rather than the webview, so it
/// cannot disagree with the binary the user is running.
export const buildTarget = () => call<[string, string]>("build_target");

/// How many pull requests await the user's review.
///
/// A count, not a list: the sidebar badge needs a number on every view,
/// and asking for the list costs 6 rate-limit points and ~4s against 1
/// and ~0.9s for this.
export const countReviewing = () => call<number>("count_reviewing");

/// The authenticated user's login.
///
/// Asked once and cached forever: it cannot change during a session.
export const getViewer = () => call<string>("get_viewer");

/// A review verdict. Mirrors the Rust `ReviewVerdict`; the strings must
/// match `parse_verdict` in commands.rs exactly, which rejects anything
/// else rather than guessing.
///
/// GitHub's schema also has DISMISS, deliberately unreachable here: it
/// dismisses someone else's review, which nothing in the UI asks for.
export type ReviewVerdictName = "approve" | "request_changes" | "comment";

/// Submit a review on a pull request.
///
/// The first write to a PR the user does not own. `body` may be empty
/// only for `approve`; the Rust side rejects the other two without it
/// rather than letting GitHub refuse after a round-trip.
export const reviewPr = (
  id: string,
  repo: string,
  number: number,
  verdict: ReviewVerdictName,
  body: string,
) => call<void>("review_pr", { id, repo, number, verdict, body });

/// Comment on a pull request without reviewing it.
export const commentOnPr = (id: string, repo: string, number: number, body: string) =>
  call<void>("comment_on_pr", { id, repo, number, body });

/// Resolve a review conversation. Takes the THREAD's id, not the PR's.
export const resolveThread = (threadId: string, repo: string, number: number) =>
  call<void>("resolve_thread", { threadId, repo, number });

/// Reopen a resolved conversation -- the undo for `resolveThread`.
export const unresolveThread = (threadId: string, repo: string, number: number) =>
  call<void>("unresolve_thread", { threadId, repo, number });

/// Reply inside a conversation, keeping the answer attached to the code
/// it is about rather than starting a new top-level comment.
export const replyToThread = (
  threadId: string,
  repo: string,
  number: number,
  body: string,
) => call<void>("reply_to_thread", { threadId, repo, number, body });

/// `Stats.merged_week`/`merged_month` are real; the other five fields
/// always come back zero today. Does not persist to SQLite.
export const getStats = () => call<Stats>("get_stats");

/// The scope hierarchy the PR Stats sidebar renders (#825): organisations
/// with their repositories and members, plus the viewer's own repositories.
///
/// Sourced from GitHub, never from a local checkout. Carries NO statistics --
/// two requests, 2 rate-limit points total, measured -- because discovery is
/// cheap and measurement waits for a click (`hooks.ts:712-717`).
export const statsTree = () => call<StatsTree>("stats_tree");

/// A COMPLETE count of pull requests for one subject and scope (#824).
///
/// The first wrapper on the hardened stats layer. #827 shipped the command
/// with no caller, deliberately -- "shipping the command now is what makes
/// the layer reachable and testable rather than dead code waiting on two
/// other PRs" -- and this is #826 becoming that caller.
///
/// `subject` is a login, or `undefined` for the VIEWER -- not for everyone.
/// "Everyone" is a different question and is `statsBoard`'s; the Rust side
/// rejects an empty string rather than reading it as either.
///
/// The answer carries its own completeness (`complete`, `slices`,
/// `unretrievable`), because a count assembled from slices must not be
/// readable as a plain number without it.
export const statsCount = (
  subject: string | undefined,
  scopeKind: string,
  scopeValue: string | undefined,
  measure: "merged" | "opened",
  days: number,
) =>
  call<StatsOutcome>("stats_count", {
    subject,
    scopeKind,
    scopeValue,
    measure,
    days,
  });

/// The scope's per-author board: the numbers behind Mine and Others, and
/// the three leaderboards (#826).
///
/// The EXPENSIVE call in this feature, and the one a click pays for: it
/// probes the window, slices anything over the 1,000-result cap, and fetches
/// per-PR nodes for every slice. Refused before it starts if the rate-limit
/// budget is near its reserve, because the thing being protected is the poll
/// loop's standing obligation.
///
/// No `subject` parameter, deliberately. A board asks about EVERYONE in the
/// scope and ranks them; constraining it to one author renders a board with
/// one name on it. `viewer` comes back in the answer so Mine and Others can
/// be split from the one load rather than measured twice -- two loads could
/// disagree, and a Mine figure that contradicts the viewer's own row on the
/// leaderboard beside it is a contradiction a reader cannot resolve.
export const statsBoard = (
  scopeKind: string,
  scopeValue: string | undefined,
  measure: "merged" | "opened",
  days: number,
) => call<StatsBoard>("stats_board", { scopeKind, scopeValue, measure, days });

/// The scoped daily opened/merged series (#826).
///
/// The cheap half of a scope page: count-only searches, no per-PR nodes, so
/// it lands in about a second per ten days where a board over a busy
/// organisation is seconds of node fetching. A SEPARATE command for exactly
/// that reason -- `StatsPage` renders each query as it arrives rather than
/// behind one gate, which is the pattern `StatsPage.tsx:12-22` records.
///
/// `subject` IS accepted here, unlike on the board, and the asymmetry is the
/// point: a chart is about one line, so "this person, in this org" is a
/// legitimate and cheap question. `undefined` means the whole scope.
export const statsSeries = (
  subject: string | undefined,
  scopeKind: string,
  scopeValue: string | undefined,
  days: number,
) => call<StatsSeries>("stats_series", { subject, scopeKind, scopeValue, days });

/// The reviews-GIVEN leaderboard: who reviewed the most in a scope (#826).
///
/// One `reviewed-by:<login>` search per person, all aliased into ONE request.
/// MEASURED live 2026-09-11 against `org:FNX-Labs`: cost **1** at 4, 10 and
/// 36 aliases, at 0.84-1.04s, 1.26-1.50s and 3.62-4.15s -- so alias count is
/// free on rate limit and linear in latency, and this account's real 4-member
/// org is one request and one point. The cheapest query on the page.
///
/// # Why `logins` is an argument rather than something Rust derives
///
/// The roster is already on screen: `statsTree` enumerated it for the sidebar
/// at 2 points, and the caller holds `org.members` for the scope the user
/// clicked. Re-fetching it here would spend a request to re-derive a list the
/// frontend has, and the two reads could disagree with the Members rows
/// beside the board if a roster changed between them.
///
/// The honest consequence, which the UI states rather than this wrapper
/// hiding: the board covers the people in the list, not everyone who
/// reviewed. An outside collaborator or a bot is absent because nothing
/// enumerated them.
///
/// # What this is NOT
///
/// Not `statsBoard`'s review figure. That one reads `reviews { totalCount }`
/// off pull request nodes, which counts reviews a PR RECEIVED and credits its
/// AUTHOR -- so the two boards name different people on the same data.
/// MEASURED: a `reviewed-by:<viewer>` search over an org window returned two
/// pull requests, each authored by SOMEBODY ELSE and each carrying one review.
/// So the author leads the received board and the reviewer the given one. Both
/// ship, each labelled for what it measures.
export const statsReviewers = (
  scopeKind: string,
  scopeValue: string | undefined,
  days: number,
  logins: string[],
) => call<StatsReviewers>("stats_reviewers", { scopeKind, scopeValue, days, logins });

/// Repos and their worktrees, WITHOUT safety classification, and what the
/// walk could not read.
///
/// ~800ms for 37 repos and 295 worktrees; safe to block a view on.
///
/// A `WorktreeScan` rather than a bare `WorktreeRepo[]` since #951: a
/// repository whose worktree listing failed was dropped from the payload
/// entirely, so the page read it as "not a repository". The shortfall
/// rides in the SAME payload rather than in a second command, because a
/// second command would mean a second full walk -- which `useWorktrees`'
/// own comment and #846's `retry: false` reasoning both forbid.
export const listWorktrees = () => call<WorktreeScan>("list_worktrees");

/// Classify one repo's worktrees. Per repo, and STREAMING: each verdict
/// is also emitted on `worktree-safety` as it is reached.
///
/// "Four git calls each" is what this comment used to say, and it is
/// wrong in the direction that caused #830. The count is UNBOUNDED:
/// `content_landed` on the Rust side spends up to four git calls per
/// CHANGED FILE, so one branch touching 100 files is ~400 calls. That is
/// why a per-call timeout never bounded this pass, and why a 111-worktree
/// repository could show sizes, count to 111, and never resolve a single
/// safety verdict. MEASURED per worktree, serial: a 114ms median against
/// a 3295ms max inside ONE repository -- a 29x spread, because the cost
/// tracks changed files rather than worktree count.
///
/// Await this for the settled set; subscribe to `worktree-safety` for the
/// rows as they land. `useWorktreeSafety` does both.
export const classifyWorktrees = (repoPath: string) =>
  call<Worktree[]>("classify_worktrees", { repoPath });

/// Apply an action to a pull request.
///
/// Rejects with GitHub's own message on refusal -- "base branch was
/// modified" is display-ready and more useful than a substitute.
export const actOnPr = (
  id: string,
  repo: string,
  number: number,
  action: PrActionName,
) => call<void>("act_on_pr", { id, repo, number, action });

/// One worktree's outcome in a bulk removal. `error` is null on success.
export interface RemovalOutcome {
  path: string;
  error: string | null;
}

/// Remove several worktrees, each safety-checked independently at delete
/// time. Resolves with an outcome per worktree rather than throwing on
/// the first refusal: partial failure is the normal case.
export const removeWorktrees = (repoPath: string, worktreePaths: string[]) =>
  call<RemovalOutcome[]>("remove_worktrees", { repoPath, worktreePaths });

/// The newest published release, or null when this build is current.
///
/// Distribution is dmg/exe/deb/AppImage, so no package manager carries
/// updates: a user on a version with a launch-blocking bug otherwise has
/// no way to learn a fix exists.
export const latestRelease = () => call<string | null>("latest_release");

/// --- Docker -------------------------------------------------------

export const dockerState = () => call<DockerState>("docker_state");
export const dockerBuilds = () => call<DockerBuild[]>("docker_builds");
export const dockerImages = () => call<DockerImage[]>("docker_images");
export const dockerDiskUsage = () => call<DockerDiskUsage>("docker_disk_usage");
export const dockerRemoveImages = (ids: string[]) =>
  call<ImageRemovalOutcome[]>("docker_remove_images", { ids });
export const dockerDanglingVolumes = () => call<DanglingVolume[]>("docker_dangling_volumes");
export const dockerRemoveVolume = (name: string) =>
  call<void>("docker_remove_volume", { name });
/// Returns bytes actually freed, read from the command's own output
/// rather than echoed from an estimate.
export const dockerPruneCache = (until?: string) =>
  call<number>("docker_prune_cache", { until });
export const dockerRunningContainers = () => call<string[]>("docker_running_containers");
export const dockerRestart = () => call<void>("docker_restart");
export const dockerStart = () => call<void>("docker_start");

/// Worktrees handed to Claude Code and still at the head they were
/// assessed at. A branch that has moved since is dropped: the assessment
/// described a different state.
export const assessedWorktrees = () => call<string[]>("assessed_worktrees");

/// Remove a worktree the safety gate refuses. Reached only from a
/// confirmation opened after reading an assessment of that worktree.
export const removeWorktreeForced = (repoPath: string, worktreePath: string) =>
  call<void>("remove_worktree_forced", { repoPath, worktreePath });

/// Clear a worktree's lock (#775). Removes nothing -- `git worktree
/// lock` puts it back -- but it clears a guard, so it is reached only
/// from a confirmation naming the holder and the age.
export const unlockWorktree = (repoPath: string, worktreePath: string) =>
  call<void>("unlock_worktree", { repoPath, worktreePath });

/// Clear a repository's stale worktree registrations (#793). Resolves
/// with how many went.
///
/// Takes no worktree path, because `git worktree prune` takes none: it
/// is repo-wide, and a per-row signature would promise a scope git does
/// not offer. Deletes nothing recoverable -- every registration it
/// clears describes a directory git has already reported gone -- so
/// unlike the removal calls it is reached without a confirmation.
export const pruneWorktrees = (repoPath: string) =>
  call<number>("prune_worktrees", { repoPath });

/// The clipboard payload for Claudify, plus whether Claude Code was
/// found. `claude_installed` is advisory: the command is returned either
/// way, since a user may paste it on another machine.
export interface ClaudifyCommand {
  command: string;
  claude_installed: boolean;
}

/// The shell command that hands a worktree to Claude Code.
///
/// Text for the clipboard, not a spawn. Spawning a terminal is not
/// portable -- macOS has no default-terminal concept, and on Linux
/// `gio open` on a shell script opens an editor -- and the clipboard
/// lands the user in their own shell, where `claude` resolves even
/// though a GUI app's PATH does not include it.
export const claudifyCommand = (repoPath: string, worktreePath: string, branch: string) =>
  call<ClaudifyCommand>("claudify_command", { repoPath, worktreePath, branch });

/// Merge a pull request when its checks pass, or cancel that.
///
/// Takes the head OID the row was rendered from: auto-merge fires later
/// and unattended, so without the guard a push after enabling would
/// merge a commit the user never saw.
export const setAutoMerge = (
  id: string,
  repo: string,
  number: number,
  expectedHead: string,
  enable: boolean,
) => call<void>("set_auto_merge", { id, repo, number, expectedHead, enable });

/// Delete a merged pull request's head branch.
///
/// `merged` is re-checked on the Rust side: deleting the head ref of an
/// OPEN pull request closes it off.
export const deleteHeadBranch = (
  refId: string,
  repo: string,
  number: number,
  branch: string,
  merged: boolean,
) => call<void>("delete_head_branch", { refId, repo, number, branch, merged });

/// Merge the base branch into a pull request's head -- GitHub's "Update
/// branch" button.
///
/// Separate from `actOnPr` because it needs `expectedHead`: GitHub
/// refuses if the branch moved since the row was rendered, so a stale
/// click reports an error rather than updating a commit the user never
/// saw. Pass the `head_oid` from the same row that showed the button.
export const updatePrBranch = (
  id: string,
  repo: string,
  number: number,
  expectedHead: string,
) => call<void>("update_pr_branch", { id, repo, number, expectedHead });
/// One pull request's outcome in a batch. `error` is null on success.
export interface BatchOutcome {
  repo: string;
  number: number;
  error: string | null;
}

/// Apply one action to several pull requests.
///
/// Returns an outcome per pull request rather than throwing on the first
/// rejection: partial failure is the normal case for a batch, and a
/// single verdict would hide the rejections.
export const actOnPrs = (
  prs: [string, string, number][],
  action: PrActionName,
) => call<BatchOutcome[]>("act_on_prs", { prs, action });

/// The actions the backend accepts. A union rather than `string`, so a
/// typo is a compile error instead of a runtime "unknown action".
export type PrActionName =
  | "merge"
  | "close"
  | "reopen"
  | "draft"
  | "ready"
  | "enqueue"
  | "dequeue";

/// Everything the detail view shows for one pull request. Cost 1.
export const getPrDetail = (repo: string, number: number) =>
  call<PrDetail>("get_pr_detail", { repo, number });

/// Disk sizes for one repo's worktrees, as `[path, bytes]` pairs.
///
/// A full tree walk -- ~13s for 147 worktrees -- so it is a separate
/// query from listing and classification, and arrives last.
///
/// `bytes` is null for a worktree whose walk exceeded the Rust side's
/// per-worktree budget (#769). Null, not absent and not 0: a missing
/// entry leaves the row on a skeleton forever, which is the bug, and 0
/// claims an empty tree the walk never actually saw the bottom of.
export const sizeWorktrees = (repoPath: string) =>
  call<[string, number | null][]>("size_worktrees", { repoPath });

/// Remove a worktree. Rejects anything not provably safe; the gate is
/// re-evaluated on the Rust side rather than trusted from the last scan.
/// Fast-forward a checkout to its upstream. Refuses on a dirty tree and
/// fast-forwards only; returns git's own output or its own refusal.
export const pullCheckout = (path: string) => call<string>("pull_checkout", { path });

/// Refresh one repository's remote refs, moving no branch (#788).
///
/// The counterpart to `pullCheckout` and deliberately not a flag on it:
/// a pull answers "make me current", this answers "tell me the truth".
/// Every verdict on the Worktrees page is computed against `origin/*` on
/// disk and the scan never fetches, so a repository nothing has fetched
/// lately reports local `main` and a frozen `origin/main` as agreeing --
/// green, honestly, about two refs that are both behind.
///
/// Resolves to git's own output, which for a successful fetch is usually
/// the EMPTY STRING: git writes its progress to stderr and says nothing
/// on stdout when there was nothing new. A caller must phrase its own
/// success line rather than showing this.
export const fetchRefs = (path: string) => call<string>("fetch_refs", { path });

/// Delete an orphaned worktree directory.
///
/// Separate from `removeWorktree` because git cannot remove it -- the
/// repository that owned it is gone. The Rust side re-checks that the
/// path is still orphaned before deleting anything.
export const removeOrphan = (path: string) => call<void>("remove_orphan", { path });

export const removeWorktree = (repoPath: string, worktreePath: string) =>
  call<void>("remove_worktree", { repoPath, worktreePath });

/// Tell the poll loop whether the active view needs live PR data.
export const setViewNeedsGithub = (needs: boolean) =>
  call<void>("set_view_needs_github", { needs });

/// Directories scanned for git checkouts. Defaults to `~/code`.
export const getWorktreeDirs = () => call<string[]>("get_worktree_dirs");

/// Replace the scanned directories. Rejects paths that are not
/// directories, so a typo fails here rather than yielding an empty view.
export const setWorktreeDirs = (dirs: string[]) =>
  call<string[]>("set_worktree_dirs", { dirs });

/// The configured focused poll interval, in seconds.
export const getPollInterval = () => call<number>("get_poll_interval");

/// Set the poll interval. Returns the value actually applied, which may be
/// clamped -- the UI shows what the backend accepted, not what was asked.
export const setPollInterval = (secs: number) =>
  call<number>("set_poll_interval", { secs });

/// PRs awaiting the user's review. Rides along in the same GraphQL
/// document as the authored list, so it costs no extra rate limit.
export const getReviewing = () => call<PullRequest[]>("get_reviewing");
/// The last successful review list, straight from SQLite. Never talks
/// to GitHub.
export const getCachedReviewing = () => call<CachedSnapshot>("get_cached_reviewing");

/// Median cycle time this week against last, in one request.
export const getCycleTrend = () => call<CycleTrend>("get_cycle_trend");

/// The period comparisons alone -- one small request (~1.6s) so the delta
/// cards paint without waiting on the daily series.
export const getPeriods = () => call<Periods>("get_periods");

/// The daily opened/merged series plus period comparisons. Fetched as
/// concurrent chunks on the Rust side. `days` is clamped to 1..=90.
export const getHistory = (days: number) => call<History>("get_history", { days });

/// Aggregates over the most recent 100 merged PRs. A separate command from
/// `getHistory` on purpose: it is the more expensive of the two and only
/// the insight row needs it, so a failure here must not blank the chart.
export const getMergedDetail = () => call<MergedDetail>("get_merged_detail");

/// Computed once at startup from the `gh` CLI token. `ok: false` means the
/// user needs to run `gh auth login`; `message` is ready-to-display prose.
export const getAuthState = () => call<AuthState>("get_auth_state");

/// Regenerable build output under the configured scan roots.
///
/// Discovery only: every `size_bytes` comes back null. See
/// `sizeArtifacts` for the second pass.
export const scanArtifacts = () => call<Artifact[]>("scan_artifacts");

/// Sizes for specific artifact directories, as
/// `[path, bytes, secsSinceWrite]`.
export const sizeArtifacts = (paths: string[]) =>
  call<[string, number, number | null][]>("size_artifacts", { paths });

/// Remove artifact directories. Each is re-verified at delete time, so a
/// stale row is refused rather than acted on.
export const removeArtifacts = (paths: string[]) =>
  call<ArtifactRemoval[]>("remove_artifacts", { paths });

/// Poetry virtualenvs, classified. Discovery only: sizes and idle times
/// come from `sizeVenvs`.
export const scanVenvs = () => call<Venv[]>("scan_venvs");

/// Sizes and idle times, as `[path, bytes, idleSecs]`.
export const sizeVenvs = (paths: string[]) =>
  call<[string, number, number | null][]>("size_venvs", { paths });

/// Remove virtualenvs. Each is re-verified at delete time.
export const removeVenvs = (paths: string[]) =>
  call<VenvRemoval[]>("remove_venvs", { paths });

/// Record that a human read an assessment for this worktree.
///
/// Deliberately NOT done by `claudifyCommand`: copying a prompt is the
/// start of an assessment, and the flag this sets unlocks removing a
/// worktree past its safety gate.
export const markAssessed = (worktreePath: string) =>
  call<void>("mark_assessed", { worktreePath });

/// Forget that a worktree was assessed, restoring its Claudify action.
export const clearAssessed = (worktreePath: string) =>
  call<void>("clear_assessed", { worktreePath });

/// Run the cleanup pass now and return what it WOULD remove.
///
/// Preview only: the backend has no removal path for this, so it cannot
/// delete regardless of what it is called with.
export const previewCleanup = () => call<LedgerEntry[]>("preview_cleanup");

/// The cleanup ledger, newest first.
export const cleanupLog = () => call<LedgerEntry[]>("cleanup_log");

export const getCleanupPrefs = () => call<CleanupPrefs>("get_cleanup_prefs");
export const setCleanupPrefs = (prefs: CleanupPrefs) =>
  call<void>("set_cleanup_prefs", { prefs });

/// Which dependencies are out of date in one repository.
export const checkPackages = (repoPath: string) =>
  call<ProjectReport[]>("check_packages", { repoPath });

/// The updates as markdown, for handing to an agent.
export const packagesMarkdown = (
  repoPath: string,
  reports: ProjectReport[],
  filter: UpdateFilter,
) => call<string>("packages_markdown", { repoPath, reports, filter });

/// Create a worktree and apply updates in it. Does NOT push.
///
/// Returns where the work landed and what each update actually did.
/// Push the run's branch and open a pull request.
///


/// Reveal the diagnostic log in the file manager. Returns its path.
export const revealLog = () => call<string>("reveal_log");

/// Every CLAUDE.md in a repository, with its import tree resolved, AND
/// what the scan could not read (#972).
export const scanClaudeMd = (repoPath: string) =>
  call<ClaudeMdScan>("scan_claude_md", { repoPath });

/// The text of one file, for rendering.
export const readClaudeMd = (path: string) => call<string>("read_claude_md", { path });

/// Rescan `~/.claude/projects` into our own cache. Returns what it read
/// AND what it could not read.
export const claudeImportTranscripts = () => call<ClaudeImported>("claude_import_transcripts");

/// Every stored Claude Code session, with liveness derived NOW (#917).
///
/// Liveness is never stored, so this is the only way to know it -- see
/// `ClaudeSessionList`. Rejects when the DATABASE could not be read,
/// which is different from an empty list and must never render as "you
/// have no sessions".
///
/// Returns the WIRE shape: every row, with each liveness reason interned
/// against `reasons` (#985). `useClaudeSessions` hydrates it, and nothing
/// below that hook sees an index.
export const claudeSessions = () => call<WireClaudeSessionList>("claude_sessions");

/// What ONE selected session knows that the list does not carry (#985).
///
/// `Class::Read`, so the phone gets it -- and the phone is who the split
/// is for: the list crosses the pairing transport every ten seconds, and
/// it was carrying every session's resume command and transcript path to
/// render one.
///
/// Resolves to `null` when the store has no such id, which is what a
/// session deleted between two polls produces. That is an ANSWER; a
/// rejection means the database could not be read. The view words them
/// differently and must never collapse them (#846).
export const claudeSessionDetail = (sessionId: string) =>
  call<ClaudeSessionDetail | null>("claude_session_detail", { sessionId });

/// Reveal a session's directory or transcript in the file manager.
/// Returns the path on success.
///
/// `Class::Local` -- the phone has no Finder to reveal into -- so every
/// caller sits behind `IS_MOBILE_BUILD`.
export const claudeRevealPath = (path: string) =>
  call<string>("claude_reveal_path", { path });

/// Aggregates for the Claude Code overview page (#921).
///
/// Two SELECTs over the cache plus a stat per session and one directory
/// listing; measured at 7ms for 1,461 sessions. Writes nothing.
export const claudeOverview = () => call<ClaudeOverview>("claude_overview");

/// How much work happened inside one session, from its own transcript
/// (#959).
///
/// `Class::Read`, so the phone gets it: "was that the long session or the
/// typo" is how a user picks which of 1,475 rows to resume, and nothing
/// else on the row answers it.
///
/// On DEMAND, never on the import path -- the rollup is 11x the startup
/// scan's head+tail read, which is why `claude::usage`'s budget exists and
/// why this takes a path rather than being folded into `claude_sessions`.
///
/// Rejects when the transcript could not be read. A resolved `messages: 0`
/// means it WAS read and carried no usage, which is a different fact.
export const claudeSessionUsage = (path: string) =>
  call<ClaudeUsage>("claude_session_usage", { path });

/// What one session's subagents cost, as a figure of its own (#1002).
///
/// ON DEMAND, for the selected session only. It reads one bounded
/// transcript PER CHILD, and the measured corpus has parents with dozens
/// -- so on the list's 10-second poll over 1,524 rows this would be the
/// whole-corpus read `claudeSessionUsage` above already refuses.
///
/// Never added into the parent's own usage. See `ClaudeSubagentRollup`.
export const claudeSubagentRollup = (sessionId: string) =>
  call<ClaudeSubagentRollup>("claude_subagent_rollup", { sessionId });

/// The tail of one session's transcript, as conversation (#982).
///
/// `Class::Read`, and the one Claude action whose phone case is stronger
/// than the desktop's: `claudeRevealPath` is `Class::Local`, so without
/// this a companion user can see that a session died and not one word of
/// what it was doing.
///
/// Bounded inside the command -- a 256 KB window, at most 200 messages,
/// each block clamped -- so the 76 MB transcript on the development
/// machine cannot be pulled over the pairing transport.
export const claudeTranscriptTail = (path: string) =>
  call<ClaudePreview>("claude_transcript_tail", { path });

// ---------------------------------------------------------------------
// The Claude Code hook installer (#915). Rust side:
// `src-tauri/src/claude/install.rs`, where every rule here is argued.
// ---------------------------------------------------------------------

/// Why an install, uninstall or status read could not proceed.
///
/// Mirrors the Rust `Refusal`, which is serialised with an internal `kind`
/// tag. Each variant exists because the REMEDY differs -- `malformed` needs
/// an editor and `io` needs permissions -- so the UI must not flatten them
/// to one message.
export type ClaudeHookRefusal =
  | { kind: "malformed"; path: string; detail: string }
  | { kind: "hooks_not_an_object"; path: string; found: string }
  | { kind: "matcher_not_understood"; path: string; event: string }
  | { kind: "io"; path: string; detail: string };

/// Whether the hooks are installed -- three states, because two would lie.
///
/// `cannot_tell` is the state that must NOT render as `not_installed`. Claude
/// Code silently ignores a `settings.json` it cannot parse, so a user in that
/// state has every hook in the file dead with no symptom -- and the remedy is
/// to fix the JSON, not to press Install. Rendering the two the same sends
/// them round a loop with no explanation.
///
/// # Why `cannot_tell` is an INTERSECTION rather than a nested field
///
/// The Rust enum is `#[serde(tag = "state")]` with `CannotTell(Refusal)` as a
/// newtype variant, and serde FLATTENS a newtype variant's fields into the
/// same object. So the refusal's own `kind`, `path` and `detail` arrive as
/// siblings of `state`, not under a `refusal` key.
///
/// Spelling it the intuitive way (`{ state: "cannot_tell"; refusal: ... }`)
/// would compile, type-check, and read `undefined` at runtime -- a silent
/// failure in the very code path whose whole purpose is to explain a silent
/// failure. `install::tests::the_wire_shape_matches_the_typescript_type`
/// pins the JSON on the Rust side, because nothing generates this type.
export type ClaudeHooksStatus =
  | { state: "installed"; command: string }
  | { state: "not_installed" }
  | { state: "stale"; detail: string }
  | ({ state: "cannot_tell" } & ClaudeHookRefusal);

/// What an install changed.
export interface ClaudeHooksInstalled {
  /// The command line written into every matcher.
  command: string;
  /// Events that had no hook of ours and now have one.
  added: string[];
  /// Events where a hook of ours was dropped and rewritten.
  ///
  /// Surfaced rather than swallowed: this is the count that tells a user
  /// their hand-edit was reverted, and reverting an edit without saying so
  /// is its own defect.
  replaced: string[];
  /// True when the settings file did not exist and was created.
  created_file: boolean;
}

/// What an uninstall removed.
export interface ClaudeHooksUninstalled {
  /// Events a hook of ours was removed from.
  removed: string[];
  /// True when there was nothing of ours to remove. Not an error.
  was_absent: boolean;
}

/// Whether the hooks are in `~/.claude/settings.json` right now.
///
/// Read every time, never cached: a cached "installed" is wrong the moment
/// the user hand-edits the file, and this is a file we invite them to edit.
export const claudeHooksStatus = () => call<ClaudeHooksStatus>("claude_hooks_status");

/// Install the hooks, appending to whatever is already there.
///
/// Also the reinstall: it drops every matcher it recognises as ours and
/// appends one fresh matcher per event, so running it twice leaves one.
export const claudeInstallHooks = () =>
  call<ClaudeHooksInstalled>("claude_install_hooks");

/// Reinstall. Identical to installing, by design -- a separate button with a
/// separate meaning, sharing one code path so the repair cannot drift.
export const claudeReinstallHooks = () =>
  call<ClaudeHooksInstalled>("claude_reinstall_hooks");

/// Remove Headstate's hooks and nothing else.
export const claudeUninstallHooks = () =>
  call<ClaudeHooksUninstalled>("claude_uninstall_hooks");

/// Every branch in a repository, classified.
///
/// Slow by nature -- ~9s on a 675-branch repository, most of it the
/// patch-id comparison that finds squash merges. The caller shows a
/// loading state rather than pretending this is instant.
export const listBranches = (repoPath: string) =>
  call<Branch[]>("list_branches", { repoPath });

/// Delete local branches. Each one is re-checked at delete time.
export const deleteBranches = (repoPath: string, names: string[]) =>
  call<DeleteOutcome[]>("delete_branches", { repoPath, names });

/// Delete branches ON THE REMOTE.
///
/// Deliberately a different function from `deleteBranches`: this is a
/// push to shared state that no reflog can undo.
export const deleteRemoteBranches = (repoPath: string, names: string[]) =>
  call<DeleteOutcome[]>("delete_remote_branches", { repoPath, names });

/// Apply updates and open a pull request, in the background.
///
/// Returns as soon as the run STARTS. The outcome arrives on the
/// `update-run-done` event -- the wizard used to await the whole run
/// with its modal open, which on a large selection meant minutes of an
/// unchanging "Applying…" and an unusable app (#495).
export const applyUpdatesInBackground = (
  repoPath: string,
  requests: UpdateRequest[],
  branch?: string,
) => call<void>("apply_updates_in_background", { repoPath, requests, branch });

/// What a background update run produced.
export interface UpdateRunDone {
  repoPath: string;
  /// The pull request, when one was opened. Null means none exists —
  /// never a claim that one does.
  url: string | null;
  branch: string | null;
  applied: number;
  failed: number;
  /// Whether the user stopped it. Distinct from `error`: a cancelled
  /// run did not fail, and the packages that landed before the stop
  /// really did land.
  cancelled: boolean;
  /// Why there is no pull request, when there is none.
  error: string | null;
}

/// Ask a background update run to stop.
///
/// It stops after the package it is on, never during one -- a package
/// manager killed halfway leaves a worktree in a state nobody asked
/// for. Rejects when nothing is running in that repository.
export const cancelUpdateRun = (repoPath: string) =>
  call<void>("cancel_update_run", { repoPath });

/// How a repository's update run is going, or how it ended.
///
/// The read for a client that was not listening. Progress and
/// completion are events, and a suspended phone holds no event stream,
/// so one that started a run and slept missed every frame including the
/// terminal one. Null when this desktop has run none for that repo.
export type UpdateRunState =
  | { state: "running"; done: number; total: number }
  | { state: "done"; outcome: UpdateRunDone };

export const updateRunState = (repoPath: string) =>
  call<UpdateRunState | null>("update_run_state", { repoPath });

// ---------------------------------------------------------------------
// Phone pairing (mobile companion). Rust side: src-tauri/src/remote/pairing.rs
// Callers: the pairing hooks in hooks.ts, behind Settings > Phone.
// ---------------------------------------------------------------------

/// What the pairing QR code encodes. Field names are the wire format the
/// phone parses, so they stay snake_case and terse.
export interface PairingQrPayload {
  v: 1;
  name: string;
  /// Every non-loopback address of this machine, IPv4 first, overlay
  /// addresses included; the phone tries them in order.
  addrs: string[];
  port: number;
  /// `sha256:<hex>` of the desktop certificate.
  fp: string;
  /// base64url, single use, expires at `exp`.
  token: string;
  /// Unix seconds.
  exp: number;
}

/// Settings > Pair a phone. Mints a two-minute, single-use token and
/// returns what to render as a QR code. Rejects until the desktop has a
/// certificate.
export const issuePairingToken = () =>
  call<PairingQrPayload>("issue_pairing_token");

/// Payload of the `pairing-request` event: a phone has proved it holds
/// the token and is waiting on the user's decision.
export interface PairingRequest {
  request_id: number;
  device_name: string;
  /// Lowercase hex, no prefix. Show it in blocks of four; the phone shows
  /// the same string so the two can be compared.
  fingerprint: string;
  /// Whether the phone offered a post-quantum step-up key.
  has_mldsa: boolean;
}

/// Answer a `pairing-request`.
///
/// `replaceExisting` matters only when a device with the same name is
/// already paired (check `listPairedDevices` when the event arrives):
/// `true` replaces it, `false` keeps both, and leaving it out rejects
/// with a message naming the device while the request stays pending --
/// so the modal can ask "replace or keep both?" and answer again. The
/// desktop never picks either on its own.
export const respondToPairing = (
  requestId: number,
  approve: boolean,
  replaceExisting?: boolean,
) =>
  call<void>("respond_to_pairing", {
    requestId,
    approve,
    replaceExisting: replaceExisting ?? null,
  });

/// A paired phone as Settings lists it. No key material.
export interface PairedDevice {
  id: number;
  name: string;
  /// Lowercase hex, no prefix.
  cert_fp: string;
  has_mldsa: boolean;
  /// RFC 3339.
  paired_at: string;
  /// RFC 3339, or null until the device's first connection after pairing.
  last_seen: string | null;
}

export const listPairedDevices = () =>
  call<PairedDevice[]>("list_paired_devices");

/// Delete the row and close that phone's open connections. A second
/// click on an already-revoked device resolves rather than rejects.
export const revokePairedDevice = (id: number) =>
  call<void>("revoke_paired_device", { id });

/// The machine's health right now, sampled on demand.
///
/// A fresh reading each call rather than the newest stored row: the
/// panel's headline numbers are "what is happening", and reading them
/// out of the minute-resolution history would show a value up to a
/// minute stale beside a chart that is honest about its resolution.
///
/// `Class::Read`, so the phone can ask a paired desktop for this and
/// get the DESKTOP's health -- which is the point of the companion's
/// version of the view.
export const systemHealth = () => call<HealthSample>("system_health");

/// The last 24 hours, downsampled server-side.
///
/// Bounded at `store::health::MAX_POINTS` (120) inside the SQL, so this
/// cannot return the raw ~1440-row series however long the app has been
/// running. The bound lives on the Rust side deliberately: the phone
/// reads this over the LAN, and an unbounded payload there is the
/// mistake that made `size_worktrees` time out (#661).
///
/// The result is oldest-first and is NOT evenly spaced in time. Rows
/// are bucketed by position, not by clock, so a stretch when the app
/// was closed comes back as two adjacent samples hours apart rather
/// than as a run of empty buckets. Consumers must therefore look at
/// `sampled_at` to find the gaps -- see `splitOnGaps` in
/// `SystemHealthPage`.
export const systemHealthHistory = () =>
  call<HealthSample[]>("system_health_history");

/// Every health condition that is true right now (#864).
///
/// Not transitions: the Rust side returns the full current set so two
/// clients cannot each see half the alerts. Deduplicate on `key`, which
/// is stable across changing figures.
///
/// This is the CPU runaway rules' only path to a screen. They were
/// evaluated on every poll and by this command, and until #864 nothing
/// in the frontend called it -- so an 8.5-hour, 12-process runaway
/// produced no visible output anywhere. Bounded like
/// `systemHealthHistory` by being a fixed small set of conditions rather
/// than a series, so it is safe to read over the LAN from the phone.
export const healthAlerts = () => call<AlertReport[]>("health_alerts");

/// What is using this machine, right now (#687, #721).
///
/// The machine's top processes by CPU and by resident set, the same two
/// summed by name, and the total process count. A kernel read of the
/// already-open process table -- no subprocess, no directory walk -- so
/// it is as cheap as `systemHealth` and safe to poll beside it.
///
/// # The name is a leftover, on purpose
///
/// It fed #665's "What Headstate is costing" panel, which reported our
/// own process, the `git`/`gh`/Docker subprocesses we spawn, and the
/// Docker daemon. #795 removed the panel and those three fields: a
/// once-a-second sample could not catch the bursty `git` fan-out that is
/// our real cost, so it told users we were cheap when we were not
/// measurable this way.
///
/// The COMMAND STRING could not follow. It is matched as a literal in
/// two remote-surface allowlists, one of which ships in the phone app on
/// its own release tag, so renaming it breaks a phone paired with an
/// older desktop. A misnomer with a paragraph beats a wire break.
///
/// There is deliberately no companion wrapper for disk sizing. Those
/// figures come from `sizeWorktrees`, `sizeArtifacts`, `sizeVenvs` and
/// `dockerDiskUsage`, which already exist above and are what the
/// Worktrees, Artifacts and Docker views show. They take seconds to tens
/// of seconds (`size_worktrees` was the #661 timeout at ~13s for 147
/// worktrees) and must never share a call site with something this
/// cheap. #796 removed the last view that summed all four.
///
/// `Class::Read`, so the phone can ask a paired desktop for this and get
/// the DESKTOP's answer -- which is the only reading that makes sense
/// there: it is the machine you left running.
export const systemFootprint = () => call<Footprint>("system_footprint");

/// Which processes are using the network, right now (#718).
///
/// # This call takes about FIVE SECONDS to return
///
/// Not a slow network, not a hung app: `nettop` samples for a full
/// interval before it prints, and no flag shortens it (measured at
/// 5.06-5.25s across every combination that might have). The caller is
/// therefore required to SAY so while it waits -- a spinner that sits
/// for five seconds without explanation is its own bug -- and to keep
/// this off the five-second health poll entirely, which is why it is a
/// separate command rather than a field on `system_health`.
///
/// See `useNetworkProcesses` for the cadence that follows from that,
/// and `health::netproc` on the Rust side for the measurements.
///
/// Returns an empty list on every platform but macOS: there is no
/// unprivileged per-process attribution on Linux, and Windows' is real
/// unwritten work. The view states the reason rather than drawing an
/// empty table.
export const systemNetworkProcesses = () =>
  call<NetProcess[]>("system_network_processes");
