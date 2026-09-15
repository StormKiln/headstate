import { type QueryClient, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { type View, useFilters } from "../store/filters";
import { listen, type UnlistenFn } from "./transport";
import { safeUnlisten } from "./unlisten";
import { timeCall, timed } from "./diag";
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type {
  AlertReport,
  Artifact,
  Branch,
  BranchDeleteFrame,
  BranchScanFrame,
  ClaudeImported,
  ClaudeOverview,
  ClaudePreview,
  ClaudeUsage,
  ClaudeSubagentRollup,
  ClaudeSessionList,
  ClaudeSessionDetail,
  WireClaudeSessionList,
  Liveness,
  CleanupPrefs,
  DockerImage,
  Footprint,
  HealthSample,
  NetProcess,
  PrDetail,
  PullRequest,
  Upstream,
  Venv,
  Worktree,
  WorktreeScan,
} from "../types/pr";
import type { PrActionName } from "./tauri";
import {
  getCached,
  actOnPrs,
  updatePrBranch,
  statsTree,
  statsBoard,
  statsSeries,
  statsCount,
  statsReviewers,
  // The four unscoped account-wide commands (#826). Their wrappers never
  // left `tauri.ts` -- #829 deleted only the hooks -- so restoring the page
  // is these four imports and the hooks below, not a rebuilt feature.
  getPeriods,
  getHistory,
  getMergedDetail,
  getCycleTrend,
  getPollInterval,
  getRemoteEnabled,
  setRemoteEnabled,
  issuePairingToken,
  listPairedDevices,
  respondToPairing,
  revokePairedDevice,
  type PairingQrPayload,
  type PairingRequest,
  actOnPr,
  getPrDetail,
  getWorktreeDirs,
  classifyRepoUpstream,
  classifyWorktrees,
  listBranches,
  systemHealth,
  healthAlerts,
  systemHealthHistory,
  systemFootprint,
  systemNetworkProcesses,
  type UpdateRunDone,
  listWorktrees,
  removeWorktree,
  pullCheckout,
  fetchRefs,
  updateAllRepositories,
  cancelUpdateAll,
  removeOrphan,
  assessedWorktrees,
  dockerBuilds,
  dockerDanglingVolumes,
  dockerDiskUsage,
  dockerImages,
  dockerPruneCache,
  dockerRemoveImages,
  dockerRemoveVolume,
  dockerState,
  deleteHeadBranch,
  pruneWorktrees,
  removeWorktreeForced,
  unlockWorktree,
  setAutoMerge,
  removeWorktrees,
  removeArtifacts,
  clearAssessed,
  markAssessed,
  checkPackages,
  readClaudeMd,
  scanClaudeMd,
  claudeImportTranscripts,
  claudeOverview,
  claudeSessionUsage,
  claudeSubagentRollup,
  claudeSessions,
  claudeSessionDetail,
  claudeTranscriptTail,
  claudeHooksStatus,
  claudeInstallHooks,
  claudeReinstallHooks,
  claudeUninstallHooks,
  cleanupLog,
  getCleanupPrefs,
  previewCleanup,
  removeVenvs,
  setCleanupPrefs,
  scanVenvs,
  sizeVenvs,
  scanArtifacts,
  sizeArtifacts,
  sizeWorktrees,
  repoTree,
  repoFile,
  getReviewing,
  getCachedReviewing,
  countReviewing,
  getStats,
  cancelUpdateRun,
  refreshNow,
  updateRunState,
  setPollInterval,
  setViewNeedsGithub,
  setWorktreeDirs,
  reviewPr,
  commentOnPr,
  replyToThread,
  resolveThread,
  unresolveThread,
  getViewer,
  rerunChecks,
  getUiPrefs,
  setUiPrefs,
  type UiPrefs,
  getAutostart,
  setAutostart,
  assessWorktree,
  getNotifyPrefs,
  setNotifyPrefs,
  type NotifyPrefs,
  type ReviewVerdictName,
} from "./tauri";

/// The PR list. Seeded from the SQLite snapshot so the first paint shows
/// real content, then reconciled by the Rust poll loop via `prs-updated`.
/// React never talks to GitHub directly.
///
/// `get_cached` returns `[]` both on a genuinely PR-free account and while
/// the first poll (~3s cold) hasn't landed yet. This hook does not attempt
/// to tell those apart -- it falls back to `refresh_now` so the first paint
/// is never a bare empty screen while a poll is in flight. Callers that
/// need "never authenticated" vs. "authenticated, still loading" should
/// consult `get_auth_state` (see `AuthGate`).
export function usePullRequests() {
  const qc = useQueryClient();

  useEffect(() => {
    // Cleanup must not race `listen()`'s own promise: if the effect tears
    // down before `listen` resolves, a naive `un.then(f => f())` calls
    // unlisten on a promise that hasn't produced `f` yet, so the listener
    // registers *after* teardown and leaks. React 19 StrictMode mounts,
    // unmounts, and remounts effects on purpose, so this is not
    // theoretical -- it's the normal dev-mode path.
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;

    listen<PullRequest[]>("prs-updated", (e) => {
      qc.setQueryData(["prs"], e.payload);
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );

    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, [qc]);

  return useQuery({
    queryKey: ["prs"],
    queryFn: PRS_FN,
    staleTime: Infinity,
  });
}

/// `Stats`'s five derived fields always come back zero from the Rust layer
/// today (see `src/types/pr.ts`); only `merged_week`/`merged_month` are
/// real. `refresh_now`-style: does not persist to SQLite, so this is always
/// a live network call, not a cache read.
export function useStats() {
  return useQuery({ queryKey: ["stats"], queryFn: getStats, staleTime: 60_000 });
}

/// The Rust poll loop emits `poll-error` (payload: a display-ready message
/// string) on every failed background poll, and nothing else listens for
/// it. Without this, M2's error handling is invisible: the UI would show
/// stale cached data forever with no indication a poll is failing.
///
/// Deliberately not a TanStack Query cache entry -- there's no `queryFn` to
/// attach it to, it's a push notification from a background loop, not the
/// result of a fetch this component initiated. A minimal module-level store
/// subscribed via `useSyncExternalStore` is the smallest thing that works.
let lastPollError: string | null = null;
const listeners = new Set<() => void>();

function setLastPollError(message: string | null): void {
  lastPollError = message;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function getSnapshot(): string | null {
  return lastPollError;
}

/// Clear the poll-error banner.
///
/// The banner used to clear ONLY on a `prs-updated` event, and
/// `refresh_now` never emits one -- so a successful tray refresh left a red
/// "Background refresh failed" banner over freshly-loaded PRs until the
/// next background tick, up to 300s later.
export function clearPollError(): void {
  setLastPollError(null);
}

/// `src-tauri/src/tray.rs` emits `refresh-requested` when the user clicks
/// "Refresh now" in the tray menu. That click has no other effect on its
/// own -- it only fires the event -- so without a listener the menu item is
/// silently dead: the click succeeds, the event fires, and nothing happens.
///
/// It calls `refreshNow()` directly rather than invalidating the `["prs"]`
/// query -- see the comment on the call itself for why. (This paragraph
/// previously claimed the opposite of what the code does.)
/// Ask GitHub now, and put the answer where the list reads it.
///
/// Shared by the tray's "Refresh now" (through `refresh-requested`) and
/// by the phone's pull-to-refresh gesture, which have identical
/// meaning and had better not drift: both are "the user asked for fresh
/// data", as opposed to the poll loop's own cadence.
///
/// `refreshNow()` directly, NOT `invalidateQueries`. Invalidating would
/// re-run `usePullRequests`'s queryFn, which reads the SQLite snapshot
/// first and only falls back to the network when that snapshot is
/// empty. The poll loop writes a snapshot every tick, so it never is --
/// meaning an invalidate would re-read the same rows the user is
/// already looking at. A manual refresh has to mean "ask GitHub now",
/// or the user waits out the 60s/300s cadence believing they refreshed.
///
/// Rejections go into the poll-error store rather than being thrown:
/// that is the channel the banner already watches, and a manual refresh
/// that failed silently is how the tray menu item used to behave.
async function refreshFromGitHub(qc: QueryClient): Promise<void> {
  try {
    const prs = await refreshNow();
    qc.setQueryData(["prs"], prs);
    // A successful manual refresh is proof the failure is over.
    clearPollError();
  } catch (err: unknown) {
    setLastPollError(
      typeof err === "string" ? err : err instanceof Error ? err.message : "Refresh failed",
    );
  }
}

/// A stable "refresh now" for the phone's pull-to-refresh gesture.
///
/// Stable because `usePullToRefresh` keys its effect on the callback:
/// a new function each render would tear down and re-attach three
/// touch listeners on every render of the whole app shell.
export function useRefreshFromGesture(): () => Promise<void> {
  const qc = useQueryClient();
  return useCallback(() => refreshFromGitHub(qc), [qc]);
}

export function useRefreshRequested(): void {
  const qc = useQueryClient();

  useEffect(() => {
    // Same guarded pattern as the two listeners above -- see their comments
    // for why the `cancelled` flag is load-bearing under StrictMode.
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;

    listen("refresh-requested", () => {
      // The shared implementation: see `refreshFromGitHub` for why this
      // is a direct call rather than an invalidate, and why a failure
      // goes to the poll-error store. Extracted when the phone's
      // pull-to-refresh gained the same meaning (#639); a second copy
      // would have drifted.
      void refreshFromGitHub(qc);
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );

    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, [qc]);
}

/// The most recent `poll-error` message, or `null` if no poll has failed
/// since this window opened (or a later poll has since succeeded and
/// re-emitted `prs-updated`, which clears it).
export function usePollError(): string | null {
  useEffect(() => {
    let unlistenError: UnlistenFn | undefined;
    let unlistenUpdated: UnlistenFn | undefined;
    let cancelled = false;

    listen<string>("poll-error", (e) => {
      setLastPollError(e.payload);
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlistenError = fn;
      },
      () => {},
    );

    // A later successful poll clears the error banner. `prs-updated` is
    // already listened to by `usePullRequests` (which updates the query
    // cache); this listens independently only to clear the error flag, so
    // the two hooks stay decoupled -- a banner can mount without the list
    // being mounted too.
    listen<PullRequest[]>("prs-updated", () => {
      setLastPollError(null);
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlistenUpdated = fn;
      },
      () => {},
    );

    return () => {
      cancelled = true;
      safeUnlisten(unlistenError);
      safeUnlisten(unlistenUpdated);
    };
  }, []);

  return useSyncExternalStore(subscribe, getSnapshot);
}

/// Whether the poll loop is currently fetching.
///
/// Emitted by the Rust loop rather than inferred from `isFetching`: the
/// tray refresh path calls `refreshNow` outside the queryFn, so the query
/// flag never flips for it. The loop that knows is the one that says.
export function usePollState(): "idle" | "fetching" | "retrying" {
  const [state, setState] = useState<"idle" | "fetching" | "retrying">("idle");

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<string>("poll-state", (e) => {
      setState(
        e.payload === "fetching"
          ? "fetching"
          : e.payload === "retrying"
            ? "retrying"
            : "idle",
      );
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, []);

  return state;
}

/// Keep the poll cadence in step with the active view.
///
/// The worktrees view needs no GitHub data, so the loop drops to the
/// background rate while it is open. It does NOT stop: the tray badge
/// would silently go stale, and the badge staying honest while the window
/// is not being watched is the reason polling lives in Rust.
export function useViewCadence(view: string): void {
  useEffect(() => {
    // Tolerate a host without the command, as the other listeners do:
    // cadence is an optimisation, and failing to set it must not break
    // the page.
    void setViewNeedsGithub(view !== "worktrees").catch(() => {});
  }, [view]);
}

/// Refresh the PR list from GITHUB after a write, and seed the cache.
///
/// `invalidateQueries(["prs"])` alone does nothing here, and this is the
/// bug behind "I closed a PR and it stayed in the list until I reloaded".
/// The query's own `queryFn` reads the SQLite snapshot first and only
/// falls through to a live fetch when that snapshot is EMPTY -- so with
/// any PRs at all, the refetch re-reads the very rows the user is looking
/// at. The invalidation was a no-op by construction.
///
/// The `prs-updated` push from the woken poll loop does eventually
/// correct it, but that is a full GitHub round-trip after the click with
/// nothing on screen to say so, and if that one poll fails the list stays
/// wrong for another interval -- up to 2 minutes focused, 10 backgrounded.
///
/// Costs one extra request per write (2 rate-limit points against
/// 5000/hour). It stays NON-OPTIMISTIC: the row disappears because GitHub
/// says it is gone, never because we assumed the write succeeded.
/// Query functions, defined ONCE at module scope.
///
/// `timed()` returns a new function on every call, so wrapping inline
/// in a hook body handed TanStack a different `queryFn` identity each
/// render. Hoisting is correct regardless of whether that ever caused
/// a refetch: a queryFn is configuration, and rebuilding it per render
/// is the kind of thing that bites later even when it is currently
/// harmless.
/// Local-scan timings, for "slow on my machine" reports about the
/// Artifacts, Virtualenvs and Docker views. Hoisted for the same reason
/// as the GitHub ones above.
const SCAN_ARTIFACTS_FN = timed("scan_artifacts", scanArtifacts);
const SCAN_VENVS_FN = timed("scan_venvs", scanVenvs);

const REVIEWING_FN = timed("reviewing", getReviewing);
const CACHED_REVIEWING_FN = timed("reviewing-cached", getCachedReviewing);
const REVIEWING_COUNT_FN = timed("reviewing-count", countReviewing);
const PRS_FN = timed("prs", async () => {
  const cached = await getCached();
  // Show the cache immediately; the poll loop supplies fresh data.
  if (cached.length > 0) return cached;
  return refreshNow();
});

/// The `refreshPrs` currently in flight, so a second caller joins it
/// instead of starting a rival (#742).
///
/// One user gesture routinely causes two of these. Approving a pull
/// request refreshes, and the auto-enqueue that follows refreshes again
/// seconds later -- and `refresh_now` searches every watched repository,
/// which on the account that reported this took 7-17 seconds. The second
/// call would abandon the first mid-flight, which is what surfaced as
/// "Background refresh failed": a real session log held 25 refreshes
/// started with no matching completion, every one of them preceded by a
/// second action within seconds.
///
/// Joining rather than debouncing, deliberately. A debounce would delay
/// the first refresh hoping a second arrives; this starts immediately
/// and lets whoever asks next share the answer. The caller's contract is
/// unchanged either way: await it, and the list is up to date.
let refreshInFlight: Promise<void> | null = null;

async function refreshPrs(qc: QueryClient): Promise<void> {
  // A caller arriving mid-flight gets the SAME promise. Note this is
  // shared across every mutation, which is the point: they all refresh
  // the same `["prs"]` key, so two concurrent fetches of it can only
  // disagree about which answer lands last.
  if (refreshInFlight) return refreshInFlight;

  refreshInFlight = (async () => {
    try {
      qc.setQueryData(["prs"], await refreshNow());
    } catch {
      // The write already succeeded; only the read-back failed. Fall back
      // to the poll loop, which the Rust side has already woken. Throwing
      // here would report a successful action as failed.
      void qc.invalidateQueries({ queryKey: ["prs"] });
    } finally {
      // Cleared before the promise resolves to its awaiters, so the next
      // gesture after this one starts a genuinely fresh fetch rather
      // than joining a settled promise.
      refreshInFlight = null;
    }
  })();

  return refreshInFlight;
}

/// Apply an action to a pull request, then refresh what it affected.
///
/// NOT optimistic. Every list mutation elsewhere updates locally first,
/// but a merge either happened or did not, and showing a PR as merged
/// before GitHub agreed would be a lie about a state the user cannot
/// undo. The Rust side wakes the poll loop on success, so the list
/// catches up within a tick rather than after the full interval.
export function useActOnPr() {
  const qc = useQueryClient();
  return (
    id: string,
    repo: string,
    number: number,
    action: PrActionName,
  ) =>
    actOnPr(id, repo, number, action).then(async () => {
      void qc.invalidateQueries({ queryKey: ["pr-detail", repo, number] });
      void qc.invalidateQueries({ queryKey: ["reviewing"] });
      await refreshPrs(qc);
    });
}

/// Who the token belongs to.
///
/// `staleTime: Infinity` because a login cannot change during a session,
/// so this is one request per launch. Used to decide whether approving is
/// offered at all -- GitHub refuses self-approval, and the UI should say
/// so before the click rather than after a round-trip.
export function useViewer() {
  return useQuery({ queryKey: ["viewer"], queryFn: getViewer, staleTime: Infinity });
}

/// Re-run a pull request's failed CI.
///
/// Invalidates the detail view, where the check list lives, and
/// refreshes the list because the rollup state changes too.
export function useRerunChecks() {
  const qc = useQueryClient();
  return (repo: string, number: number, runId: number) =>
    rerunChecks(repo, number, runId).then(async () => {
      void qc.invalidateQueries({ queryKey: ["pr-detail", repo, number] });
      await refreshPrs(qc);
    });
}

/// The `latestReviews` state each verdict produces.
///
/// `comment` is absent deliberately: a COMMENT review does not change
/// whether the viewer has approved, so seeding one would be inventing a
/// state change that did not happen.
export const REVIEW_STATE: Partial<Record<ReviewVerdictName, string>> = {
  approve: "APPROVED",
  request_changes: "CHANGES_REQUESTED",
};

/// `detail` with the viewer's own review replaced by `state`.
///
/// Pure and non-mutating: React Query compares by reference, and editing
/// the cached object in place would leave components rendering the old
/// value.
export function withOwnReview(detail: PrDetail, viewer: string, state: string): PrDetail {
  const others = detail.latest_reviews.filter((r) => r.author !== viewer);
  return { ...detail, latest_reviews: [...others, { author: viewer, state }] };
}

/// Re-read one pull request's mergeability after a review, without
/// disturbing the verdict this app just seeded.
///
/// A whole-object refetch is what we cannot do here: it would overwrite
/// `latest_reviews` with GitHub's pre-approval set, which lags
/// `addPullRequestReview` by a second or two, and the button would
/// revert to "Approve" for an approval that succeeded. That is the very
/// failure the seeding exists to prevent, so this copies the three
/// merge-related fields across and leaves everything else alone.
///
/// Failure is swallowed on purpose. The review itself already
/// succeeded; a failed follow-up read must not report it as failed. The
/// poll loop and `usePrDetail`'s own refetching still catch up.
async function mergeFieldsAfterReview(
  qc: QueryClient,
  repo: string,
  number: number,
): Promise<void> {
  try {
    const fresh = await getPrDetail(repo, number);
    qc.setQueryData<PrDetail>(["pr-detail", repo, number], (prev) =>
      prev === undefined
        ? prev
        : {
            ...prev,
            merge_status: fresh.merge_status,
            merge_queue_enabled: fresh.merge_queue_enabled,
            in_merge_queue: fresh.in_merge_queue,
          },
    );
  } catch {
    // Deliberately silent; see above.
  }
}

/// Submit a review on a pull request.
///
/// Invalidates `["reviewing"]` and refreshes the PR list: approving a PR
/// removes it from the review queue, and the queue is the surface the
/// user is standing on when they do this.
export function useReviewPr() {
  const qc = useQueryClient();
  return (
    id: string,
    repo: string,
    number: number,
    verdict: ReviewVerdictName,
    body: string,
  ) =>
    reviewPr(id, repo, number, verdict, body).then(async () => {
      void qc.invalidateQueries({ queryKey: ["reviewing"] });
      // Write the verdict we KNOW landed straight into the cache, before
      // asking GitHub anything.
      //
      // `latestReviews` lags `addPullRequestReview`: for a second or two
      // afterwards GitHub still returns the pre-approval review set. The
      // refetch below can land inside that window, and `staleTime` means
      // nothing asks again -- so the button reverted to "Approve" for an
      // approval that had succeeded, which reads as "the click did
      // nothing" and invites a second review.
      //
      // The mutation already verified the outcome (it rejects a PENDING
      // review), so this is not optimism about whether it worked. It is
      // the authoritative answer, applied while GitHub's read side
      // catches up. The refetch that follows overwrites it either way.
      // From the cache rather than a hook argument: `useViewer` has
      // `staleTime: Infinity` and is fetched once at launch, so by the
      // time anyone can click Approve it is populated. Undefined means we
      // genuinely could not ask, and then the seed is skipped rather than
      // attributed to the wrong person.
      const viewer = qc.getQueryData<string>(["viewer"]);
      const state = REVIEW_STATE[verdict];
      if (state !== undefined && viewer !== undefined) {
        qc.setQueryData<PrDetail>(["pr-detail", repo, number], (prev) =>
          prev === undefined ? prev : withOwnReview(prev, viewer, state),
        );
      }
      // The REVIEW is not refetched here, and that is deliberate.
      //
      // The seed above writes the verdict we know landed. Immediately
      // awaiting a refetch of THE SAME KEY replaced it with GitHub's
      // pre-approval review set -- inside the exact lag window the seed
      // exists to cover. So the button reverted to "Approve" for an
      // approval that had succeeded, and only corrected itself when
      // something else refetched later: clicking Merge, or leaving the
      // PR and coming back.
      //
      // The seed is authoritative rather than optimistic (the mutation
      // rejects a PENDING review), so there is nothing to confirm.
      //
      // But MERGEABILITY is a different field with the opposite
      // problem, and leaving it alone was a bug (#699). An approval is
      // exactly what makes a pull request mergeable, or auto-merge
      // enqueue it -- so `merge_status` and `in_merge_queue` are stale
      // the instant the review lands, and nothing was re-reading them:
      //
      // - a queue-enabled PR still showed "Add to merge queue" for a
      //   pull request GitHub had already queued, and clicking it got
      //   an "already in the merge queue" error;
      // - a PR that became mergeable kept a disabled Merge button, so
      //   the user had to finish the job on github.com.
      //
      // `usePrDetail` polls while `merge_status` is `unknown`, but that
      // could never fix this: the stale value is not `unknown`, it is
      // the PRE-approval verdict (`blocked`), which looks like a
      // settled answer. Something has to ask again once, and the poller
      // takes over from there if GitHub is still recomputing.
      //
      // `mergeFieldsAfterReview` re-reads the one pull request and
      // copies ONLY those fields over, so the seeded verdict survives.
      // Awaited: the detail view the user is looking at, one request.
      await mergeFieldsAfterReview(qc, repo, number);
      // NOT awaited: the whole-world list refresh.
      //
      // `refreshPrs` calls `refresh_now`, which searches EVERY watched
      // repository. Awaiting it meant approving one pull request took
      // ~20s to register in the UI while the write itself had landed on
      // GitHub instantly -- the click looked ignored, and re-clicking
      // was the natural response.
      //
      // The list still updates: this runs, and the Rust side wakes the
      // poll loop on success besides. It just no longer stands between
      // the user and the button they pressed.
      void refreshPrs(qc);
    });
}

/// Comment on a pull request.
///
/// Only the detail view changes: a comment does not alter any state the
/// list renders, so refreshing the whole list would be a wasted request.
export function useCommentOnPr() {
  const qc = useQueryClient();
  return (id: string, repo: string, number: number, body: string) =>
    commentOnPr(id, repo, number, body).then(() => {
      void qc.invalidateQueries({ queryKey: ["pr-detail", repo, number] });
    });
}

/// Resolve, reopen, and reply on a review conversation.
///
/// All three invalidate the detail query, because all three change the
/// unresolved COUNT the header renders beside the thread list. Leaving it
/// stale would put "2 unresolved conversations" above a list showing one
/// -- the header contradicting the section beneath it.
///
/// The list queries are deliberately untouched: `unresolved_threads` is
/// on the list model too, but a poll refreshes it within the minute and
/// invalidating a 6-second query for a number nobody is looking at is the
/// cost #328 measured and rejected.
///
/// AWAITED, not `void`-ed. A fire-and-forget invalidation settles the
/// promise immediately, so the button cleared its busy state before the
/// data it depends on existed -- the same defect as #377, which these
/// three inherited by being written one commit before that fix. The
/// backend now verifies each of these mutations landed, so refetching
/// here is confirming a known-good result rather than hoping.
export function useResolveThread() {
  const qc = useQueryClient();
  return (threadId: string, repo: string, number: number) =>
    resolveThread(threadId, repo, number).then(async () => {
      await qc.refetchQueries({ queryKey: ["pr-detail", repo, number] });
    });
}

export function useUnresolveThread() {
  const qc = useQueryClient();
  return (threadId: string, repo: string, number: number) =>
    unresolveThread(threadId, repo, number).then(async () => {
      await qc.refetchQueries({ queryKey: ["pr-detail", repo, number] });
    });
}

export function useReplyToThread() {
  const qc = useQueryClient();
  return (threadId: string, repo: string, number: number, body: string) =>
    replyToThread(threadId, repo, number, body).then(async () => {
      await qc.refetchQueries({ queryKey: ["pr-detail", repo, number] });
    });
}

/// Regenerable build output under the configured scan roots.
///
/// Discovery only. Sizes come from `useArtifactSizes`, because the two
/// passes differ by three orders of magnitude and blocking the list on
/// the slow one would leave the view empty for a minute -- the exact
/// complaint that shaped the worktree page.
export function useArtifacts(enabled: boolean) {
  return useQuery({
    queryKey: ["artifacts"],
    queryFn: SCAN_ARTIFACTS_FN,
    enabled,
    // The set changes when someone builds or clones, not on a timer.
    staleTime: 60 * 1000,
    // `retry: false`, against the default `retry: 3` (#846).
    //
    // This is a filesystem walk, not a network call. The default turned
    // one rejection into FOUR sequential walks of the whole code tree --
    // measured at ~1.5s for 178 directories on a 221 GB tree, longer on
    // a cold cache -- and then `ArtifactsPage` reported "No build output
    // found in the scanned directories": a clean machine, after silently
    // trying three times to look at a dirty one. `useWorktreeSizes`
    // records the same reasoning for the same class of query.
    //
    // Acceptable only because the page now has an explicit retry, which
    // is the rule `useStatsBoard` states.
    retry: false,
  });
}

/// Sizes for artifact directories, measured in per-repository batches.
///
/// Batched by repo rather than one query for everything, so the page
/// fills in progressively instead of staying blank until the slowest
/// directory finishes. A 61 GB `target/` can take tens of seconds on its
/// own; the other 177 rows should not wait behind it.
///
/// `staleTime` is long for the same reason as the worktree equivalent: a
/// directory's size does not change unless its contents do.
export function useArtifactSizes(artifacts: Artifact[], enabled: boolean) {
  // Grouped OUTSIDE the query so the key set is stable across renders --
  // a fresh grouping each render would remount every query and restart
  // the measurement.
  const byRepo = new Map<string, string[]>();
  for (const a of artifacts) {
    const list = byRepo.get(a.repo_path) ?? [];
    list.push(a.path);
    byRepo.set(a.repo_path, list);
  }
  const groups = [...byRepo.entries()].sort(([a], [b]) => a.localeCompare(b));

  const results = useQueries({
    queries: groups.map(([repo, paths], i) => ({
      // Keyed on the repo alone, NOT on `paths.length`.
      //
      // With the count in the key, removing one directory changed every
      // surviving group's key too -- so the cache had nothing for the
      // new keys and all of them refetched at once. That is the storm
      // this is fixing; dropping removed entries from the cache
      // achieves nothing if the key they were cached under no longer
      // exists.
      //
      // The query function still closes over the current `paths`, so a
      // group whose membership changed re-measures on its next natural
      // fetch rather than never.
      queryKey: ["artifact-sizes", repo],
      queryFn: () =>
        // The GROUP INDEX in the log label, never the path. This log is
        // meant to be sent to someone, and Settings promises it carries
        // "counts and timings only -- never repository names".
        timeCall(`size_artifacts[#${i}] n=${paths.length}`, () => sizeArtifacts(paths)),
      enabled,
      staleTime: 5 * 60 * 1000,
    })),
  });

  const sizes = new Map<string, number>();
  const ages = new Map<string, number>();
  for (const r of results) {
    for (const [path, bytes, age] of r.data ?? []) {
      sizes.set(path, bytes);
      if (age !== null) ages.set(path, age);
    }
  }
  return {
    sizes,
    ages,
    /// How many repositories have not answered yet -- the number that
    /// makes a partially-filled page legible rather than broken.
    pending: results.filter((r) => r.isFetching).length,
    total: results.length,
    /// How many batches FAILED outright (#956).
    ///
    /// Counted separately from `pending`, because a failed batch leaves
    /// `pending` and never comes back: a caller that only watches
    /// `pending` sees the number fall to zero and concludes everything was
    /// measured. #769 is the shape of that mistake -- silence read as
    /// success. Copied from `useAllWorktreeSizes`, whose comment has said
    /// exactly this for longer than this hook has been wrong.
    ///
    /// `size_artifacts` genuinely rejects: it `map_err`s a semaphore
    /// acquire and then runs inside `spawn_blocking`, whose join can fail.
    /// With `retry: false` reasoning nearby, one rejection is final.
    failed: results.filter((r) => r.isError).length,
  };
}

/// Remove artifact directories.
///
/// Invalidates the scan AND the sizes: a removed directory must leave
/// the list, and a stale size row would otherwise keep counting bytes
/// that are no longer there.
export function useRemoveArtifacts() {
  const qc = useQueryClient();
  return async (paths: string[]) => {
    const out = await removeArtifacts(paths);
    // The SCAN is invalidated: a removed directory must leave the list.
    await qc.invalidateQueries({ queryKey: ["artifacts"] });
    // The SIZES are not.
    //
    // Invalidating them re-walked every group on the machine. Measured
    // on a real removal: 54 concurrent `size_artifacts` calls all
    // finishing around 17.8s, with groups of TWO directories taking
    // 17.6s -- contention, not measurement. The 20.4s "freeze" after a
    // deletion was this, not `remove_dir_all` and not rendering.
    //
    // A removal is the one operation whose effect on other rows is
    // known to be nil: the removed directories are gone and the rest
    // are untouched. So the removed entries are dropped from the cached
    // results and everything else stands. `useRemoveWorktrees` already
    // does exactly this.
    const gone = new Set(out.filter((o) => o.error === null).map((o) => o.path));
    if (gone.size > 0) {
      qc.setQueriesData<[string, number, number | null][]>(
        { queryKey: ["artifact-sizes"] },
        (old) => old?.filter(([path]) => !gone.has(path)),
      );
    }
    return out;
  };
}

/// Poetry virtualenvs, classified.
///
/// Discovery only. Sizes and idle times come from `useVenvSizes`,
/// because deciding staleness needs a full walk of each venv.
export function useVenvs(enabled: boolean) {
  return useQuery({
    queryKey: ["venvs"],
    queryFn: SCAN_VENVS_FN,
    enabled,
    // 30 MINUTES, not 60 seconds.
    //
    // A one-minute staleTime on a scan that takes 9-40 SECONDS means
    // the page spends most of its life re-measuring. Measured on a real
    // machine: rescans at 18:28, 18:31, 18:34, 18:37 -- each 9-40s of
    // scan followed by up to 73s of sizing, so the list never settled
    // and sizes never appeared to populate.
    //
    // Virtualenvs do not appear and disappear on a one-minute cadence.
    // The manual refresh and the post-removal invalidation are what
    // update this promptly; the timer only needs to catch drift.
    staleTime: 30 * 60 * 1000,
    // A background refetch mid-session restarts the whole cycle for no
    // benefit -- the data is minutes old at worst.
    refetchOnWindowFocus: false,
    // `retry: false`, and the pairing with an explicit retry is what
    // makes that acceptable (#846).
    //
    // The two settings above are correct for DATA and were a trap for a
    // FAILURE: `staleTime: 30 * 60 * 1000` with `refetchOnWindowFocus:
    // false` meant a rejected scan sat there for half an hour with
    // nothing re-running it and nothing offering to. `VenvSection` read
    // only `isLoading`, so the whole section -- its orphan count, its
    // bulk-remove button -- silently ceased to exist for thirty minutes
    // on a machine that might hold 78 removable virtualenvs.
    //
    // The default `retry: 3` made it worse, not better: three silent
    // re-runs of a scan measured at 26 SECONDS (walking 28,144
    // directories) is over a minute of looking broken before the section
    // vanishes. `hooks.ts` states the governing rule on `useStatsBoard`
    // -- `retry: false` is acceptable "because the view has an explicit
    // retry that tells the user it is trying again" -- and the section
    // now renders `QueryError` with exactly that.
    retry: false,
  });
}

/// How many virtualenvs one sizing request measures.
///
/// Small enough that a stalled entry holds up few others, large enough
/// that the IPC round trips do not dominate. Four matches the backend's
/// concurrency cap, so a chunk maps to one permit rather than queueing
/// against itself.
const VENV_CHUNK = 4;

/// Sizes and idle times for virtualenvs.
///
/// One batch rather than per-project groups: unlike artifacts, venvs are
/// individually small (under a gigabyte each on a real cache), so no
/// single one holds up the rest and the added query keys would only
/// fragment the cache.
export function useVenvSizes(venvs: Venv[], enabled: boolean) {
  const paths = venvs.map((v) => v.path);

  // CHUNKED, so results land progressively.
  //
  // This was one query over every virtualenv, which meant one silent
  // wait: measured on a real machine at up to 73 seconds with a single
  // boolean `measuring` and nothing on screen changing. Worse, one
  // unreadable path -- a network mount, a permission wall -- stalled
  // every other row behind it.
  //
  // Chunks rather than one query per venv: 13 separate IPC round trips
  // for 13 directories is its own cost, and the backend already logs
  // per-venv timings for the "which one is slow" question. Four is
  // small enough that a stalled chunk holds up at most three others.
  const chunks: string[][] = [];
  for (let i = 0; i < paths.length; i += VENV_CHUNK) {
    chunks.push(paths.slice(i, i + VENV_CHUNK));
  }

  const results = useQueries({
    queries: chunks.map((chunk, i) => ({
      // Keyed on the chunk's OWN paths, not its index. An index would
      // reuse a cache entry for a different set after a removal --
      // the same trap `artifact-sizes` hit with `paths.length`.
      queryKey: ["venv-sizes", ...chunk],
      queryFn: () => timeCall(`size_venvs[#${i}] n=${chunk.length}`, () => sizeVenvs(chunk)),
      enabled: enabled && chunk.length > 0,
      // Matched to the scan. Sizing walks every virtualenv, and a short
      // window meant it re-ran almost as often as it finished.
      staleTime: 30 * 60 * 1000,
      refetchOnWindowFocus: false,
    })),
  });

  const sizes = new Map<string, number>();
  const idle = new Map<string, number>();
  for (const r of results) {
    for (const [path, bytes, secs] of r.data ?? []) {
      sizes.set(path, bytes);
      if (secs !== null) idle.set(path, secs);
    }
  }
  return {
    sizes,
    idle,
    measuring: results.some((r) => r.isFetching),
    /// How many chunks have not answered yet, and how many there are.
    /// The pair is what makes a partially-filled page legible rather
    /// than looking stuck -- the same thing `useArtifactSizes` reports.
    pending: results.filter((r) => r.isFetching).length,
    total: results.length,
    /// How many chunks FAILED outright (#956).
    ///
    /// The same field, for the same reason, as `useArtifactSizes` and
    /// `useAllWorktreeSizes`: a rejected chunk is neither fetching nor
    /// holding data, so it contributes nothing to `pending` and `pending`
    /// reaches 0 with measurements missing. A caller reading `pending ===
    /// 0` as "fully measured" then puts an understated byte count on a
    /// Remove button.
    failed: results.filter((r) => r.isError).length,
  };
}

/// Remove virtualenvs, then refresh what is left.
export function useRemoveVenvs() {
  const qc = useQueryClient();
  return async (paths: string[]) => {
    const out = await removeVenvs(paths);
    await qc.invalidateQueries({ queryKey: ["venvs"] });
    await qc.invalidateQueries({ queryKey: ["venv-sizes"] });
    return out;
  };
}

/// Automatic cleanup preferences.
export function useCleanupPrefs() {
  const qc = useQueryClient();
  const { data } = useQuery({
    queryKey: ["cleanup-prefs"],
    queryFn: getCleanupPrefs,
    staleTime: Infinity,
  });
  return {
    prefs: data,
    set: async (prefs: CleanupPrefs) => {
      await setCleanupPrefs(prefs);
      await qc.invalidateQueries({ queryKey: ["cleanup-prefs"] });
    },
  };
}

/// The cleanup ledger, and a way to run a pass now.
///
/// Running it on demand is the whole of Phase 1's value: the user turns
/// the feature on, clicks once, and reads what it WOULD have removed on
/// their own machine -- rather than being asked to trust a rule they
/// have never seen applied.
export function useCleanupLog(enabled: boolean) {
  const qc = useQueryClient();
  const { data = [], isLoading, isError, refetch } = useQuery({
    queryKey: ["cleanup-log"],
    queryFn: cleanupLog,
    enabled,
    staleTime: 30_000,
    // Paired with the explicit retry the component now renders, the way
    // #846 paired them on `useArtifacts`, `useVenvs` and `useClaudeMd`:
    // a silent background retry makes a failure look like a slow load.
    retry: false,
  });
  return {
    entries: data,
    isLoading,
    // `isError` and `refetch` are RETURNED since #854. Without them the
    // component could not comply however it was written -- the hook is
    // the whole reason this surface was not fixable by #846, which
    // changed four components and no hooks' return shapes. An audit
    // ledger that reads "No reports yet" because the read FAILED is the
    // sharpest form of this bug: the component's own purpose is turning
    // "trust this predicate" into "I have read this list".
    isError,
    refetch,
    run: async () => {
      const out = await previewCleanup();
      await qc.invalidateQueries({ queryKey: ["cleanup-log"] });
      return out;
    },
  };
}

/// Outdated packages for one repository.
///
/// `enabled` gates it on a repo actually being selected, and it never
/// runs on a timer: these commands hit registries and take seconds.
export function usePackages(repoPath: string | undefined) {
  return useQuery({
    queryKey: ["packages", repoPath],
    queryFn: () => checkPackages(repoPath as string),
    enabled: Boolean(repoPath),
    // Long, because the answer changes when a registry publishes, not
    // when the user clicks around. Refetching costs seconds and network.
    staleTime: 10 * 60 * 1000,
  });
}

/// CLAUDE.md files in one repository, with import trees resolved.
///
/// `retry: false`, paired with the page's explicit retry (#846). A
/// rejected scan used to leave `data` at its `[]` default and the page
/// then said "No CLAUDE.md files in this repository" -- a confident,
/// wrong answer to a question it could not answer, which is the exact
/// defect `QueryError` was written to diagnose.
export function useClaudeMd(repoPath: string | undefined) {
  return useQuery({
    queryKey: ["claude-md", repoPath],
    queryFn: () => scanClaudeMd(repoPath as string),
    enabled: Boolean(repoPath),
    staleTime: 30_000,
    retry: false,
  });
}

/// The text of one file.
///
/// Fetched separately from the scan: holding every file's contents to
/// display one is a lot of bytes across the bridge for nothing.
///
/// `retry: false` and an explicit retry in the pane (#846). A file
/// renamed or deleted between the scan and the click -- a real
/// possibility at `staleTime: 30_000` -- is a settled refusal, not a
/// flaky call, so three silent re-reads only delay saying so. The pane
/// used to render NOTHING on a rejection: `textLoading` false, `text`
/// undefined, both arms of its chain failing and the chain ending at
/// `null`.
export function useClaudeMdText(path: string | undefined) {
  return useQuery({
    queryKey: ["claude-md-text", path],
    queryFn: () => readClaudeMd(path as string),
    enabled: Boolean(path),
    staleTime: 30_000,
    retry: false,
  });
}

/// One directory level of a repository, from the git index (#1031).
///
/// Keyed on BOTH the repository and the path, so descending is a cache
/// miss for the new level rather than a refetch of the old one, and two
/// repositories that both have a `src/` never share an entry.
///
/// `retry: false`, and `useClaudeMdText`'s reason applies unchanged: a
/// directory removed between the scan and the click is a real possibility
/// -- ~100 sibling agent worktrees are created and destroyed continuously
/// on this machine -- and a settled refusal is not a flaky call, so three
/// silent re-reads only delay saying so. The page offers an explicit
/// retry instead, which is honest about being a second attempt (#1036).
///
/// A rejection is NOT an empty listing. `data` is left `undefined` on a
/// failure rather than defaulted to a tree with no entries, which is
/// #846 exactly: the four outcomes the page renders are distinguishable
/// only if this hook keeps them distinguishable.
export function useRepoTree(repoPath: string | undefined, path: string) {
  return useQuery({
    queryKey: ["repo-tree", repoPath, path],
    queryFn: () => repoTree(repoPath as string, path),
    enabled: Boolean(repoPath),
    staleTime: 30_000,
    retry: false,
  });
}

/// One file's bounded contents (#1033).
///
/// Fetched only when a file is selected, for `useClaudeMdText`'s reason:
/// holding every file's contents to display one is a lot of bytes across
/// the bridge for nothing -- and here the bridge may be the pairing
/// transport to a phone.
///
/// `retry: false` for the same reason as the listing above, and one more
/// of its own that is measured: the git index and the filesystem already
/// disagree today. Two tracked files in that corpus do not exist on disk,
/// both indexed symlinks whose targets are gone. A file in the listing
/// that cannot be read is a settled answer about that file, not a
/// transient one -- and it does NOT invalidate the listing, which is the
/// `PartialScanNotice` trade: one unreadable entry is not evidence the
/// other 650 are wrong.
export function useRepoFile(repoPath: string | undefined, path: string | undefined) {
  return useQuery({
    queryKey: ["repo-file", repoPath, path],
    queryFn: () => repoFile(repoPath as string, path as string),
    enabled: Boolean(repoPath) && Boolean(path),
    staleTime: 30_000,
    retry: false,
  });
}

/// How often the Claude Code view re-derives liveness.
///
/// Liveness is not stored (migration 11 has no `status` column), so the
/// ONLY way a row stops saying "running" is this poll. 10 seconds is the
/// same order as `useSystemHealth`'s and for the same reason: it is a
/// question about the machine right now, and the cost is one directory
/// read of a handful of files plus a scoped `sysinfo` refresh -- three
/// pids against 1,438 sessions on the real machine.
///
/// It also supplies `now` to the page without a clock read during render
/// (see `useClaudeSessions`).
const CLAUDE_POLL_MS = 10_000;

/// Every Claude Code session, with liveness derived on each poll (#917).
///
/// # Why it imports first, once
///
/// A fresh database has no `claude_session` rows at all, so the view
/// would open empty on a machine with 1,438 real sessions on disk. The
/// import is a full rescan measured at 1.4s for the real corpus and is
/// idempotent by construction (it upserts on `session_id`), so running
/// it once when this view first mounts is simpler and more correct than
/// any incremental scheme -- #914's own reasoning.
///
/// It is a SEPARATE query rather than part of the list's `queryFn` so
/// that its failure is separable: a rescan that could not read
/// `~/.claude/projects` must not take the stored list down with it, and
/// the list's own 10-second poll must not re-scan 1,438 transcripts
/// every tick. The import runs once per mount; the list polls.
///
/// `retry: false` on both: a permission error on `~/.claude` is a
/// settled refusal, not a flaky call, and three silent re-reads only
/// delay saying so (the rule #846 applied to `useClaudeMd`).
///
/// # `dataUpdatedAt` is the page's `now`
///
/// Returned so the page can pass it down rather than reading
/// `Date.now()` during render -- the purity rule `Sparkline` and
/// `HealthConditions` both state, and which `yarn lint` enforces. It
/// advances once per poll, which is also the honest edge for "how old is
/// this reading": the last moment we actually heard from the machine.
/// Resolve each row's interned liveness reason back into its sentence
/// (#985).
///
/// The inverse of the backend's `Reasons` table, and the ONE place the
/// wire shape is known: every consumer below this sees `Liveness` with
/// its `why`, unchanged from before the split.
///
/// # An index with no entry is `unknown`, and says so
///
/// It should be impossible -- the backend builds the table and the
/// indices in one pass -- so this arm is about what happens if it ever is
/// not. The alternatives were worse: `?? ""` would put an empty tooltip
/// under a confident "Not running", and `?? "not running"` would invent
/// grounds the app does not have. `unknown` with a reason naming the
/// failure is the honest reading, and it is the state the app already
/// renders as "Could not tell" (#846, #841 -- `unknown` must never be
/// shown as a shade of `dead`, because "not running" is what offers
/// Resume).
///
/// Exported for its test.
export function hydrateClaudeSessions(wire: WireClaudeSessionList): ClaudeSessionList {
  const why = (ix: number): string | null => wire.reasons[ix] ?? null;
  return {
    sessions: wire.sessions.map((s) => {
      let liveness: Liveness;
      if (s.liveness.state === "running") {
        liveness = { state: "running", pid: s.liveness.pid, status: s.liveness.status };
      } else {
        const reason = why(s.liveness.why);
        liveness =
          reason === null
            ? {
                state: "unknown",
                why: `the reason for this session's state did not arrive (index ${s.liveness.why} of ${wire.reasons.length})`,
              }
            : { state: s.liveness.state, why: reason };
      }
      return {
        session_id: s.session_id,
        name: s.name,
        cwd: s.cwd,
        git_branch: s.git_branch,
        last_activity_at: s.last_activity_at,
        liveness,
        cwd_state: s.cwd_state,
        kind: s.kind,
        subagents: s.subagents,
      };
    }),
    registry_failure: wire.registry_failure,
    registry_unreadable: wire.registry_unreadable,
  };
}

/// What one SELECTED session knows that the list does not carry (#985).
///
/// # Keyed by id, and polled with the list
///
/// `CLAUDE_POLL_MS`, the same tick as the list, because the detail pane
/// shows a LIVENESS -- and liveness is derived per read, so a detail that
/// did not poll would keep saying "running" under a list that had already
/// stopped. That is the staleness the split had to avoid, moved from the
/// rows to the pane.
///
/// It is one row, so this costs about what one row of the old list cost:
/// the saving is that the other 1,473 no longer come with it.
///
/// `retry: false`, this feature's rule: a database that could not be read
/// is a settled refusal, and three silent re-reads only delay saying so.
///
/// A resolved `null` means the store has no such id -- a session deleted
/// between two polls -- which the caller renders differently from a
/// rejection (#846).
export function useClaudeSessionDetail(sessionId: string | null, enabled: boolean) {
  return useQuery<ClaudeSessionDetail | null>({
    queryKey: ["claude-session-detail", sessionId],
    queryFn: () => claudeSessionDetail(sessionId as string),
    enabled: enabled && sessionId !== null && sessionId !== "",
    refetchInterval: enabled && sessionId ? CLAUDE_POLL_MS : false,
    staleTime: CLAUDE_POLL_MS - 1_000,
    retry: false,
  });
}

export function useClaudeSessions(enabled: boolean) {
  const list = useQuery<ClaudeSessionList>({
    queryKey: ["claude-sessions"],
    // Hydrated HERE, in the `queryFn`, so the interning is a transport
    // detail with one boundary. Everything downstream -- the search, the
    // chips, the badges, the tests -- sees `Liveness` with its sentence,
    // exactly as it did before #985.
    queryFn: async () => hydrateClaudeSessions(await claudeSessions()),
    enabled,
    refetchInterval: enabled ? CLAUDE_POLL_MS : false,
    staleTime: CLAUDE_POLL_MS - 1_000,
    retry: false,
  });

  // The one-shot import. `staleTime: Infinity` so it does not re-run on
  // a remount within the session; the "Rescan" button invalidates it
  // explicitly when the user wants a fresh read of disk.
  const imported = useQuery<ClaudeImported>({
    queryKey: ["claude-import"],
    queryFn: claudeImportTranscripts,
    enabled,
    staleTime: Infinity,
    retry: false,
  });

  const qc = useQueryClient();
  return {
    list,
    imported,
    /// `now`, resolved once per poll rather than per render.
    now: list.dataUpdatedAt,
    /// Re-read `~/.claude/projects` and then the list. Both, in that
    /// order: a rescan that did not refresh the list would leave the
    /// user looking at the rows from before it.
    rescan: async () => {
      await qc.invalidateQueries({ queryKey: ["claude-import"] });
      await qc.invalidateQueries({ queryKey: ["claude-sessions"] });
    },
  };
}


/// How much work happened inside one session (#959).
///
/// # Not polled, and keyed by PATH
///
/// A session's usage never changes once the session is dead, which is
/// 1,295 of 1,474 rows on the real corpus. A 10-second poll of a figure
/// that cannot move would be the one expensive read on this page repeated
/// forever -- the rollup is 11x the cost of the startup scan's head+tail.
/// So `staleTime: Infinity`: it is read when a session is selected and
/// cached under that transcript's path.
///
/// The PATH is the key rather than the session id, because the path is
/// what the command takes and because a transcript that moved is a
/// different file with a different answer.
///
/// `retry: false`, the rule this whole feature follows: a permission
/// error on `~/.claude` is a settled refusal, not a flaky call, and three
/// silent re-reads only delay saying so (#846).
///
/// # No `= {}` default
///
/// A rejected read must reach the caller's error arm. Four zeroes for a
/// transcript that could not be read is a confident wrong answer with a
/// credible shape, which is precisely what this feature is filed against.
export function useClaudeSessionUsage(path: string | null) {
  return useQuery<ClaudeUsage>({
    queryKey: ["claude-usage", path],
    queryFn: () => claudeSessionUsage(path as string),
    // The path is required, so the query simply does not run without one
    // -- a session with no transcript is a real row and must not produce
    // a rejected query that reads as a failure.
    enabled: path !== null && path !== "",
    staleTime: Infinity,
    retry: false,
  });
}

/// What one session's subagents cost, as a figure of its own (#1002).
///
/// # Keyed by session id, and not polled
///
/// The session id rather than a path, because the rollup is over the
/// CHILDREN's transcripts and there is no single path to key on. Keyed by
/// the parent whose figure it is.
///
/// `staleTime: Infinity` for `useClaudeSessionUsage`'s reason, doubled: a
/// dead session's usage cannot move, and this reads one bounded
/// transcript PER CHILD -- the measured corpus has parents with dozens.
/// Polling it every ten seconds would be the whole-corpus read the
/// bounded summariser exists to avoid, once per tick.
///
/// `enabled` on the caller's own test for whether there is anything to
/// roll up: a session with no attributed subagents must not produce a
/// query at all, rather than one that resolves to zeros.
///
/// `retry: false` and no `= {}` default, the rule this feature follows
/// throughout: a rejected read must reach the caller's error arm, because
/// zeros for a rollup that could not be read is a confident wrong answer
/// with a credible shape.
export function useClaudeSubagentRollup(sessionId: string | null) {
  return useQuery<ClaudeSubagentRollup>({
    queryKey: ["claude-subagent-rollup", sessionId],
    queryFn: () => claudeSubagentRollup(sessionId as string),
    enabled: sessionId !== null && sessionId !== "",
    staleTime: Infinity,
    retry: false,
  });
}

/// The tail of one session's transcript, as conversation (#982).
///
/// Keyed by path and `staleTime: Infinity` for the same reasons
/// `useClaudeSessionUsage` above is: a dead session's transcript does not
/// change, and this is a 256 KB read that crosses the pairing transport
/// on the phone.
///
/// `enabled` so the read is not merely cached but never STARTED until the
/// user asks: the preview is behind a disclosure on the detail pane, so a
/// user scrolling the list does not pull 256 KB per selection.
export function useClaudeTranscriptTail(path: string | null, enabled: boolean) {
  return useQuery<ClaudePreview>({
    queryKey: ["claude-transcript-tail", path],
    queryFn: () => claudeTranscriptTail(path as string),
    enabled: enabled && path !== null && path !== "",
    staleTime: Infinity,
    retry: false,
  });
}

/// How often the Claude Code overview re-reads.
///
/// 10 seconds, the same order as `useSystemHealth`, and for the same
/// reason: the running count is derived per read (nothing is stored, per
/// migration 11's deliberate lack of a `status` column), so this poll is
/// the ONLY thing that makes a session stop saying "running". The cost is
/// two SELECTs and a stat per session -- measured at 7ms for 1,461
/// sessions -- which makes a 10-second tick trivially affordable.
///
/// It also supplies `now` without a clock read during render; see below.
const CLAUDE_OVERVIEW_POLL_MS = 10_000;

/// The Claude Code overview's aggregates (#921).
///
/// # `retry: false`, and the page retries explicitly
///
/// A permission error on `~/.claude` is a settled refusal, not a flaky
/// call, so three silent re-reads only delay saying so. That is the rule
/// #846 applied to `useClaudeMd` one view over, and the stakes are higher
/// here: this page's failure mode is a CHART, and a chart of zeros reads
/// as a measured quiet month rather than as an absent reading.
///
/// There is deliberately no `= {}` default anywhere in the chain. The page
/// switches on `isError` BEFORE it switches on emptiness, so a rejected
/// query renders the reason and never "no sessions".
///
/// # `dataUpdatedAt` is the page's `now`
///
/// Returned so the page can pass it down rather than reading `Date.now()`
/// during render -- the purity rule `Sparkline` and `HealthConditions`
/// both state, and which `yarn lint` enforces. It advances once per poll,
/// which is also the honest edge for "how old is this reading": the last
/// moment we actually heard from the machine.
///
/// # The rescan is separate, and manual
///
/// `claude_overview` READS the cache; it never populates it. So on a
/// machine whose cache is empty the page says so and offers Rescan, rather
/// than silently running a 1,461-transcript disk walk on every mount. Two
/// reasons it is not folded into the poll: a rescan that failed must not
/// take the stored aggregates down with it, and a 10-second poll must not
/// re-read 881 MB of transcript tails every tick.
///
/// #917's session list owns the one-shot import at mount. Once both ship,
/// opening either page fills the cache for both -- they share
/// `["claude-import"]`, so the query key is the coordination and neither
/// page needs to know about the other.
export function useClaudeOverview(enabled: boolean) {
  const query = useQuery<ClaudeOverview>({
    queryKey: ["claude-overview"],
    queryFn: claudeOverview,
    enabled,
    refetchInterval: enabled ? CLAUDE_OVERVIEW_POLL_MS : false,
    staleTime: CLAUDE_OVERVIEW_POLL_MS - 1_000,
    retry: false,
  });

  const qc = useQueryClient();
  return {
    query,
    /// Resolved once per poll rather than per render.
    now: query.dataUpdatedAt,
    /// Re-read `~/.claude/projects`, then the aggregates. In that order:
    /// refreshing the aggregates first would show the user the figures
    /// from before their rescan.
    ///
    /// The error is thrown rather than swallowed, so the caller can say
    /// which half failed. A rescan that could not read the transcript
    /// directory and then silently refreshed unchanged aggregates is the
    /// button that looks like it worked.
    rescan: async () => {
      await claudeImportTranscripts();
      await qc.invalidateQueries({ queryKey: ["claude-import"] });
      await qc.invalidateQueries({ queryKey: ["claude-overview"] });
    },
  };
}

/// Merge the base branch into a pull request's head.
///
/// Invalidates the same keys as `useActOnPr`: the update changes CI
/// state and mergeability, so a row left showing "behind" after a
/// successful update would be stale in exactly the way the button was
/// meant to fix.
export function useUpdatePrBranch() {
  const qc = useQueryClient();
  return (id: string, repo: string, number: number, expectedHead: string) =>
    updatePrBranch(id, repo, number, expectedHead).then(async () => {
      void qc.invalidateQueries({ queryKey: ["pr-detail", repo, number] });
      void qc.invalidateQueries({ queryKey: ["reviewing"] });
      await refreshPrs(qc);
    });
}

/// Apply one action to several pull requests.
///
/// Invalidates once after the whole batch rather than per pull request:
/// forty mutations would otherwise trigger forty refetches of the same
/// list. Resolves with per-PR outcomes; it rejects only if the batch
/// itself could not run.
export function useActOnPrs() {
  const qc = useQueryClient();
  return (prs: [string, string, number][], action: PrActionName) =>
    actOnPrs(prs, action).then(async (outcomes) => {
      void qc.invalidateQueries({ queryKey: ["reviewing"] });
      await refreshPrs(qc);
      return outcomes;
    });
}

/// Enable or cancel "merge when green".
export function useSetAutoMerge() {
  const qc = useQueryClient();
  return (id: string, repo: string, number: number, expectedHead: string, enable: boolean) =>
    setAutoMerge(id, repo, number, expectedHead, enable).then(async () => {
      void qc.invalidateQueries({ queryKey: ["pr-detail", repo, number] });
      await refreshPrs(qc);
    });
}

/// Delete a merged pull request's head branch.
export function useDeleteHeadBranch() {
  const qc = useQueryClient();
  return (refId: string, repo: string, number: number, branch: string, merged: boolean) =>
    deleteHeadBranch(refId, repo, number, branch, merged).then(async () => {
      void qc.invalidateQueries({ queryKey: ["pr-detail", repo, number] });
      await refreshPrs(qc);
    });
}

/// The list row for one pull request, from whichever cached list holds it.
///
/// Both `["prs"]` (My PRs) and `["reviewing"]` (To review) hold
/// `PullRequest[]`, and the detail view is reachable from either -- so
/// checking only one would leave half the rows with no seed. `["prs"]`
/// first because it is the larger list and the one that is warm on
/// launch.
///
/// Deliberately reads the CACHE rather than taking the row as a prop.
/// Threading a `PullRequest` down from `App` would seed only the click
/// path: a refetch, a remount, or a PR reached any other way would have
/// nothing, and the component would need two code paths for the same
/// data. The cache is already the single source for these rows.
function cachedRow(
  qc: QueryClient,
  repo: string | undefined,
  number: number | undefined,
): PullRequest | undefined {
  if (!repo || !number) return undefined;
  for (const key of [["prs"], ["reviewing"]]) {
    const row = qc
      .getQueryData<PullRequest[]>(key)
      ?.find((p) => p.repo === repo && p.number === number);
    if (row) return row;
  }
  return undefined;
}

/// A `PrDetail` from a list row, for the fields the two types share.
///
/// Everything here is a fact the row already carried and the user was
/// looking at when they clicked -- title, number, author, the branch
/// pair, the diff size, the merge and review state. The rest is left at
/// its EMPTY value rather than invented: no body, no comments, no review
/// threads, no checks. `PrDetailView` hides an empty section, so the
/// placeholder renders as the real view minus the parts that have not
/// arrived, not as a page of zeroes claiming to be complete.
///
/// `additions`/`deletions`/`changed_files` are the exception to "the row
/// already had it": the list query does not select them (see
/// `PRS_QUERY`), so the diff size is genuinely missing until the detail
/// lands. Zero is the honest value and the view suppresses the line at
/// zero rather than printing "+0 −0 across 0 files".
///
/// `checks_total: 0` matters: the Checks section is hidden while `checks`
/// is empty, and a non-zero total with an empty list would make it claim
/// "showing 0 of 12" on data nobody fetched.
function seedFromRow(row: PullRequest): PrDetail {
  return {
    id: row.id,
    number: row.number,
    title: row.title,
    url: row.url,
    // The list only carries OPEN pull requests, and `PrDetailView` reads
    // this to decide whether to offer "Delete branch" -- an action that
    // must never appear on a guess. Lowercase to match the mapper, which
    // lowercases GitHub's `state`.
    state: "open",
    is_draft: row.is_draft,
    body: "",
    author: row.author,
    repo: row.repo,
    head_ref: row.head_ref,
    head_oid: row.head_oid,
    head_ref_id: row.head_ref_id,
    base_ref: row.base_ref,
    merge_status: row.merge_status,
    review: row.review,
    latest_reviews: row.latest_reviews,
    // NOT on the list row. False offers a plain Merge, which is the same
    // safe direction the Rust mapper defaults to: GitHub refuses it if
    // the branch really does require the queue.
    merge_queue_enabled: false,
    in_merge_queue: row.in_merge_queue,
    additions: 0,
    deletions: 0,
    changed_files: 0,
    unresolved_threads: row.unresolved_threads,
    comment_count: row.comment_count,
    comments: [],
    review_threads: [],
    // Zero for the same reason as `checks_total` below: `ReviewThreads`
    // renders nothing while the list is empty, and a non-zero total
    // against it would claim "Showing 0 of 12" about threads nobody has
    // fetched. The row does carry `unresolved_threads`, but that is a
    // count of OPEN threads and not the connection total, so it is not a
    // stand-in here (#802).
    review_threads_total: 0,
    checks: [],
    checks_total: 0,
  };
}

/// One pull request's detail, fetched when the view opens.
///
/// Not part of the poll loop: it is per-PR and only wanted while on
/// screen.
///
/// SEEDED from the clicked row (#790). The fetch behind this is the
/// slowest in the app -- up to four serial GitHub round-trips, because
/// the check rollup pages on a cursor -- and the view used to render a
/// bare "Loading pull request…" for all of it, despite the row the user
/// just clicked already holding the title, number, author, branch pair
/// and review state. The felt wait is now the time for the BODY and the
/// CHECKS to arrive, not the time for the page to exist.
///
/// `placeholderData`, not `initialData`. The distinction is the whole
/// point: `initialData` would be written into the cache as if it had
/// been fetched, so `staleTime` would suppress the real fetch for 30
/// seconds and the body would never load. `placeholderData` is render-
/// only -- it never enters the cache, it leaves `isPlaceholderData` true
/// so the view can mark what is provisional, and the fetch runs
/// regardless.
///
/// A PR reached with no cached row (a cold launch straight into a detail
/// view, a repository filtered out of both lists) gets `undefined` back
/// and the original spinner. That path is unchanged and is why the
/// spinner branch stays.
export function usePrDetail(repo: string | undefined, number: number | undefined) {
  const qc = useQueryClient();
  return useQuery({
    queryKey: ["pr-detail", repo, number],
    // DIAGNOSTIC LOGGING (Settings > diagnostic log). `timeCall` rather
    // than `timed`, because the query function closes over per-render
    // arguments and so cannot be hoisted to module scope -- see
    // `diag.ts`. This is the line that tells a slow COMMAND from a slow
    // RENDER: the Rust side's `[diag] cmd get_pr_detail` pair brackets
    // the fetch, and the gap between the two is React's (#790).
    queryFn: () =>
      timeCall(`pr-detail`, () => getPrDetail(repo as string, number as number)),
    enabled: Boolean(repo && number),
    staleTime: 30_000,
    // Evaluated on every render, which is what makes it work: the row
    // can land in the cache AFTER this view mounts (open the app on a
    // detail view, the poll arrives a second later) and a value captured
    // once would miss it.
    placeholderData: () => {
      const row = cachedRow(qc, repo, number);
      return row ? seedFromRow(row) : undefined;
    },
    // EXPLICIT, because the inherited default is 3 and this fetch sits
    // behind octocrab's own `max_retries: 3` with a 60-second minimum
    // wait on a rate-limit response (`auth.rs`). Stacked, that is up to
    // 16 attempts for one click, each one of them able to wait out the
    // 30-second command ceiling before the next begins -- minutes of
    // spinner from a single click, which is what #790 reported.
    //
    // 1, not 0: a detail fetch really does fail transiently (a laptop
    // waking, a 502 from GitHub), and one quiet retry saves the user a
    // click. Two would be the first step back towards the multiplier.
    retry: 1,
    // A whole session, against the default five minutes (#790). `gcTime`
    // is how long an UNUSED entry survives, not how long it is trusted
    // -- `staleTime: 30_000` above is still what decides that -- so this
    // does not serve stale CI state: a revisit past 30 seconds refetches
    // either way. What it changes is whether that refetch happens BEHIND
    // the previous answer or behind a blank spinner. At the default, a
    // triage session that works through a list and comes back to a pull
    // request six minutes later paid the full cold load again.
    //
    // Unbounded deliberately. The entry is one pull request's detail, a
    // few KB; a heavy day is tens of them, and the cost of holding them
    // is far below the cost of re-fetching them through the slowest
    // query in the app. They are dropped on relaunch like everything
    // else in this cache.
    gcTime: Infinity,
    // `unknown` mergeability is TRANSIENT: GitHub sets it while it
    // recomputes, which approving a pull request is precisely what
    // triggers. One invalidation after the mutation is not enough --
    // the refetch lands while GitHub is still computing and gets
    // `unknown` back, then nothing asks again.
    //
    // Polling only in that state, and only while the detail view is
    // open. It stops the moment a real answer arrives, so this is a few
    // seconds of extra requests on one pull request rather than a
    // background cost.
    refetchInterval: (query) =>
      query.state.data?.merge_status === "unknown" ? 3_000 : false,
  });
}

/// Repos with worktrees. Listing only -- see `useWorktreeSafety`.
///
/// `staleTime` is short but non-zero: the set changes when the user
/// creates or removes a worktree, not on a timer, so refetching on every
/// mount would spend a second of subprocess work for nothing.
/// Every repository and its worktrees.
///
/// `enabled` defaults to true because the three callers that discover
/// repositories -- the sidebar, the picker, the Worktrees page -- exist
/// to show this and would only ever pass true. The parameter is for the
/// System Health footprint panel, which must not scan until the user
/// asks: its disk figures are gated behind an explicit action, and
/// discovery is part of what that gate has to cover. `list_worktrees`
/// is not free, and a scan that runs on view open is the beginning of
/// the #661 shape even when the sizing behind it is deferred.
///
/// # `data` is still the repository list (#951)
///
/// The command now returns a `WorktreeScan` -- the repositories AND the
/// paths the walk could not read -- but this hook keeps `data` as
/// `WorktreeRepo[]` and hands the shortfall back beside it as
/// `unreadable`. That is deliberate, and it is the frontend half of the
/// wrapper design on the Rust side: four components read `data` and every
/// one of them wants the repositories, so folding the scan object into
/// `data` would have churned all four plus their fixtures for no gain and
/// obscured the actual fix.
///
/// ONE query, spread and re-shaped -- not two `useQuery` calls on the same
/// key and not a second command. There is only one scan, and asking twice
/// would mean walking `~/code` twice, which is what this hook's own note
/// above and #846's `retry: false` reasoning both forbid.
///
/// `data` is left `undefined` while the query is pending and on a
/// rejection, exactly as before, because three of the four consumers
/// distinguish those from an empty list and `WorktreeSidebar`'s comment
/// says why.
export function useWorktrees(enabled = true) {
  const q = useQuery({
    queryKey: ["worktrees"],
    queryFn: listWorktrees,
    enabled,
    staleTime: 30_000,
  });
  return {
    ...q,
    data: q.data?.repos,
    /// What the walk could not read. `[]` rather than `undefined` when
    /// there is nothing to report AND while the scan is still pending: a
    /// scan that has not answered has not reported a shortfall either,
    /// and the difference between "not yet" and "nothing" is already
    /// carried by `isLoading`.
    unreadable: q.data?.unreadable ?? EMPTY_UNREADABLE,
  };
}

/// One frozen empty array, so `unreadable` keeps a stable identity when
/// there is nothing to report. A fresh `[]` per render would make every
/// `useMemo` keyed on it recompute forever.
const EMPTY_UNREADABLE: readonly string[] = Object.freeze<string[]>([]);

/// Verdicts streaming in from the Rust side, one worktree at a time.
///
/// The Rust command emits `worktree-safety` per worktree as it reaches a
/// verdict, and this collects them into a map that grows while the
/// command is still running.
///
/// Why a subscription rather than just awaiting the command -- the
/// `useStreamingSizes` argument, arriving at the column that needed it
/// more (#830). Classification's cost tracks CHANGED FILES, not worktree
/// count: MEASURED per worktree, a 114ms median against a 3295ms max
/// INSIDE one repository, a 29x spread. Awaiting the whole set means
/// every row waits on the worst branch in the repository, which on a
/// 111-worktree machine was a safety column that never resolved at all.
/// Partial answers are the entire point, so they must be observable
/// before the promise settles.
///
/// Keyed by absolute path, which is unique across repositories, so one
/// subscription serves every view without them having to agree on
/// anything -- exactly as the size stream does.
///
/// Unlike the size stream there is no null-versus-absent distinction to
/// keep: a worktree that could not be classified arrives with a real
/// `Safety` value (`unknown`, carrying why), because the Rust side has an
/// honest verdict for that case and the size pass had no honest number.
/// An absent key still means "no verdict yet".
function useStreamingSafety(): Map<string, Worktree> {
  const [seen, setSeen] = useState<Map<string, Worktree>>(() => new Map());

  useEffect(() => {
    // The same guarded teardown as every other listener here -- see
    // `usePullRequests` for why the promise cannot be unwrapped naively.
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<Worktree>("worktree-safety", (e) => {
      const w = e.payload;
      setSeen((prev) => {
        // A fresh Map, not a mutation: React compares by identity, and
        // an in-place `set` would leave every row showing its skeleton
        // because nothing re-rendered.
        const next = new Map(prev);
        next.set(w.path, w);
        return next;
      });
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, []);

  return seen;
}

/// Safety for one repo's worktrees, landing one worktree at a time.
///
/// Fetched only when that repo is selected -- classifying all 37
/// repositories up front would take ~16s.
///
/// `partial` carries the verdicts that have landed while the query is
/// still in flight. Until #830 this hook offered only the settled `data`,
/// so the page had nothing to show between "started" and "every worktree
/// classified" -- and on the reporting machine that gap never ended. The
/// size pass got this treatment in #754 on the argument that it was the
/// slow one; classification was left whole on the strength of a ~16s
/// figure that assumed a bounded number of git calls per worktree. It is
/// not bounded (see `classifyWorktrees`), so the gap had no ceiling.
///
/// `pending`, `total` and `failed` are reported for the reason
/// `useAllWorktreeSizes` reports them, and with the same separation of
/// `failed` from `pending`: a caller that only watches `pending` sees it
/// fall to zero and concludes everything was classified. Here the two
/// numbers mean:
///
///   - `pending` -- listed worktrees with no verdict yet. Falls to zero
///     when every row has heard something, success or not.
///   - `failed` -- worktrees whose verdict is `unknown`, i.e. the Rust
///     side gave up (`CLASSIFY_TIMEOUT`) or git refused. These have
///     ARRIVED, so they are not pending; they are also not answers the
///     user can act on, so reporting them inside `pending` would hide
///     them and reporting them as successes would be a lie.
///
/// `total` is the number of worktrees the listing says exist, which is
/// the only honest denominator: counting only what has arrived would make
/// "3 of 3" true at every instant of a 111-worktree classification.
///
/// NOT the default `retry: 3`. The `useWorktreeSizes` reasoning applies
/// unchanged and with a sharper edge: this pass is now bounded per
/// worktree, so a rejection is a real refusal rather than a timeout, and
/// three silent re-runs of a pass measured in seconds-to-minutes puts the
/// column back on skeletons for four times as long before saying
/// anything. A failed classification is not a flaky network call.
export function useWorktreeSafety(repoPath: string | undefined, listed?: Worktree[]) {
  const streamed = useStreamingSafety();
  const query = useQuery({
    // Keyed on the repo alone, and deliberately NOT on the worktree
    // count. A count in the key makes every sibling key change the
    // moment one worktree is removed, which refetches the entire view --
    // the mistake `useArtifactSizes` records at length.
    queryKey: ["worktree-safety", repoPath],
    queryFn: () => classifyWorktrees(repoPath as string),
    enabled: Boolean(repoPath),
    staleTime: 30_000,
    retry: false,
  });

  // The settled answer wins on any path it has: it is the authoritative
  // set, and the stream is only ever an early view of the same pass.
  // Streamed values go in FIRST so that ordering holds -- the same rule
  // `useAllWorktreeSizes` merges by.
  const verdicts = useMemo(() => {
    const out = new Map<string, Worktree>();
    // Only this repository's rows. The stream is keyed by absolute path
    // across every repository, so a previously-opened repository's
    // verdicts are still in the map and must not be counted against this
    // one's totals.
    const paths = new Set((listed ?? []).map((w) => w.path));
    for (const [p, w] of streamed) if (paths.size === 0 || paths.has(p)) out.set(p, w);
    if (query.data) for (const w of query.data) out.set(w.path, w);
    return out;
  }, [streamed, query.data, listed]);

  const total = listed?.length ?? query.data?.length ?? 0;
  const failed = [...verdicts.values()].filter((w) => w.safety.kind === "unknown").length;
  return {
    ...query,
    partial: verdicts,
    /// Listed worktrees with no verdict of any kind yet. Never negative:
    /// the stream can carry a path the current listing does not have if a
    /// worktree was removed mid-pass.
    pending: Math.max(0, total - verdicts.size),
    total,
    failed,
  };
}

/// Sizes streaming in from the Rust side, one worktree at a time.
///
/// The Rust command emits `worktree-size` per worktree as it finishes
/// walking it, and this collects them into a map that grows while the
/// command is still running.
///
/// Why a subscription rather than just awaiting the command: the walk is
/// the expensive pass by a wide margin, and its cost tracks BYTES, not
/// worktree count -- MEASURED, 21.40s for a single 200 GB checkout
/// against 0.78s for a 0.33 GB one. Awaiting the whole set means every
/// row waits on the largest tree, which on a 100-worktree machine is the
/// indefinite page of skeletons #754 reported. Partial answers are the
/// entire point, so they must be observable before the promise settles.
///
/// Keyed by absolute path, which is unique across repositories, so one
/// subscription serves both the per-repository and all-repositories
/// views without them having to agree on anything.
///
/// A null VALUE means the Rust side gave up on that worktree's walk
/// (#769); an absent KEY means it has not been walked yet. The map has
/// to keep those apart, because they render differently -- "not
/// measured" against a skeleton -- and collapsing them is what left a
/// row promising a number for 15 minutes.
function useStreamingSizes(): Map<string, number | null> {
  const [sizes, setSizes] = useState<Map<string, number | null>>(() => new Map());

  useEffect(() => {
    // The same guarded teardown as every other listener here -- see
    // `usePullRequests` for why the promise cannot be unwrapped naively.
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<[string, number | null]>("worktree-size", (e) => {
      const [path, bytes] = e.payload;
      setSizes((prev) => {
        // A fresh Map, not a mutation: React compares by identity, and
        // an in-place `set` would leave every row showing its skeleton
        // because nothing re-rendered.
        const next = new Map(prev);
        next.set(path, bytes);
        return next;
      });
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, []);

  return sizes;
}

/// Disk sizes for one repo's worktrees, keyed by path.
///
/// The slowest of the three passes by far, so it is last: the list
/// appears, then safety, then sizes.
///
/// `partial` carries the sizes that have landed while the query is still
/// in flight. Until #754 this hook offered only the settled `data`, so
/// the per-repository view had nothing to show between "started" and
/// "every worktree measured" -- and on a repository with a large
/// checkout that gap is minutes. The all-repositories view got
/// progressive fill from having one query per repository; the
/// per-repository view is ONE query, so it needed the finer grain.
///
/// Sizes change only when the tree does, so they are cached for longer
/// than the rest.
export function useWorktreeSizes(repoPath: string | undefined) {
  const partial = useStreamingSizes();
  const query = useQuery({
    queryKey: ["worktree-sizes", repoPath],
    queryFn: async () => {
      const pairs = await sizeWorktrees(repoPath as string);
      return new Map(pairs);
    },
    enabled: Boolean(repoPath),
    staleTime: 5 * 60 * 1000,
    // NOT the default `retry: 3`. This query is a full filesystem walk
    // measured in minutes on a large repository, so the default turns
    // one rejection into FOUR sequential walks with `isFetching` true
    // throughout -- a column of skeletons for four times the walk, and
    // then a silent em dash. That is #769's "15+ minutes" almost
    // exactly. A failed walk is not a flaky network call; retrying it
    // repeats a very expensive operation to get the same answer.
    retry: false,
  });
  return { ...query, partial };
}

/// Sizes for EVERY repository, landing one worktree at a time.
///
/// MEASURED, and the numbers that used to be here were wrong in a way
/// that mattered: this claimed `size_repo` "shells out to `du`" at
/// "roughly 0.75s each". It does neither -- it is a native recursive
/// walk, and its cost tracks BYTES rather than worktree count. On one
/// checkout: 21.40s for a 200 GB main checkout, 0.78s for a 0.33 GB
/// worktree, 0.01s for a 0.01 GB one. A per-worktree average is
/// meaningless, and believing one is what made #754 look like a hang
/// rather than a walk that had not finished.
///
/// Two levels of granularity, because one was not enough:
///
///   - one query PER REPOSITORY, so a repository's results land without
///     waiting for the others;
///   - `worktree-size` events merged on top, so a row fills as soon as
///     its OWN tree is walked rather than waiting for the largest tree
///     in its repository.
///
/// The per-repository queries share `worktree-sizes` keys with
/// `useWorktreeSizes`, so opening a repository afterwards is free.
///
/// `staleTime` is long for the same reason it is on the single-repo
/// hook: a worktree's size does not change unless its contents do.
export function useAllWorktreeSizes(repoPaths: string[], enabled: boolean) {
  const streamed = useStreamingSizes();
  const results = useQueries({
    queries: repoPaths.map((path) => ({
      queryKey: ["worktree-sizes", path],
      queryFn: async () => new Map(await sizeWorktrees(path)),
      enabled,
      staleTime: 5 * 60 * 1000,
      // Same reason as the single-repo hook: four sequential walks of a
      // large repository is #769's wait, not a recovery.
      retry: false,
    })),
  });

  // Merged into one map, so callers do not care that it arrived in
  // pieces. Streamed values go in FIRST so a settled query's answer
  // wins on any key it has -- the settled set is the authoritative one,
  // and the stream is only ever an early view of the same walk.
  const sizes = new Map<string, number | null>(streamed);
  for (const r of results) {
    if (r.data) for (const [k, v] of r.data) sizes.set(k, v);
  }
  return {
    sizes,
    /// How many repositories have not answered yet -- the number that
    /// makes a partially-filled page legible rather than broken.
    pending: results.filter((r) => r.isFetching).length,
    total: results.length,
    /// How many repositories FAILED outright.
    ///
    /// Counted separately from `pending`, because a failed repository
    /// leaves `pending` and never comes back: a caller that only
    /// watches `pending` sees the number fall to zero and concludes
    /// everything was measured. #769 is the shape of that mistake --
    /// silence read as success.
    failed: results.filter((r) => r.isError).length,
  };
}

/// The upstream verdict for every repository's MAIN CHECKOUT, one
/// repository at a time (#1042).
///
/// # What was wrong, and why nothing was in the logs
///
/// The All Repositories table reads `upstream` off the main checkout the
/// scan already lists, on the reasoning that the field is "already on the
/// wire". It is not, and never was: `Worktree::upstream` is written in
/// exactly one place on the Rust side, inside `classify`, which the walk
/// runs only behind a `with_safety` flag that every production caller
/// passes `false` for. The path that would have set it is `#[cfg(test)]`.
/// So the column was an indefinite skeleton BY CONSTRUCTION -- not a
/// stuck request, which would have left a trace, but no request at all.
///
/// # One query per repository, which is the whole shape
///
/// `useAllWorktreeSizes`' granularity, for its reason: a repository whose
/// git is slow must not hold the other 37 rows on skeletons. Each row
/// resolves, or fails, entirely on its own.
///
/// There is deliberately no streaming event underneath, unlike the size
/// and safety passes. Those stream because one call covers many rows and
/// the rows must not wait for each other; here the unit of work IS one
/// row, so the promise settling is the row filling and an event would be
/// a second delivery of the same single answer.
///
/// # A failed repository is an ANSWER, not a longer wait
///
/// The map carries `Upstream` values only. A repository whose
/// classification rejected is recorded as `Upstream::Unknown` carrying
/// the refusal, because the caller renders an absent entry as a PENDING
/// skeleton -- "not computed yet" -- and a row that can never be computed
/// must stop making that promise. This is the `Safety::Pending` versus
/// `Safety::Unknown` distinction the Worktrees page already draws,
/// applied to the field this table actually reads. #1042's requirement in
/// one line: nothing may skeleton forever.
///
/// # No fetch
///
/// The command reads refs already on disk and this adds nothing. The
/// table's no-fetch property is measured (#1026) and the verdicts are
/// qualified by ref age rather than made fresh by a network call to every
/// remote in the scan root -- 13 of which have never been contacted from
/// the reporting machine at all.
///
/// `staleTime` is long for `useWorktreeSizes`' reason at a different
/// scale: a repository's position against its upstream changes when the
/// user acts or when its refs are fetched, and both of those invalidate
/// this key explicitly rather than being waited for.
///
/// NOT the default `retry: 3`, the same refusal every pass in this file
/// makes: a rejection here is git declining, not a flaky network call,
/// and three silent re-runs would put the cell back on a skeleton for
/// four times as long before saying anything.
export function useRepoUpstreams(repoPaths: readonly string[], enabled = true) {
  const results = useQueries({
    queries: repoPaths.map((path) => ({
      queryKey: ["repo-upstream", path],
      queryFn: () => classifyRepoUpstream(path),
      enabled,
      staleTime: 5 * 60 * 1000,
      retry: false,
    })),
  });

  // Built inline rather than in a memo, exactly as `useAllWorktreeSizes`
  // builds its size map: `useQueries` returns a fresh results array on
  // every render anyway, so a memo over it would recompute regardless
  // while adding a dependency array to get wrong. The loop is over the
  // repositories in the scan root -- 38 on the reporting machine.
  const upstreams = new Map<string, Upstream>();
  repoPaths.forEach((path, i) => {
    const r = results[i];
    if (r?.data?.upstream) {
      upstreams.set(path, r.data.upstream);
      return;
    }
    // The rejection arm, and it must produce a VALUE. `r.error` is the
    // Rust refusal, which already says what failed -- "could not list
    // worktrees: ..." -- so it is carried through rather than replaced
    // with a sentence of this layer's own.
    if (r?.isError) {
      upstreams.set(path, { kind: "unknown", n: errorText(r.error) });
    }
    // Anything else is genuinely still in flight, and an absent key is
    // how the caller spells that. It renders as a skeleton, which is
    // honest for exactly as long as the query is running -- and only
    // then, which is the whole of #1042.
  });

  return {
    upstreams,
    /// Repositories with no answer of any kind yet. Falls to zero when
    /// every row has heard something, success or failure.
    pending: results.filter((r) => r.isPending).length,
    /// Repositories whose classification REJECTED, counted separately
    /// from `pending` for `useAllWorktreeSizes`' reason: a caller
    /// watching only `pending` sees it reach zero and concludes
    /// everything was measured. #769 is the shape of that mistake.
    failed: results.filter((r) => r.isError).length,
    total: results.length,
  };
}

/// A query rejection as the string the cell will show.
///
/// Tauri rejects with the command's own `Err` string, which is already
/// display-ready and names what failed. An `Error` instance is unwrapped
/// to its message; anything else is stringified rather than dropped,
/// because a cell that says a failure happened without saying what is
/// still better than a cell that keeps promising an answer.
function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
/// Remove a worktree, then refresh both queries.
///
/// Deliberately NOT optimistic. Every other mutation in this app updates
/// locally first, but this one deletes files: showing a row as gone
/// before the deletion succeeded would be a lie about the filesystem, and
/// the failure case here is "your work is still there", which the user
/// needs to see rather than have hidden.
/// Fast-forward a checkout to its upstream.
///
/// Invalidates the worktree list on success rather than patching a row:
/// the upstream line is the whole reason the action exists, and a
/// successful pull that left "behind by 40" on screen would look like
/// it had failed. Unlike removal there is no long re-classification to
/// avoid -- the row count does not change.
export function usePullCheckout() {
  const qc = useQueryClient();
  return (path: string) =>
    pullCheckout(path).then((out) => {
      void qc.invalidateQueries({ queryKey: ["worktrees"] });
      // AND the safety classification, which is what actually renders
      // the "N commits behind upstream" line. Invalidating only the
      // base list left the row yellow for ~10 seconds after a
      // SUCCESSFUL pull, until something else happened to refresh it --
      // so the button looked like it had done nothing, which is exactly
      // what #346 reported.
      void qc.invalidateQueries({ queryKey: ["worktree-safety"] });
      // AND the overview's verdicts (#1042), which are a THIRD cache
      // holding the same ahead/behind fact for the All Repositories
      // table. It has a five-minute `staleTime`, so without this a
      // successful pull would leave that table saying "40 behind" for
      // five minutes -- #346's report, in the new place.
      void qc.invalidateQueries({ queryKey: ["repo-upstream"] });
      return out;
    });
}

/// Refresh one repository's remote refs without moving a branch (#788).
///
/// Invalidates exactly what `usePullCheckout` does, and for the same
/// reason: `fetched_at` lives on the worktree LIST (it is
/// `FETCH_HEAD`'s mtime, stat'd during the scan) while the
/// ahead/behind counts come from the CLASSIFICATION pass, so a refresh
/// that invalidated only one of the two would leave the page
/// half-updated -- a row whose age note said "just now" beside an
/// ahead/behind from before the fetch, which is a worse lie than the
/// stale one this feature exists to stop.
///
/// Does NOT unwrap git's output into a user-facing message. `git fetch`
/// prints its progress to stderr and nothing to stdout, so a successful
/// refresh resolves to the empty string -- the caller must phrase its
/// own success line. That asymmetry with `usePullCheckout`, whose `git
/// pull` does say "Already up to date.", is the reason this is not
/// simply the same hook with a different command string.
export function useFetchRefs() {
  const qc = useQueryClient();
  return (path: string) =>
    fetchRefs(path).then((out) => {
      void qc.invalidateQueries({ queryKey: ["worktrees"] });
      void qc.invalidateQueries({ queryKey: ["worktree-safety"] });
      // And the overview's verdicts (#1042). Sharpest here of the three:
      // this feature exists to make a stale verdict fresh, and the All
      // Repositories table is the surface that most loudly qualifies its
      // verdicts by ref age. A refresh that updated the age note and not
      // the verdict beside it would be the exact half-updated row this
      // hook's own comment refuses.
      void qc.invalidateQueries({ queryKey: ["repo-upstream"] });
      return out;
    });
}

/// Fast-forward every repository in the scan roots (#1012).
///
/// Invalidates exactly what `usePullCheckout` does, and for the same
/// reason: `fetched_at` lives on the worktree LIST while the ahead/behind
/// counts come from the CLASSIFICATION pass, so refreshing one without
/// the other leaves the page half-updated -- a row whose age note says
/// "just now" beside an ahead/behind from before the pull.
///
/// Invalidated even when the run reports failures or was cancelled, and
/// that is deliberate: a partial run really did move some repositories,
/// and leaving those rows saying "40 behind" would make a run that
/// half-worked look like one that did nothing -- which is #346's exact
/// report about the single-repository button.
export function useUpdateAllRepositories() {
  const qc = useQueryClient();
  return () =>
    updateAllRepositories().then((report) => {
      void qc.invalidateQueries({ queryKey: ["worktrees"] });
      void qc.invalidateQueries({ queryKey: ["worktree-safety"] });
      // And the overview's verdicts (#1042). This button lives ON the
      // All Repositories table, so its own rows are the ones that would
      // otherwise sit stale for five minutes -- a run that moved 30
      // repositories, reporting so, above a table still saying they are
      // behind.
      void qc.invalidateQueries({ queryKey: ["repo-upstream"] });
      return report;
    });
}

/// Ask the Update All run to stop.
///
/// Rejects when nothing is running, which the caller must surface rather
/// than swallow: a Cancel that appears to work on a run that already
/// finished is its own small lie.
export function useCancelUpdateAll() {
  return () => cancelUpdateAll();
}

/// How far the Update All run has got, or null when idle.
///
/// The same shape as `useRemovalProgress` and `useUpdateProgress` beside
/// it, and for a reason those two make with room to spare: removal was
/// judged to warrant progress at ~30 seconds, and this run's BEST case is
/// 40 seconds while its worst is 22 minutes. MEASURED -- 45 repositories,
/// `git fetch --dry-run` at 25ms-1055ms each, and every call bounded by
/// `GIT_TIMEOUT` at 30s when a remote is unreachable.
///
/// Counts only. The event carries no paths, which is the rule all three
/// emitters state in identical terms: a progress event is not a place to
/// leak what the user is working on. The paths are in the RESULT, where
/// the user needs them.
export function useUpdateAllProgress(): { done: number; total: number } | null {
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<[number, number]>("update-all-progress", (e) => {
      const [done, total] = e.payload;
      // Cleared on the last one rather than leaving "45 of 45" on screen
      // after the work is over; the returned report carries the outcome.
      setProgress(done >= total ? null : { done, total });
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, []);

  return progress;
}

/// How big ONE orphaned directory is, measured when asked (#845).
///
/// Every other size on the worktree page comes from `sizeWorktrees`,
/// and an orphan is the one row that cannot use it: that command opens
/// with `git worktree list` inside the repository, and an orphan's
/// repository is exactly what is gone -- so it returns `Err` rather
/// than a number. The Orphaned view therefore has NO size at all (the
/// listing leaves `size_bytes: None`, and `useAllWorktreeSizes` is
/// gated on the all-repositories view), which is why the confirmation
/// this feeds had nothing to say about how much was about to go.
///
/// So it reuses `sizeArtifacts`, which takes EXPLICIT PATHS and walks
/// them with no git involved -- the one existing command that can
/// measure a directory nothing owns. The name is a misnomer here and
/// that is deliberate: the command string is matched as a literal in
/// two remote-surface allowlists, one of which ships in the phone app
/// on its own release tag, so a clearer name would break a phone
/// paired with an older desktop. `tauri.ts:874` records the same trade
/// for the same reason. It is `Class::Read` in both surfaces, so this
/// works from the companion as well as the desktop.
///
/// `enabled` is the dialog being open, not the row existing. A walk per
/// orphan on mount would measure directories nobody asked about -- and
/// on the machine that prompted #845 that is 2.5 GB across three trees.
/// Measuring at the moment of the question is also what makes the
/// number trustworthy: it describes the directory as it is now, not as
/// it was when the page loaded.
///
/// `retry: false`, matching every other sizing query here. A failed
/// walk is not a flaky network call, and the dialog has a BRANCH for
/// "could not be measured" -- repeating an expensive walk three times
/// to reach the same branch only delays the question.
export function useOrphanSize(path: string | undefined, enabled: boolean) {
  const query = useQuery({
    queryKey: ["orphan-size", path],
    queryFn: async () => {
      const out = await sizeArtifacts([path as string]);
      // The walk answers for the one path it was given, or for none.
      // An empty result is NOT zero bytes: zero reads as "this tree is
      // empty, delete it", which for an unmeasurable directory is the
      // most damaging thing this dialog could say -- the same rule
      // `size_worktrees` states about flattening its own nulls.
      return out[0]?.[1] ?? null;
    },
    enabled: enabled && Boolean(path),
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
  return {
    /// The measured bytes, or null when the walk answered nothing.
    bytes: query.data ?? null,
    /// Still walking. Kept separate from a null result for the reason
    /// `VenvSection` learned the hard way: "still measuring" and "there
    /// is nothing to measure" are opposite answers, and one value for
    /// both is how a dialog ends up stating a figure it does not have.
    measuring: query.isFetching,
    /// The walk REFUSED. Distinct from a null result again: this is the
    /// case the dialog must name out loud, because it is the second
    /// thing about an orphan that could not be checked.
    failed: query.isError,
  };
}

/// Delete an orphaned worktree directory.
///
/// Invalidates rather than patching a row: an orphan's removal changes
/// what the Orphaned section contains, and that section disappears
/// entirely at zero.
export function useRemoveOrphan() {
  const qc = useQueryClient();
  return (path: string) =>
    removeOrphan(path).then(() => {
      void qc.invalidateQueries({ queryKey: ["worktrees"] });
    });
}

export function useRemoveWorktree() {
  const qc = useQueryClient();
  return (repoPath: string, worktreePath: string) =>
    removeWorktree(repoPath, worktreePath).then(() => {
      // Drop the row rather than invalidating. Invalidation re-runs
      // `classify_repo` over EVERY worktree in the repo, sequentially --
      // ~0.35s each, so 51 seconds on a 146-worktree repo, during which
      // the deleted row sits there looking undeleted.
      //
      // It is also unnecessary: removing a worktree cannot change any
      // other worktree's safety, since each verdict is computed from that
      // worktree's own state. Filtering the cache is instant and exactly
      // as accurate as re-running 146 git commands.
      qc.setQueryData<Worktree[]>(["worktree-safety", repoPath], (old) =>
        old?.filter((w) => w.path !== worktreePath),
      );
      // The repo listing IS invalidated: it is cheap, and a repo that
      // just lost its last worktree should leave the sidebar.
      void qc.invalidateQueries({ queryKey: ["worktrees"] });
    });
}

/// Remove every safe worktree in a repo.
///
/// Drops the successful paths from the cache rather than invalidating,
/// for the same reason a single removal does: re-classifying 146
/// worktrees takes ~51s, and removing worktrees cannot change any other
/// worktree's safety.
export function useRemoveWorktrees() {
  const qc = useQueryClient();
  return (repoPath: string, worktreePaths: string[]) =>
    removeWorktrees(repoPath, worktreePaths).then((outcomes) => {
      const removed = new Set(
        outcomes.filter((o) => o.error === null).map((o) => o.path),
      );
      // EVERY cached classification, not just `repoPath`'s.
      //
      // The bulk button's targets come from what is DISPLAYED, which on
      // the all-repositories view spans many repos -- while this only
      // ever updated the selected one. So worktrees in other
      // repositories were really removed, the toast correctly said so,
      // and their rows came straight back because their cache still
      // held them. `repoPath` is also "" when nothing is selected, and
      // then this updated a key that does not exist at all.
      //
      // Filtering every cached list by path is safe regardless of which
      // repository a path belongs to: a path that is not in a list
      // leaves it unchanged.
      qc.setQueriesData<Worktree[]>({ queryKey: ["worktree-safety"] }, (old) =>
        old?.filter((w) => !removed.has(w.path)),
      );
      // The base listing too, and by EDITING it rather than only
      // invalidating.
      //
      // The page renders `classified ?? selected?.worktrees` -- so when
      // the classification has not arrived (or was cleared), it falls
      // back to this list. Invalidation alone leaves the stale rows on
      // screen until the refetch lands, which is what "the same 3
      // worktrees are still listed" was: they really were removed, and
      // the fallback was still serving them.
      //
      // `unreadable` is carried through UNCHANGED (#951). Removing a
      // worktree says nothing about a path the scan could not read, so
      // dropping the report here would clear the partial-scan banner on
      // an unrelated action -- and the next refetch would bring it back,
      // which is how a warning becomes noise nobody trusts.
      qc.setQueryData<WorktreeScan>(["worktrees"], (old) =>
        old && {
          ...old,
          repos: old.repos.map((r) => ({
            ...r,
            worktrees: r.worktrees.filter((w) => !removed.has(w.path)),
          })),
        },
      );
      void qc.invalidateQueries({ queryKey: ["worktrees"] });
      return outcomes;
    });
}

/// Which worktrees have been assessed, so the row can say so.
export function useAssessed() {
  return useQuery({
    queryKey: ["assessed-worktrees"],
    queryFn: assessedWorktrees,
    staleTime: 5_000,
  });
}

/// Record that a human read an assessment.
///
/// Invalidates the assessed list so the row's action updates -- which is
/// the point: the change is now the deliberate result of the user saying
/// they read the verdict, rather than a delayed side effect of copying a
/// command.
export function useMarkAssessed() {
  const qc = useQueryClient();
  return async (worktreePath: string) => {
    await markAssessed(worktreePath);
    await qc.invalidateQueries({ queryKey: ["assessed-worktrees"] });
  };
}

/// Forget a worktree's assessment.
///
/// The inverse of `useMarkAssessed`, and the way back from a one-way
/// door: the mark persists across restarts, so without this an
/// exploratory click removed that worktree's Claudify action for good.
export function useClearAssessed() {
  const qc = useQueryClient();
  return async (worktreePath: string) => {
    await clearAssessed(worktreePath);
    await qc.invalidateQueries({ queryKey: ["assessed-worktrees"] });
  };
}

/// Remove a worktree past the safety gate.
///
/// Separate hook from `useRemoveWorktree` on purpose: the two are not
/// interchangeable, and a single function with a boolean would make the
/// dangerous call one typo away from the safe one.
export function useRemoveWorktreeForced() {
  const qc = useQueryClient();
  return (repoPath: string, worktreePath: string) =>
    removeWorktreeForced(repoPath, worktreePath).then(() => {
      qc.setQueryData<Worktree[]>(["worktree-safety", repoPath], (old) =>
        old?.filter((w) => w.path !== worktreePath),
      );
      void qc.invalidateQueries({ queryKey: ["assessed-worktrees"] });
      void qc.invalidateQueries({ queryKey: ["worktrees"] });
    });
}

/// Clear a worktree's lock (#775).
///
/// INVALIDATES rather than patching the row, which is the opposite of
/// what `useRemoveWorktree` does and deliberately so. Removal knows the
/// answer -- the row is gone -- so it filters the cache and skips
/// re-running `classify_repo` over every worktree. Unlocking does not:
/// the whole point is that the verdict underneath was hidden by the
/// lock, and the row must now be re-classified to show it.
///
/// Patching in the `underlying` verdict the lock was carrying would be
/// the tempting shortcut and would be wrong twice over. It was computed
/// at scan time and the tree may have moved since; and it would make
/// the app's own display value into a safety verdict, which is the one
/// place a stale answer deletes something. The gate re-checks at
/// removal time regardless, so a wrong row here would only mislead.
export function useUnlockWorktree() {
  const qc = useQueryClient();
  return (repoPath: string, worktreePath: string) =>
    unlockWorktree(repoPath, worktreePath).then(() => {
      void qc.invalidateQueries({ queryKey: ["worktree-safety"] });
      void qc.invalidateQueries({ queryKey: ["worktrees"] });
    });
}

/// Clear the locks on several worktrees, reporting each one's outcome
/// (#792).
///
/// N sequential unlocks, not one bulk call, and deliberately no new Rust
/// command for it. `unlock_worktree` re-checks the target against the
/// repository's own listing on every call, and that per-target check is
/// the thing worth keeping: a batch endpoint would either repeat it N
/// times anyway or drop it, and dropping it on the bulk path is how a
/// bulk path becomes the dangerous one.
///
/// `allSettled`, never `all`. One row somebody else already unlocked
/// between the scan and the click is an ordinary race -- the scan is a
/// snapshot -- and `all` would abandon the remaining unlocks over it.
/// Partial success is the normal case, so the caller is handed every
/// outcome and says so.
///
/// Sequential rather than parallel. Each call shells out to `git
/// worktree list`, and 20 of those at once on a 150-worktree repository
/// is a thundering herd for no gain: the work is git's disk, not ours.
///
/// Invalidates ONCE at the end rather than per unlock, for the reason
/// `useUnlockWorktree` documents -- the row must be re-classified
/// because the verdict underneath was hidden by the lock -- but twenty
/// invalidations would mean twenty `classify_repo` passes over every
/// worktree in the repository.
export function useUnlockWorktrees() {
  const qc = useQueryClient();
  return async (repoPath: string, worktreePaths: string[]) => {
    const outcomes: { path: string; error: string | null }[] = [];
    for (const path of worktreePaths) {
      try {
        await unlockWorktree(repoPath, path);
        outcomes.push({ path, error: null });
      } catch (e) {
        // The same unwrapping `errorMessage` does in `QueryError.tsx`,
        // inlined rather than imported: this is the api layer and that is
        // a component module, so importing it here would invert the
        // layering for four lines. A bare `String(e)` would render
        // "Error: that worktree is not locked" -- git's own words with
        // a class name bolted on -- where the rest of the app shows the
        // message alone.
        //
        // Falls back to a fixed sentence rather than to `String(e)` for
        // anything else: a thrown object stringifies to "[object
        // Object]", and a row in a failure list reading that tells the
        // user strictly less than "it failed" does.
        const msg =
          typeof e === "string"
            ? e
            : e instanceof Error
              ? e.message
              : "the unlock failed for an unknown reason";
        outcomes.push({ path, error: msg });
      }
    }
    void qc.invalidateQueries({ queryKey: ["worktree-safety"] });
    void qc.invalidateQueries({ queryKey: ["worktrees"] });
    return outcomes;
  };
}

/// Clear a repository's stale worktree registrations (#793).
///
/// INVALIDATES rather than patching the cache, and here that is not even
/// a choice between two honest options. `git worktree prune` is
/// repo-wide and git decides what it clears, so the only way to know
/// which rows went is to ask again -- filtering the cache by what we
/// hoped it would do would leave a row on screen that no longer exists,
/// or remove one that does.
///
/// Both keys, like the unlock above: `worktrees` holds the listing the
/// pruned registrations appear in, and `worktree-safety` holds the
/// verdicts computed over it.
export function usePruneWorktrees() {
  const qc = useQueryClient();
  return (repoPath: string) =>
    pruneWorktrees(repoPath).then((cleared) => {
      void qc.invalidateQueries({ queryKey: ["worktree-safety"] });
      void qc.invalidateQueries({ queryKey: ["worktrees"] });
      return cleared;
    });
}

/// A local-store failure: a full disk, a locked database.
///
/// Its own channel, deliberately. It used to share `poll-error`, which
/// `prs-updated` clears -- and `persist_and_emit` emits the error then
/// unconditionally emits `prs-updated`, so the banner was destroyed
/// microseconds after it appeared. A store failure also describes a
/// condition a later successful poll did not fix, so nothing clears it
/// but the user.
export function useStoreError(): { message: string | null; dismiss: () => void } {
  const [msg, setMsg] = useState<string | null>(null);
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<string>("store-error", (e) => setMsg(e.payload)).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, []);
  return { message: msg, dismiss: () => setMsg(null) };
}

/// --- Docker -------------------------------------------------------

/// Whether Docker can be talked to. Polled slowly: the daemon starting
/// or stopping is the kind of thing a user does, so the view should
/// notice without them reloading.
export function useDockerState() {
  return useQuery({
    queryKey: ["docker-state"],
    queryFn: dockerState,
    refetchInterval: 15_000,
    staleTime: 5_000,
  });
}

/// The image list, with provenance. Only fetched when Docker is up --
/// asking a stopped daemon just produces an error the view already
/// explains better.
export function useDockerImages(enabled: boolean) {
  return useQuery({
    queryKey: ["docker-images"],
    queryFn: dockerImages,
    enabled,
    staleTime: 10_000,
  });
}

/// The PR Stats scope hierarchy: organisations, their repositories and
/// their members, plus the viewer's own repositories (#825).
///
/// `enabled` threads the explicit-load rule from the caller, which is the
/// live pattern in this file (`useArtifacts`, `useDockerImages`,
/// `useAllWorktreeSizes`, ...) and what #823 settled on after the "Measure"
/// button it originally cited was removed by #796. Here it means the tree is
/// enumerated when the PR Stats sidebar is on screen and not before -- a
/// user on My PRs does not pay for a hierarchy they are not looking at.
///
/// # Why this is cheap enough to run on entering the view
///
/// It is DISCOVERY, not measurement, which is the split `useArtifacts` and
/// `useArtifactSizes` are built around above. MEASURED on the live API: 2
/// rate-limit points and ~1.6s for a 2-org / 8-member / 65-repository
/// hierarchy, because it carries no statistics at all. The expensive part of
/// this feature is what a CLICK on one of these rows costs, and that is
/// `stats_count`'s problem, behind its own budget check.
///
/// # `staleTime`
///
/// Five minutes. Longer than the Docker hooks' ten seconds because the
/// answer changes on a human timescale -- someone joins the org, someone
/// creates a repository -- and far shorter than `useArtifacts`' because a
/// stale roster offers scopes that may no longer exist. The Rust command
/// deliberately does NOT cache this (unlike `stats_count`), so this
/// `staleTime` is the only thing stopping a re-fetch per navigation, which
/// is the right layer for "do not re-ask while the user is still here".
export function useStatsTree(enabled: boolean) {
  return useQuery({
    queryKey: ["stats-tree"],
    // `timeCall` rather than `timed` for all five stats queries (#853):
    // the other four close over per-render scope arguments, so a hoisted
    // `timed()` wrapper is not available to them, and `stats-tree` uses
    // the same form to keep the five reading alike in the log.
    queryFn: () => timeCall("stats-tree", statsTree),
    enabled,
    staleTime: 5 * 60 * 1000,
  });
}

/// A scope selection, as the sidebar writes it and the stats commands read
/// it (#825 / #826).
///
/// Carried as one object rather than three loose arguments because the three
/// ARE one selection -- `setStatsScope`'s doc comment records why writing
/// them separately passes through states that are not selections at all.
/// Keeping them together through the query layer means a hook cannot be
/// handed a subject with the wrong scope.
export interface StatsScope {
  kind: "repo" | "org" | "user" | "all";
  value: string | undefined;
  /// The person, or `undefined` for the viewer. Only a Members row sets
  /// one, and it KEEPS the org scope.
  subject: string | undefined;
}

/// Whether a scope is loadable at all.
///
/// `all` needs no value; every other kind does. Checked here rather than in
/// each hook so a half-written selection cannot reach a command and come
/// back as "scope org needs a value" -- an error message about an internal
/// contract, shown to a user who only clicked a row.
export function scopeIsLoadable(scope: StatsScope | undefined): scope is StatsScope {
  return !!scope && (scope.kind === "all" || !!scope.value);
}

/// A stable cache key fragment for a scope.
///
/// Built from the three fields rather than from the object, because a
/// TanStack query key is compared structurally and an object literal rebuilt
/// each render would be a new key on every render. Prefixed per field so
/// `org:a` and `user:a` cannot collide -- genuinely different questions, and
/// a board computed for one must never be served for the other. Mirrors the
/// Rust `Scope::cache_key`'s reasoning at the cache layer that actually
/// holds the answer.
///
/// The subject is part of the key for the series (which narrows to one
/// person) and deliberately NOT passed to the board (which is about
/// everyone). Both are below.
function scopeKey(scope: StatsScope): string {
  return `${scope.kind}:${scope.value ?? ""}`;
}

/// The per-author board behind the Mine and Others views (#826).
///
/// # Nothing loads until clicked
///
/// `enabled` is threaded from the caller, which is the live pattern every
/// expensive hook here follows (`useArtifacts`, `useAllWorktreeSizes`,
/// `useDockerImages`, `useSystemFootprint`). The old "Measure button" is
/// gone -- #796 removed the last one and `SystemHealthPage.tsx:1763-1788`
/// argues against re-adding one -- so the gate is a prop, and the click that
/// opens it is the sidebar row.
///
/// A scope with no value is not loadable and is gated out here rather than
/// erroring in Rust, so arriving at the view with nothing selected shows an
/// empty state rather than a failed query.
///
/// # The query key carries no collection size
///
/// `hooks.ts:1741-1755`'s rule: a count in a key makes every sibling key
/// change when one item is removed, refetching everything. So the key is the
/// QUESTION -- scope, measure, window -- and never the number of authors or
/// slices the answer happens to contain. The answer's own size is in the
/// answer.
///
/// Deliberately NOT keyed on `subject` either. A board is about everyone in
/// the scope, so clicking a colleague in the sidebar must NOT refetch it:
/// the same board answers "how is this org doing" and "how is this person
/// doing in it", and the UI picks a row. That is what makes a Members click
/// free after the org has been loaded once.
///
/// # `staleTime`
///
/// Five minutes, matching the other stats hooks. The window ends yesterday,
/// so the answer for a closed window cannot change at all; five minutes is
/// about the window that includes today, and about not re-spending a
/// multi-point load because the user switched tabs and came back.
///
/// Since #836 this is no longer the ONLY thing holding a board. `stats_board`
/// persists a closed window's answer through `store::stats`, so a board also
/// survives a restart -- which is the case `staleTime` could never cover and
/// the one that was re-spending the full ~45-point load on every cold start.
/// The two layers answer different questions and both are wanted: this one
/// stops a re-fetch inside a session, the store stops one across sessions.
///
/// `retry: false`, for `useAllWorktreeSizes`' reason rather than by default:
/// a failed board is an expensive thing to repeat silently, and the view has
/// an explicit retry that tells the user it is trying again.
export function useStatsBoard(
  scope: StatsScope | undefined,
  measure: "merged" | "opened",
  days: number,
  enabled: boolean,
) {
  const loadable = scopeIsLoadable(scope);
  return useQuery({
    queryKey: [
      "stats-board",
      loadable ? scopeKey(scope) : "none",
      measure,
      days,
    ],
    queryFn: () =>
      timeCall(`stats-board[${scopeKey(scope!)} ${measure} ${days}d]`, () =>
        statsBoard(scope!.kind, scope!.value, measure, days),
      ),
    enabled: enabled && loadable,
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
}

/// The scoped daily activity series (#826).
///
/// A SEPARATE query from the board, which is the progressive-rendering
/// requirement rather than a preference: `StatsPage.tsx:12-22` records that
/// three independent queries rendering as each lands beat one combined gate,
/// because the costs differ enough that blocking on the slowest left the
/// fast numbers finished and invisible. This one is count-only and lands in
/// about a second per ten days; a board over a busy organisation is seconds
/// of node fetching. #826 notes an org "Others" view has MORE parts and more
/// variance, so a single gate would be worse here than it was there.
///
/// Keyed ON the subject, unlike the board, and the asymmetry is the point: a
/// chart draws one line, so "this person in this org" is a different chart
/// and must not be served the organisation's.
export function useStatsSeries(
  scope: StatsScope | undefined,
  days: number,
  enabled: boolean,
) {
  const loadable = scopeIsLoadable(scope);
  return useQuery({
    queryKey: [
      "stats-series",
      loadable ? scopeKey(scope) : "none",
      scope?.subject ?? "*",
      days,
    ],
    queryFn: () =>
      timeCall(
        `stats-series[${scopeKey(scope!)} ${scope?.subject ?? "*"} ${days}d]`,
        () => statsSeries(scope!.subject, scope!.kind, scope!.value, days),
      ),
    enabled: enabled && loadable,
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
}

/// The scoped counts behind the headline cards (#826).
///
/// Two `stats_count` calls -- merged and opened -- run as independent
/// queries rather than one, so the faster of the two paints first and a
/// failure in one does not blank the other. That is the same progressive
/// rule as above applied one level down, and it matters here because the two
/// are genuinely independent questions to GitHub.
///
/// # `failed` is counted separately from `pending`
///
/// `hooks.ts:1397-1434`'s rule, and the reason is stated there: "a failed
/// repository leaves `pending` and never comes back: a caller that only
/// watches `pending` sees the number fall to zero and concludes everything
/// was measured". The same shape applies to two counts -- a caller watching
/// only `pending` would render a page that looks fully measured while one
/// card is missing. #826 requires a failed sub-query be distinguishable from
/// a zero, and this is where that distinction is produced.
export function useScopedCounts(
  scope: StatsScope | undefined,
  days: number,
  enabled: boolean,
) {
  const loadable = scopeIsLoadable(scope);
  const results = useQueries({
    queries: (["merged", "opened"] as const).map((measure) => ({
      // The subject IS in this key: a count is about one person or about
      // everyone, and those are different numbers. `stats_count` takes a
      // login or `null` for the viewer -- it has no "everyone", which is
      // `stats_board`'s question -- so an unset subject means the viewer.
      queryKey: [
        "stats-count",
        loadable ? scopeKey(scope) : "none",
        scope?.subject ?? "@me",
        measure,
        days,
      ],
      queryFn: () =>
        timeCall(
          `stats-count[${scopeKey(scope!)} ${scope?.subject ?? "@me"} ${measure} ${days}d]`,
          () => statsCount(scope!.subject, scope!.kind, scope!.value, measure, days),
        ),
      enabled: enabled && loadable,
      staleTime: 5 * 60 * 1000,
      retry: false,
    })),
  });
  const [merged, opened] = results;
  return {
    merged: merged?.data,
    opened: opened?.data,
    /// Still in flight.
    pending: results.filter((r) => r.isFetching).length,
    /// FAILED, which is not the same as still pending and not the same as
    /// zero. A caller must be able to say "could not measure" rather than
    /// printing a 0 it did not verify.
    failed: results.filter((r) => r.isError).length,
    /// The first error, for a message the user can act on.
    error: results.find((r) => r.isError)?.error,
    refetch: () => {
      for (const r of results) void r.refetch();
    },
  };
}

/// The reviews-GIVEN board: who reviewed the most, per scope (#826).
///
/// # Why this is a separate query from the board, and not a field on it
///
/// `useStatsBoard` reads `reviews { totalCount }` off pull requests the
/// ROW'S AUTHOR wrote, which counts review their work RECEIVED. Reviews
/// GIVEN cannot be derived from that document at all, at any cost: a PR node
/// says how many reviews it attracted, never who wrote them -- so the two
/// boards are answers to different questions over different searches, and
/// #829 was right to refuse to print one under the other's title.
///
/// MEASURED, and the two genuinely name different people on this account's
/// own data (live API, 2026-09-11): a `reviewed-by:<viewer>` search over an
/// org window returned two pull requests, both AUTHORED BY SOMEONE ELSE and
/// each carrying `reviews { totalCount } == 1`. So those same two pull
/// requests credit the AUTHOR on the received board and the REVIEWER on the
/// given one. A single chart could not have been both.
///
/// # Cost: one point, and the expense is LATENCY
///
/// One `reviewed-by:<login>` search per member, aliased into one document.
/// MEASURED live 2026-09-11 against `org:FNX-Labs`, 3 runs per cell:
///
/// | Reviewer aliases | Cost | Wall clock |
/// |---|---|---|
/// | 4 (this account's real org size) | **1** | 0.84-1.04s |
/// | 10 | **1** | 1.26-1.50s |
/// | 36 | **1** | 3.62-4.15s |
///
/// So alias count is free on rate limit and linear in latency, exactly as
/// #823 measured for the history document -- and this is the CHEAP document
/// shape, count-only with no `nodes`, which is why 36 aliases answer in 3.6s
/// where 10 node-bearing aliases at a 50-node page failed outright at the
/// ~11s deadline (`board.rs`'s table). That is also why the page size is
/// never raised: `fetch::SLICE_PAGE_FULL` is 50 and `degrade` sheds pages
/// before aliases because NODES drive the deadline, and this document
/// materialises none.
///
/// `totalCount` is read UNPAGED and must stay that way. #826's measurement
/// is that the `first:` ARGUMENT is what GitHub prices, not the connection
/// -- so adding `first:` to a connection read only for a count would buy
/// nothing and cost a point per search. There is a partial disagreement on
/// record in #823 about whether the paged form is actively more expensive;
/// both measurements agree the unpaged form is free, so this takes the cheap
/// path that neither disputes.
///
/// # Where the logins come from, and why they are not in the key
///
/// The caller passes the roster it already holds for this scope, read off
/// `useStatsTree`'s `org.members` -- so no request is spent re-deriving a
/// list that is on screen in the sidebar beside the board.
///
/// The key carries the scope and the window and deliberately NOT the member
/// list. That is this file's standing rule, which `useStatsBoard` states as
/// "a count in a key makes every sibling key change when one item is
/// removed, refetching everything": a roster that gained a person would
/// invalidate every window's cached board. The logins are an INPUT to the
/// request rather than part of its identity -- the question is "who reviewed
/// most in this scope and window", and that question is the same question
/// when the roster changes. The consequence, stated because it is a real
/// trade rather than a free win: a newly-added member does not appear until
/// the five-minute `staleTime` lapses. Five minutes of a missing row beats
/// re-spending every board in the cache on a roster edit, and a roster that
/// changed mid-session is the rarer event by far.
///
/// # `enabled`
///
/// Threaded from the caller like every other expensive hook here, so nothing
/// loads until a scope is clicked. Also gated on there BEING logins: a scope
/// with no roster (a repository, Personal, Everything) has nobody to ask
/// about, and the Rust command rejects an empty list rather than returning an
/// empty board that would read as "nobody reviewed anything".
export function useStatsReviewers(
  scope: StatsScope | undefined,
  days: number,
  logins: string[],
  enabled: boolean,
) {
  const loadable = scopeIsLoadable(scope);
  return useQuery({
    queryKey: ["stats-reviewers", loadable ? scopeKey(scope) : "none", days],
    queryFn: () =>
      timeCall(
        `stats-reviewers[${scopeKey(scope!)} ${days}d n=${logins.length}]`,
        () => statsReviewers(scope!.kind, scope!.value, days, logins),
      ),
    enabled: enabled && loadable && logins.length > 0,
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
}

/// The period comparisons behind the unscoped page's delta cards.
///
/// # Restored by #826's reopening, after #829 removed it as "superseded"
///
/// It was not superseded, and the distinction is the whole reason this hook
/// exists beside the scoped ones above. A scope page answers "how is THIS
/// organisation / repository / person doing", which requires choosing one
/// first. This answers "how am I doing, across everything" with NO selection
/// at all -- and `search` with an `author:@me` qualifier and NO repository
/// qualifier is the only query shape that spans every organisation the
/// viewer contributes to, owned or not (`github/query.rs:241-258`).
///
/// MEASURED live 2026-09-11, 30-day window ending yesterday, one aliased
/// document at cost 1: account-wide `author:@me is:merged` returns **893**
/// merged pull requests. The nearest scoped equivalent, `Personal` /
/// `All repos` (`user:pktstorm`), returns **317** -- 35% of it. The rest is
/// `org:FNX-Labs` (494) and `org:Stohic` (82): org repositories the viewer
/// contributes to without owning, which no single sidebar row covers and
/// which on this account is most of the activity. So "All repos" is not a
/// narrower spelling of account-wide; it is a different and much smaller
/// number, and presenting the scoped page as a replacement lost 576 of 893
/// pull requests with nothing on screen to say so.
///
/// Separate from `useHistory` so the four headline numbers appear in about
/// a second rather than waiting on the whole daily series. Same staleTime,
/// so the two stay consistent within a session.
export function usePeriods() {
  return useQuery({
    queryKey: ["periods"],
    queryFn: getPeriods,
    staleTime: 5 * 60 * 1000,
  });
}

/// The daily series behind the unscoped page's activity chart.
///
/// Held for five minutes rather than the list's live cadence: these counts
/// move on the order of hours, and the query is only mounted while the
/// Stats view is open, so a shorter window would spend rate limit for no
/// visible change.
///
/// Restored with `usePeriods` above; see its doc for why the unscoped page
/// was not superseded by the scoped one.
export function useHistory(days: number) {
  return useQuery({
    queryKey: ["history", days],
    queryFn: () => getHistory(days),
    staleTime: 5 * 60 * 1000,
  });
}

/// The merged-PR sample behind the insight cards and repo table. Kept
/// separate from `useHistory` so a slow or failed detail fetch leaves the
/// chart and cards fully rendered.
///
/// This one is a SAMPLE -- the most recent 100 merged pull requests
/// (`github/query.rs:175-195`) -- which is why the unscoped page carries a
/// page-level "from a sample of recent merged pull requests" caveat that a
/// scope page does not need. The scope pages measure a whole window and say
/// so; this measures a fixed recent slice and says THAT. Two honest claims
/// about two different populations, which is the reason both pages exist.
export function useMergedDetail() {
  return useQuery({
    queryKey: ["merged-detail"],
    queryFn: getMergedDetail,
    staleTime: 5 * 60 * 1000,
  });
}

/// Median cycle time this week against last, for the unscoped page.
///
/// The Stats page could prove throughput but not improvement: cycle time
/// was a single window with no prior value, which is why its delta card
/// was hardcoded to null. Same 5-minute staleness as the other stats.
export function useCycleTrend() {
  return useQuery({
    queryKey: ["cycle-trend"],
    queryFn: getCycleTrend,
    staleTime: 5 * 60 * 1000,
  });
}

/// Build history. Failed builds are kept: a failing build is usually
/// what the user came to investigate.
export function useDockerBuilds(enabled: boolean) {
  return useQuery({
    queryKey: ["docker-builds"],
    queryFn: dockerBuilds,
    enabled,
    staleTime: 10_000,
  });
}


export function useDockerDiskUsage(enabled: boolean) {
  return useQuery({
    queryKey: ["docker-disk"],
    queryFn: dockerDiskUsage,
    enabled,
    staleTime: 10_000,
  });
}

export function useDockerVolumes(enabled: boolean) {
  return useQuery({
    queryKey: ["docker-volumes"],
    queryFn: dockerDanglingVolumes,
    enabled,
    staleTime: 10_000,
  });
}

/// Remove images, dropping the successful ones from the cache.
///
/// Filtered rather than invalidated, for the same reason worktree
/// removal is: re-resolving provenance means git calls per tag, and
/// removing an image cannot change any other image's standing.
export function useRemoveImages() {
  const qc = useQueryClient();
  return (ids: string[]) =>
    dockerRemoveImages(ids).then((outcomes) => {
      const gone = new Set(outcomes.filter((o) => o.error === null).map((o) => o.id));
      qc.setQueryData<DockerImage[]>(["docker-images"], (old) =>
        old?.filter((i) => !gone.has(i.id)),
      );
      void qc.invalidateQueries({ queryKey: ["docker-disk"] });
      return outcomes;
    });
}

export function useRemoveVolume() {
  const qc = useQueryClient();
  return (name: string) =>
    dockerRemoveVolume(name).then(() => {
      void qc.invalidateQueries({ queryKey: ["docker-volumes"] });
      void qc.invalidateQueries({ queryKey: ["docker-disk"] });
    });
}

export function usePruneCache() {
  const qc = useQueryClient();
  return (until?: string) =>
    dockerPruneCache(until).then((freed) => {
      void qc.invalidateQueries({ queryKey: ["docker-disk"] });
      return freed;
    });
}

/// Directories scanned for git checkouts, and a way to change them.
///
/// The mutation can FAIL -- a path that is not a directory is rejected by
/// the backend -- so this surfaces the error rather than swallowing it,
/// unlike the interval setting which only clamps.
export function useWorktreeDirs() {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ["worktree-dirs"],
    queryFn: getWorktreeDirs,
    staleTime: Infinity,
  });
  const set = (dirs: string[]) =>
    setWorktreeDirs(dirs).then((applied) => {
      qc.setQueryData(["worktree-dirs"], applied);
      return applied;
    });
  return { dirs: query.data ?? [], set };
}

/// The poll interval setting, and a way to change it.
///
/// The value is authoritative on the Rust side, which owns the running
/// loop -- the mutation returns what was actually applied after clamping,
/// so the UI can never show a value the backend rejected.
export function usePollInterval() {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ["poll-interval"],
    queryFn: getPollInterval,
    staleTime: Infinity,
  });
  const set = (secs: number) =>
    setPollInterval(secs).then((applied) => {
      qc.setQueryData(["poll-interval"], applied);
      return applied;
    });
  return { seconds: query.data, set };
}

/// Interface preferences.
export function useUiPrefs() {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ["ui-prefs"],
    queryFn: getUiPrefs,
    staleTime: Infinity,
  });
  const set = (prefs: UiPrefs) =>
    setUiPrefs(prefs).then(() => {
      qc.setQueryData(["ui-prefs"], prefs);
    });
  return { prefs: query.data, set };
}

/// Whether the app starts at login.
///
/// No optimistic seed: this one can genuinely FAIL -- registering a
/// launch agent touches the filesystem -- so the checkbox should reflect
/// what the OS actually did, not what was asked for.
export function useAutostart() {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ["autostart"],
    queryFn: getAutostart,
    staleTime: Infinity,
  });
  const set = (enabled: boolean) =>
    setAutostart(enabled).then(() =>
      qc.invalidateQueries({ queryKey: ["autostart"] }),
    );
  return { enabled: query.data ?? false, set };
}

/// Whether phones can connect right now.
///
/// Asked of the listener rather than stored, like autostart: the two
/// differ when the port could not be bound at startup, and the box
/// should say what is true. Refetched after EVERY attempt, failed or
/// not -- a start that bound the port but could not save the setting
/// is still a running listener, and the box must show it.
export function useRemoteEnabled() {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ["remote-enabled"],
    queryFn: getRemoteEnabled,
    staleTime: Infinity,
  });
  const set = (enabled: boolean) =>
    setRemoteEnabled(enabled).finally(() =>
      qc.invalidateQueries({ queryKey: ["remote-enabled"] }),
    );
  return { enabled: query.data ?? false, set };
}

// ---------------------------------------------------------------------
// The Claude Code hook installer (#915).
// Rust side: src-tauri/src/claude/install.rs
// ---------------------------------------------------------------------

/// Whether the hooks are in `~/.claude/settings.json`, plus the three
/// actions that change that.
///
/// # Why there is no `staleTime: Infinity` here
///
/// Every other settings query in this file caches forever, because it reads a
/// value only this app writes. This one reads a file that OTHER things edit:
/// the user's editor, another tool's installer, a `git checkout` of their
/// dotfiles. A cached "installed" is wrong the moment any of those happens,
/// which is the same staleness argument the schema makes for liveness -- and
/// the issue states it as a rule: never infer "installed" from "we wrote the
/// file", read it back.
///
/// So it refetches when the settings dialog is focused, and every action
/// invalidates rather than writing an optimistic value. An optimistic
/// "installed" would be precisely the lie the feature is built to avoid: it
/// would show a green tick for a write that Claude Code will silently ignore.
export function useClaudeHooks() {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ["claude-hooks-status"],
    queryFn: claudeHooksStatus,
    // Short rather than Infinity, and refetched on focus: see above.
    staleTime: 5_000,
    refetchOnWindowFocus: true,
  });
  // `invalidateQueries` and NOT `setQueryData`: the result of an install is
  // what the FILE says afterwards, not what we asked for.
  const reread = () => qc.invalidateQueries({ queryKey: ["claude-hooks-status"] });
  return {
    status: query.data,
    /// Distinct from `status === undefined` after an error: a caller must be
    /// able to tell "still loading" from "the call itself failed".
    isLoading: query.isLoading,
    error: query.error,
    install: () => claudeInstallHooks().finally(reread),
    reinstall: () => claudeReinstallHooks().finally(reread),
    uninstall: () => claudeUninstallHooks().finally(reread),
    /// Re-read the file, with nothing written (#961).
    ///
    /// Exported because the `cannot_tell` arm's own message ends "Fix the
    /// JSON by hand, then install" -- and Install is `disabled` in exactly
    /// that state, deliberately and correctly. So the instruction named a
    /// control that does not exist: every caller of `reread` was a
    /// `.finally` on one of the three buttons that are disabled or hidden
    /// there, leaving `refetchOnWindowFocus` as the only re-check. That is
    /// invisible, undiscoverable, and will not fire at all for a user who
    /// never leaves the window.
    ///
    /// The SAME function the three writes use, not a second read path: the
    /// answer to "did my hand-edit take" must be the same answer an
    /// install would have computed, or the button lies in a subtler way
    /// than having no button did.
    reread,
  };
}

// ---------------------------------------------------------------------
// Phone pairing. Rust side: src-tauri/src/remote/pairing.rs
// ---------------------------------------------------------------------

/// Settings > Paired devices.
export function usePairedDevices() {
  return useQuery({
    queryKey: ["paired-devices"],
    queryFn: listPairedDevices,
    staleTime: 30_000,
  });
}

/// Settings > Pair a phone. Turns the listener on first when it is
/// off: a QR code for a port nothing is bound to would send the phone
/// to "connection refused" with no way to tell why. The box on the same
/// panel is refreshed so it shows the switch that just flipped.
///
/// A refused start rejects BEFORE minting -- there is no point issuing
/// a token the phone cannot present.
export function useIssuePairingToken() {
  const qc = useQueryClient();
  return async (): Promise<PairingQrPayload> => {
    if (!(await getRemoteEnabled())) {
      try {
        await setRemoteEnabled(true);
      } finally {
        await qc.invalidateQueries({ queryKey: ["remote-enabled"] });
      }
    }
    return issuePairingToken();
  };
}

/// Answer a `pairing-request`. Rejects with the Rust side's message
/// when the name is already taken and `replaceExisting` was not given;
/// the request stays pending, so the caller asks and answers again.
export function useRespondToPairing() {
  const qc = useQueryClient();
  return async (requestId: number, approve: boolean, replaceExisting?: boolean) => {
    await respondToPairing(requestId, approve, replaceExisting);
    if (approve) await qc.invalidateQueries({ queryKey: ["paired-devices"] });
  };
}

/// Revoke one phone. The list is refreshed whichever way the call
/// ends: a revoke that failed half-way still changed what is stored.
export function useRevokePairedDevice() {
  const qc = useQueryClient();
  return async (id: number) => {
    try {
      await revokePairedDevice(id);
    } finally {
      await qc.invalidateQueries({ queryKey: ["paired-devices"] });
    }
  };
}

/// The phone waiting on the user's decision, or null.
///
/// A queue rather than "latest wins": two phones scanning in quick
/// succession must each get a decision, and swapping the modal's
/// contents while the user is comparing fingerprints is exactly the
/// confusion the modal exists to prevent. `dismiss` moves to the next.
export function usePairingRequest(): { request: PairingRequest | null; dismiss: () => void } {
  const [queue, setQueue] = useState<PairingRequest[]>([]);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<PairingRequest>("pairing-request", (e) => {
      // One entry per request id. A duplicate delivery would otherwise
      // re-show a request already answered, with a dialog whose Approve
      // can only fail -- seen in dev, where StrictMode's double-mount
      // met a listener that had not really been unlistened.
      setQueue((q) =>
        q.some((r) => r.request_id === e.payload.request_id) ? q : [...q, e.payload],
      );
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      if (unlisten) safeUnlisten(unlisten);
    };
  }, []);

  return {
    request: queue[0] ?? null,
    dismiss: () => setQueue((q) => q.slice(1)),
  };
}

/// How far a bulk worktree removal has got, or null when idle.
///
/// The button previously showed a single boolean for what can be ~30
/// seconds of sequential deletion, so a long batch was
/// indistinguishable from a hang. The Rust side emits (done, total)
/// after EACH removal -- including failures, or a batch where several
/// fail appears to stall.
export function useRemovalProgress(): { done: number; total: number } | null {
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<[number, number]>("worktree-removal-progress", (e) => {
      const [done, total] = e.payload;
      // Clears on the last one rather than leaving "106 of 106" on
      // screen after the work is over.
      setProgress(done >= total ? null : { done, total });
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, []);

  return progress;
}

/// How far a background package-update run has got, or null when none
/// is running.
///
/// The same shape as `useRemovalProgress`, and for the same reason: a
/// run is one package-manager invocation per package, so a selection of
/// 122 sits for minutes. `apply_updates_in_background` returns
/// immediately, and until #626 the ONLY event it ever emitted was the
/// terminal one -- so the wizard closed and nothing said anything until
/// the whole run finished.
///
/// It matters most on the phone. A desktop user can leave the window
/// open; a phone that has been backgrounded holds no event stream at
/// all (`src-mobile/src/background.rs` is explicit that a suspended app
/// gets no `/v1/events`), so what it can show is what arrives while it
/// is in the foreground.
export function useUpdateProgress(): { done: number; total: number } | null {
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<[number, number]>("update-run-progress", (e) => {
      const [done, total] = e.payload;
      // Cleared on the last one rather than leaving "47 of 47" up after
      // the work is over; `update-run-done` carries the outcome.
      setProgress(done >= total ? null : { done, total });
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, []);

  return progress;
}

/// What one worktree is holding, fetched on demand.
///
/// `enabled` so it costs nothing until a row is actually opened:
/// several git calls per worktree, and there can be hundreds of rows.
/// A long `staleTime` because the answer only changes when the branch
/// does, and the row is already keyed by path.
export function useAssessment(
  repoPath: string | null,
  worktreePath: string | null,
  branch: string | null,
) {
  return useQuery({
    queryKey: ["assessment", repoPath, worktreePath],
    queryFn: () => assessWorktree(repoPath!, worktreePath!, branch!),
    enabled: repoPath !== null && worktreePath !== null && branch !== null,
    staleTime: 5 * 60 * 1000,
  });
}

/// Which desktop notifications the user wants.
///
/// Seeds optimistically on write, like `usePollInterval`: the Rust side
/// is the source of truth, but a checkbox that waits for a round-trip to
/// tick feels broken.
export function useNotifyPrefs() {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ["notify-prefs"],
    queryFn: getNotifyPrefs,
    staleTime: Infinity,
  });
  const set = (prefs: NotifyPrefs) =>
    setNotifyPrefs(prefs).then(() => {
      qc.setQueryData(["notify-prefs"], prefs);
    });
  return { prefs: query.data, set };
}

/// PRs awaiting the user's review.
///
/// The app previously queried only `author:@me`, so it could say nothing
/// about the queue the user is the bottleneck for -- the largest gap for a
/// daily driver. Same 60s staleness as the authored list.
export function useReviewing(enabled = true) {
  // The cached list, read from SQLite and never from GitHub. Its own
  // query so it resolves in milliseconds while the live one runs --
  // folding the cache into the live queryFn instead would let a cached
  // result satisfy `staleTime` and leave the list permanently stale.
  //
  // The cache is still worth having even now that the live query is
  // fast: it paints in milliseconds where a network round-trip cannot.
  //
  // CORRECTION to what this comment used to say. It claimed "the live
  // query cannot be made meaningfully faster -- a bare 25-item search
  // already costs 6.2s". That measurement was WRONG, and it sat here as
  // a reason not to try. A bare 25-item search costs ~0.7s; the 6.2s was
  // the FIELDS, and specifically `mergeStateStatus` at ~154ms per pull
  // request. #328 pages at 25 concurrently now, which measured 62 pull
  // requests in ~7s against ~21s-then-truncate.
  const cached = useQuery({
    queryKey: ["reviewing-cached"],
    queryFn: CACHED_REVIEWING_FN,
    enabled,
    // Read once per mount. The live query is what keeps the view
    // current; re-reading the cache would only ever show older data.
    staleTime: Infinity,
    gcTime: Infinity,
  });

  const live = useQuery({
    queryKey: ["reviewing"],
    queryFn: REVIEWING_FN,
    // Only the view that RENDERS these pull requests fetches them. It
    // used to run on every view -- including Docker and Worktrees, which
    // show none -- purely so a sidebar badge could display its length.
    // That is a 100-node query for a number, and on a slow account it
    // failed there too.
    enabled,
    staleTime: 60_000,
  });

  // Live data the moment it exists; the cache only until then. Note
  // `live.data` is checked rather than `live.isSuccess`, so a refetch
  // still in flight keeps showing the previous LIVE list rather than
  // falling back to a staler cached one.
  //
  // The cache now arrives as `{prs, stale_secs}` rather than a bare
  // array (#742). It used to age itself out by returning NOTHING, which
  // reached here as `[]` -- and `[] ?? x` is `[]`, so an aged-out cache
  // beat the loading state and the view rendered a confident "nothing
  // awaits your review" until the live fetch landed. On the account
  // that reported this, that was seventeen seconds.
  const data = live.data ?? cached.data?.prs;

  // Only meaningful while the CACHE is what is on screen: once live data
  // arrives it is current by definition, whatever the disk said.
  const staleSecs = live.data === undefined ? (cached.data?.stale_secs ?? null) : null;

  return {
    ...live,
    data,
    // Loading only when there is genuinely nothing to show. With a warm
    // cache the panel paints immediately, which is the whole point --
    // the reported complaint was an empty view for over a minute.
    isLoading: data === undefined && (live.isLoading || cached.isLoading),
    // True while the live query runs, INCLUDING when the cache is
    // already painted. This drives the "refreshing" indicator, which is
    // the other half of the complaint: "no indication that it is
    // blocked".
    isRefreshing: live.isFetching,
    /// Whether what is on screen came from disk rather than GitHub.
    isFromCache: live.data === undefined && cached.data !== undefined,
    /// How old the shown rows are, when they are too old to present as
    /// current. `null` means either fresh or live -- no marker needed.
    staleSecs,
  };
}


/// How many pull requests await the user's review.
///
/// The badge's own query, so it does not depend on the list being
/// fetched. MEASURED: 1 rate-limit point and ~0.9s, against 6 and ~4s
/// for the list it replaces here.
export function useReviewingCount() {
  return useQuery({
    queryKey: ["reviewing-count"],
    queryFn: REVIEWING_COUNT_FN,
    staleTime: 60_000,
  });
}

/// How many fields GitHub refused on the last poll, or 0.
///
/// Advisory, like `useTruncation`. GitHub answered with usable data and
/// a complaint that it could not compute all of it; the list is real but
/// short, and saying so beats either hiding it or -- as v3.2.5 did --
/// discarding the data and showing nothing.
export function useIncomplete(): number {
  const [refused, setRefused] = useState(0);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<number>("prs-incomplete", (e) => setRefused(e.payload)).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      // Same tolerance as truncation: an advisory notice must not break
      // the page when the event bridge is absent.
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, []);

  return refused;
}

/// How many pull requests the review list is MISSING, or 0.
///
/// The 100 -> 50 fallback returns a short list and everything
/// downstream presented it as complete. The v3.5.3 diagnostic log
/// caught the consequence on a real machine: 50 pull requests shown
/// against a count of 62, with twelve gone and nothing to say so. That
/// is almost certainly the "numbers are off" report -- the sidebar
/// badge and the panel come from different queries, and only one of
/// them got truncated.
///
/// Advisory, like `useTruncation` and `useIncomplete`: the pull
/// requests that arrived are real, so the list is shown and annotated
/// rather than replaced with an error.
export function useReviewShortfall(): number {
  const [short, setShort] = useState(0);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<number>("reviewing-short", (e) => setShort(e.payload)).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      // Same tolerance as the other advisories: a notice must not break
      // the page when the event bridge is absent.
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, []);

  return short;
}

/// GitHub's true open-PR count, when it exceeds what the poll fetched.
///
/// `null` until the first poll reports, then GitHub's count while the
/// list is short and `0` once it is complete. The zero matters: the loop
/// emits on every tick precisely so a recovered poll can take the notice
/// back, and holding the last non-zero value left "showing 8 of 29" over
/// a complete list until relaunch (#745).
export function useTruncation(): number | null {
  const [total, setTotal] = useState<number | null>(null);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    // Tolerate a host without Tauri's event bridge (tests that do not opt
    // into mocked events, and any non-Tauri render). Truncation is an
    // advisory notice; failing to subscribe must not break the page.
    listen<number>("prs-truncated", (e) => setTotal(e.payload)).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, []);

  return total;
}

/// `usePeriods`, `useHistory`, `useMergedDetail` and `useCycleTrend` WERE
/// here, and #826 removed all four.
///
/// Each wrapped a command hardcoded to `author:@me` -- `get_periods`,
/// `get_history`, `get_merged_detail`, `get_cycle_trend` -- which is the one
/// thing #823 named as blocking its second audience: there was no way to ask
/// about anybody else. `StatsPage` now asks through the parameterised layer
/// (`useScopedCounts`, `useStatsSeries`, `useStatsBoard`), which answers the
/// same questions for any scope and carries the completeness facts the old
/// commands had no field for.
///
/// Removed rather than left unused, because an unused hook is one a future
/// page can reach for and quietly get the viewer's own account back under a
/// scope heading -- which would be wrong in a way nothing on screen would
/// contradict.
///
/// The Rust commands remain registered and on both remote surfaces. They are
/// not dead: `transport.test.ts` exercises the wrappers, and the phone's
/// `remote_call` can still reach them. Retiring the commands themselves is a
/// separate change with its own surface-guard implications, and this PR is
/// already the largest of the three.

/// Branches for one repository, fetched only when it is selected.
///
/// Measured at ~9s on a 675-branch repository, most of it the patch-id
/// comparison that finds squash merges. Scanning every repository up
/// front is not an option, and `staleTime` is deliberately short: the
/// answer is about deletability, and a stale "safe to delete" is the
/// one wrong answer that costs work.
export function useBranches(repoPath: string | undefined) {
  return useQuery({
    queryKey: ["branches", repoPath],
    queryFn: () => listBranches(repoPath!),
    enabled: !!repoPath,
    // 10 seconds, deliberately, and not to be lengthened. This page
    // decides whether a branch is safe to DELETE, and a stale "yes" is
    // the expensive mistake. Streaming (#657) changes what the page can
    // show DURING a scan; it does not change how long an answer stays
    // trusted after one.
    staleTime: 10_000,
  });
}

/// What a running branch scan has reported so far.
///
/// `total` is `null` until the scan says how many branches it is about
/// to classify. Once it has, `classified` climbing towards `total` is
/// the page's evidence that the stream is alive — and a `classified`
/// that stops short of `total` is the page's evidence that it is not.
export interface BranchScanState {
  /// Every row, in final order, verdicts filling in as they arrive.
  branches: Branch[];
  /// How many the scan said it would classify, or `null` before it did.
  total: number | null;
  /// How many verdicts have arrived.
  classified: number;
}

const IDLE_SCAN: BranchScanState = { branches: [], total: null, classified: 0 };

/// Branch rows as the desktop classifies them, for the cold visit.
///
/// # Why this exists next to `useBranches` rather than inside it
///
/// `useBranches` is the AUTHORITY: its resolved value is the completed
/// scan, and it is what the delete controls read. This hook is a view
/// of the same work in flight. Keeping them separate is what stops
/// streamed data becoming the basis of a deletion — the frames land
/// here, never in the query cache, so `delete_local`'s re-check against
/// an uncached `scan` remains the only thing a deletion is gated on.
///
/// The cold visit is the case it exists for. #682's cache fixed the
/// repeat visit and structurally cannot fix the first one: there is
/// nothing to serve. The desktop already does this work per branch and
/// in parallel, so every verdict is complete the moment its thread
/// finishes; before this they simply all waited for the slowest.
///
/// Frames for another repository are dropped. The event is app-global
/// and a scan is per-repository, so without the check a page that
/// switched repositories mid-scan would fold the old one's verdicts
/// into the new one's rows.
export function useBranchScan(repoPath: string | undefined): BranchScanState {
  // The repository is held IN the state, not merely in the effect's
  // dependency list. A new repository must start from nothing, and
  // resetting from inside the effect would be a synchronous `setState`
  // during an effect -- a cascading render, and one that paints the
  // previous repository's rows for a frame first. Held here, the reset
  // is a comparison during render instead.
  const [held, setState] = useState<BranchScanState & { repo?: string }>(IDLE_SCAN);
  const state = held.repo === repoPath ? held : IDLE_SCAN;

  useEffect(() => {
    if (!repoPath) return;

    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<BranchScanFrame>("branch-scan-progress", (e) => {
      const f = e.payload;
      if (f.repo !== repoPath) return;
      setState((was) => {
        // Frames for the repository we are no longer showing cannot
        // reach here (the guard above), but a frame arriving while the
        // held state still describes the PREVIOUS one must build on
        // nothing rather than on those rows.
        const prev = was.repo === repoPath ? was : IDLE_SCAN;
        if (f.kind === "listed") {
          // A fresh listing REPLACES rather than merges: it is the
          // start of a new scan, and carrying verdicts over from the
          // previous one would show answers computed against refs that
          // have since moved.
          return { repo: repoPath, branches: f.branches, total: f.total, classified: 0 };
        }
        const byName = new Map(f.verdicts);
        // Counted from the rows actually updated, not from
        // `verdicts.length`. A frame that arrives before its listing —
        // or names a branch the listing did not — must not advance a
        // count the user reads as "this many rows are answered".
        let landed = 0;
        const branches = prev.branches.map((b) => {
          const d = byName.get(b.name);
          if (d === undefined || b.deletable.kind !== "pending") return b;
          landed += 1;
          return { ...b, deletable: d };
        });
        return { ...prev, repo: repoPath, branches, classified: prev.classified + landed };
      });
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
      unlisten = undefined;
    };
  }, [repoPath]);

  // Narrowed on the way out: `repo` is bookkeeping for the reset above,
  // not something a caller should read or compare against.
  return useMemo(
    () => ({
      branches: state.branches,
      total: state.total,
      classified: state.classified,
    }),
    [state.branches, state.total, state.classified],
  );
}

/// What a running branch deletion has reported so far, or `null` when
/// none is running.
///
/// The phase is the point. A deletion re-checks every branch against a
/// fresh scan before touching anything, and that scan is the slow half
/// — so a single counter would report 0 of 562 throughout the part
/// that takes the minutes (#724). `checking` and `deleting` are
/// separate states so the page can say which wait the user is in.
export type BranchDeleteState =
  | { phase: "checking"; done: number; total: number }
  | { phase: "deleting"; done: number; total: number; failed: number };

/// How far a bulk branch deletion has got, or `null` when idle.
///
/// # Why this is not `useRemovalProgress` with different words
///
/// The worktree removal is N sequential deletions and one counter
/// describes it honestly. A branch deletion is not: it opens with a
/// full uncached scan of the repository — the safety gate, ~64ms per
/// branch — and only then starts deleting. Reported as one number that
/// gate would hold the count at zero for minutes, which is exactly the
/// "ran for ten minutes with no progress" that was reported. Two
/// phases, because there are two.
///
/// Never inferred from whether frames are still arriving: a dead run
/// stops sending too, so traffic cannot distinguish a stall from an
/// ending. This clears when the deleting phase reaches its total, and
/// the caller clears it on settle — the promise resolving is the only
/// thing that actually proves the run is over.
///
/// Frames for another repository are dropped, the same reason
/// `useBranchScan` drops them: the event is app-global, the work is
/// per-repository.
export function useBranchDeleteProgress(
  repoPath: string | undefined,
): BranchDeleteState | null {
  const [state, setState] = useState<(BranchDeleteState & { repo?: string }) | null>(null);

  useEffect(() => {
    if (!repoPath) return;

    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<BranchDeleteFrame>("branch-delete-progress", (e) => {
      const f = e.payload;
      if (f.repo !== repoPath) return;
      if (f.kind === "checking") {
        setState({ repo: repoPath, phase: "checking", done: f.done, total: f.total });
        return;
      }
      // Cleared on the last one rather than leaving "562 of 562" up
      // after the work is over — the same rule `useRemovalProgress`
      // follows. The failure count is not lost with it: the outcomes
      // arrive with the promise and are reported per branch.
      setState(
        f.done >= f.total
          ? null
          : {
              repo: repoPath,
              phase: "deleting",
              done: f.done,
              total: f.total,
              failed: f.failed,
            },
      );
    }).then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
      unlisten = undefined;
    };
  }, [repoPath]);

  // A frame that arrived for the previous repository must not describe
  // this one. Compared during render rather than reset in an effect,
  // which would paint the stale phase for a frame first.
  const live = state !== null && state.repo === repoPath ? state : null;

  // Narrowed on the way out: `repo` is bookkeeping for the reset above,
  // not something a caller should read or compare against — the same
  // shape `useBranchScan` returns.
  return useMemo(() => {
    if (live === null) return null;
    return live.phase === "checking"
      ? { phase: "checking" as const, done: live.done, total: live.total }
      : {
          phase: "deleting" as const,
          done: live.done,
          total: live.total,
          failed: live.failed,
        };
  }, [live]);
}

/// Surface the outcome of a background update run.
///
/// The run continues regardless of what is on screen (#495), so the
/// result has to find the user rather than the other way round. A
/// pull request that actually exists gets a toast whose action opens
/// it in My pull requests; a run that stopped at the worktree says so
/// without claiming one is coming.
/// Report a finished run, however it was learned about.
///
/// Shared by the `update-run-done` listener and by the resume read that
/// asks how a run ended after a suspension -- the two describe the same
/// event and must say the same thing. A phone that slept through a run
/// gets this from `update_run_state` instead of from the stream it was
/// not holding.
function reportRunOutcome(
  d: UpdateRunDone,
  qc: QueryClient,
  setView: (v: View) => void,
  selectPr: (pr: { repo: string; number: number }) => void,
): void {
  // Refresh whatever the run changed, whichever way it ended.
  void qc.invalidateQueries({ queryKey: ["packages"] });
  void qc.invalidateQueries({ queryKey: ["worktrees"] });

  // Stopped by the user: neither a success nor a failure, and saying
  // either would be wrong. The packages that landed before the stop
  // really did land, and the worktree holding them still exists.
  if (d.cancelled) {
    toast.info("Update run stopped", {
      description:
        d.applied === 0
          ? "Nothing had been applied yet."
          : `${d.applied} package${d.applied === 1 ? "" : "s"} were applied before it stopped, in ${d.branch ?? "the worktree"}.`,
    });
    return;
  }

  if (d.url !== null) {
    const pr = prFromUrl(d.url);
    toast.success("Package update pull request is ready", {
      description:
        d.failed === 0
          ? `${d.applied} package${d.applied === 1 ? "" : "s"} updated.`
          : `${d.applied} updated, ${d.failed} could not be.`,
      // Only offered when the URL parses. A button that silently
      // does nothing is worse than no button.
      action: pr
        ? {
            label: "Open",
            onClick: () => {
              setView("my-prs");
              selectPr(pr);
            },
          }
        : undefined,
    });
    return;
  }

  // No pull request. Never phrased as though one is on its way.
  toast.warning("Updates applied, but no pull request was opened", {
    description: d.error ?? undefined,
  });
}

export function useUpdateRunOutcome(): void {
  const setView = useFilters((s) => s.setView);
  const selectPr = useFilters((s) => s.selectPr);
  const qc = useQueryClient();

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    // GUARDED. `listen` reaches into the Tauri runtime and throws
    // outside it, and this hook runs at the app level rather than
    // behind a view -- so without this every App test would have to
    // mock it, which is a mock that exists only to stop a crash.
    let started: Promise<UnlistenFn>;
    try {
      started = listen<UpdateRunDone>("update-run-done", (e) => {
        reportRunOutcome(e.payload, qc, setView, selectPr);
      });
    } catch {
      return;
    }
    started.then(
      (fn) => {
        if (cancelled) safeUnlisten(fn);
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      if (unlisten) safeUnlisten(unlisten);
    };
  }, [setView, selectPr, qc]);
}

/// Stop a background update run.
///
/// It stops after the package it is on, so this resolving means "asked
/// to stop", not "stopped". The terminal `update-run-done` -- carrying
/// `cancelled: true` -- is what says it actually has.
export function useCancelUpdateRun(): (repoPath: string) => Promise<void> {
  return useCallback((repoPath: string) => cancelUpdateRun(repoPath), []);
}

/// Catch up on a run that ended while nobody was listening.
///
/// `useUpdateRunOutcome` hears the terminal event -- if the app is
/// there to hear it. A phone is often not: `src-mobile/src/background.rs`
/// is explicit that a suspended app holds no `/v1/events` stream, so a
/// run started and then backgrounded delivers its progress and its
/// outcome to nobody. Without this a phone could start a run and never
/// learn how it ended, which is most of why #626 held the action back
/// from the phone in the first place.
///
/// Asks on mount and on every return to the foreground, for the repo
/// currently selected. Reports a finished run once: `seen` keeps a
/// second foreground from re-toasting an outcome already shown, which
/// on a phone would otherwise fire on every app switch.
export function useUpdateRunResume(repoPath: string | undefined): void {
  const setView = useFilters((s) => s.setView);
  const selectPr = useFilters((s) => s.selectPr);
  const qc = useQueryClient();

  useEffect(() => {
    if (repoPath === undefined || repoPath === "") return;
    let live = true;
    // Outcomes already reported, so returning to the foreground twice
    // does not announce the same run twice. Keyed by branch and counts
    // rather than identity: the registry holds one outcome per repo,
    // and a NEW run in the same repo has different ones.
    let seen: string | null = null;

    const ask = () => {
      updateRunState(repoPath).then(
        (state) => {
          if (!live || state === null || state.state !== "done") return;
          const key = `${state.outcome.branch ?? ""}:${state.outcome.applied}:${state.outcome.failed}:${String(state.outcome.cancelled)}`;
          if (seen === key) return;
          seen = key;
          reportRunOutcome(state.outcome, qc, setView, selectPr);
        },
        // A desktop that predates the command, or no runtime at all.
        // The live event is the primary path; this is the backstop.
        () => {},
      );
    };

    ask();
    const onVisible = () => {
      if (document.visibilityState === "visible") ask();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      live = false;
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [repoPath, qc, setView, selectPr]);
}

/// `owner/repo` and number from a pull request URL.
///
/// Returns null rather than guessing: the toast's action is only
/// offered when this parses, so a URL shape we do not recognise costs
/// a button rather than producing one that goes nowhere.
export function prFromUrl(url: string): { repo: string; number: number } | null {
  const m = /github\.com\/([^/]+\/[^/]+)\/pull\/(\d+)/.exec(url);
  if (!m) return null;
  const number = Number(m[2]);
  return Number.isFinite(number) ? { repo: m[1], number } : null;
}

/// How often the System Health view re-reads the current sample.
///
/// Five seconds, not one: what this drives is load averages and memory
/// pressure, neither of which changes meaningfully faster, and each
/// call does real work on a blocking thread reading the kernel.
const HEALTH_POLL_MS = 5_000;

/// The machine's health right now, polled only while the view is open.
///
/// `enabled` is the whole point of the signature. This is the one query
/// in the app that would otherwise keep sampling the CPU for a page
/// nobody is looking at, which the issue explicitly rules out. A
/// disabled query has its `refetchInterval` suspended by TanStack, so
/// passing `false` genuinely stops the timer rather than merely
/// discarding the result.
///
/// `refetchIntervalInBackground` is left at its default of false on
/// purpose: a minimised window stops polling too, and the Rust
/// collector's own once-a-minute sampler is what keeps the history
/// complete while nobody is watching.
export function useSystemHealth(enabled: boolean) {
  return useQuery<HealthSample>({
    queryKey: ["system-health"],
    queryFn: systemHealth,
    enabled,
    refetchInterval: enabled ? HEALTH_POLL_MS : false,
    // Just under the poll interval, so a remount within one tick paints
    // from cache instead of firing an extra sample.
    staleTime: HEALTH_POLL_MS - 1_000,
  });
}

/// The last 24 hours, for the charts.
///
/// Refetched far more slowly than the live sample: the series gains at
/// most one point a minute, so anything faster re-fetches 120 samples
/// to draw the identical line. On the phone that payload crosses the
/// LAN, which is the cost #661 was about.
export function useSystemHealthHistory(enabled: boolean) {
  return useQuery<HealthSample[]>({
    queryKey: ["system-health-history"],
    queryFn: systemHealthHistory,
    enabled,
    refetchInterval: enabled ? 60_000 : false,
    staleTime: 30_000,
  });
}

/// Every health condition true right now, for the page to show (#864).
///
/// The CPU runaway rules had no path to a screen before this hook: they
/// were evaluated on every poll and by `health_alerts`, and nothing in
/// the frontend called that command, so a 12-process runaway that ran
/// 8.5 hours produced no visible output. A rule that evaluates into
/// nothing is indistinguishable from no rule.
///
/// Cadenced with `useSystemHealthHistory` rather than the live sample
/// because it is DERIVED from that same stored series: the rules read
/// history, so asking faster than the series grows re-derives an
/// identical answer. The one exception is the aggregate CPU rule's
/// process-table read, which is why this is a command rather than
/// client-side arithmetic over the samples the charts already have.
export function useHealthAlerts(enabled: boolean) {
  return useQuery<AlertReport[]>({
    queryKey: ["health-alerts"],
    queryFn: healthAlerts,
    enabled,
    refetchInterval: enabled ? 60_000 : false,
    staleTime: 30_000,
  });
}

/// What is using this machine, polled with the rest of the view.
///
/// Read by the System Health CPU and Memory detail pages, which show the
/// machine's top processes. Named `footprint` after the "What Headstate
/// is costing" panel it once fed; #795 removed that panel and the
/// command kept its name -- see `systemFootprint` in `api/tauri.ts`.
///
/// Shares `HEALTH_POLL_MS` with `useSystemHealth` deliberately. The two
/// answer halves of one question -- "is the machine busy" and "what is
/// making it busy" -- and a reader compares them directly: the CPU page
/// puts a percentage above the process list that explains it. On
/// different cadences that pair would describe two different instants,
/// so a process could appear at 400% beside a CPU figure taken before it
/// started.
///
/// Safe on that cadence because the Rust side is a kernel read of the
/// already-open process table: no subprocess, no directory walk. No
/// disk-sizing hook shares this timer and none may -- `size_worktrees`
/// and its three siblings take seconds to tens of seconds, and the whole
/// point of #661 is that a slow command on a timer is the failure. They
/// are driven from explicit actions on the Worktrees, Artifacts and
/// Docker pages instead.
///
/// `enabled` for the same reason as its two neighbours: a disabled
/// query has its `refetchInterval` suspended by TanStack, so passing
/// `false` genuinely stops the timer rather than discarding results.
export function useSystemFootprint(enabled: boolean) {
  return useQuery<Footprint>({
    queryKey: ["system-footprint"],
    queryFn: systemFootprint,
    enabled,
    refetchInterval: enabled ? HEALTH_POLL_MS : false,
    staleTime: HEALTH_POLL_MS - 1_000,
  });
}

/// How long ONE per-process network reading takes, on macOS.
///
/// Not a guess and not a timeout: `nettop` samples for a full interval
/// before it prints anything, and `-L 1` waits that interval out.
/// Measured at 5.06-5.25s across every flag combination that might have
/// shortened it (`-s 1`, `-d`), against 0.08s of CPU -- it is a sleep,
/// not work.
///
/// Exported for the view, which needs it to tell the user how long the
/// wait will be. A page that says "about five seconds" while the code
/// believes something else is a page that will eventually lie, so the
/// number the UI quotes and the number this cadence is built on are one
/// constant.
export const NET_PROCESSES_SAMPLE_MS = 5_000;

/// How often the Network page re-reads which processes are using the
/// network (#718).
///
/// # Not `HEALTH_POLL_MS`, and this is the whole design decision
///
/// The shared health poll is five seconds. One reading here takes ~5.1
/// seconds, so on that timer every call would OUTLIVE the interval that
/// spawned it: `nettop` processes would overlap continuously and the
/// machine would permanently host one or more of them for as long as
/// the app was open. That is #661's rule -- a slow command on a shared
/// timer is the failure -- met in its worst available form, so this
/// reading gets its own command and its own cadence.
///
/// # Why fifteen seconds
///
/// It has to be comfortably more than one reading takes, so at most one
/// `nettop` is ever alive: 15s leaves ~10s of quiet between readings,
/// which absorbs a slow machine without the two ever overlapping. And
/// it cannot be much longer, because the rates on this panel come from
/// DIFFERENCING consecutive readings -- a 60s cadence would average
/// every burst over a minute and show a link that spiked as a link that
/// hummed.
///
/// The duty cycle that buys: about a third of wall-clock time with a
/// `nettop` running, but at 0.08s of CPU per reading it is ~0.5% of one
/// core -- and only while somebody is looking at this one page.
///
/// # `enabled` is what makes that acceptable
///
/// This is the most expensive reading in the app that is on any timer
/// at all, so the timer must not exist unless the page is open. The
/// Network detail component is the only caller and passes `true` only
/// while it is mounted; TanStack genuinely suspends the
/// `refetchInterval` of a disabled query rather than merely discarding
/// its results, so navigating away stops the subprocess rather than
/// hiding it.
///
/// `refetchIntervalInBackground` stays at its default of false: a
/// minimised window must not keep spawning these, and unlike the health
/// sampler there is no history being filled in the meantime, so nothing
/// is lost by stopping.
export const NET_PROCESSES_POLL_MS = 15_000;

/// Which processes are using the network, on the Network page's own
/// cadence (#718).
///
/// # The two honesty problems this hook's shape exists to serve
///
/// 1. **The first reading takes ~5 seconds to arrive.** `isPending` is
///    therefore a five-second state, not a flicker, and the view is
///    required to explain it rather than spin. `NET_PROCESSES_SAMPLE_MS`
///    is exported so it can quote the real number.
/// 2. **One reading is not a rate.** The counts are cumulative since
///    each process started, so a rate needs two readings differenced --
///    putting the first rate ~20s after the page opens (one 5s reading,
///    a 15s cadence, a second 5s reading). The view says so.
///
/// `keepPreviousData` is deliberately NOT used, and `staleTime` is set
/// so a remount inside one cadence paints from cache instead of firing
/// another five-second subprocess.
export function useNetworkProcesses(enabled: boolean) {
  return useQuery<NetProcess[]>({
    queryKey: ["system-network-processes"],
    queryFn: systemNetworkProcesses,
    enabled,
    refetchInterval: enabled ? NET_PROCESSES_POLL_MS : false,
    // Just under the cadence, for the same reason as the health poll --
    // and it matters more here, because the extra call it prevents is
    // a five-second subprocess rather than a kernel read.
    staleTime: NET_PROCESSES_POLL_MS - 1_000,
    // One reading is expensive enough that retrying a failure three
    // times would cost fifteen seconds of subprocesses to reach the
    // same error message. The cadence itself is the retry.
    retry: false,
  });
}
