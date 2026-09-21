import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CleanupPrefs,
  ProjectReport,
  UpdateFilter,
  UpdateRequest,
} from "../types/pr";

/// The local transport is the ONLY thing mocked. Everything above it --
/// `transport.ts`, `tauri.ts`, the hooks -- is real, so what the mock
/// records is exactly what would reach `@tauri-apps/api` on the desktop.
const local = vi.hoisted(() => ({
  call: vi.fn<(name: string, args?: Record<string, unknown>) => Promise<unknown>>(() =>
    Promise.resolve(undefined),
  ),
  listen: vi.fn<(event: string, cb: unknown) => Promise<() => void>>(() =>
    Promise.resolve(() => {}),
  ),
}));
vi.mock("./local", () => ({ local }));

/// The remote transport, mocked the same way, so the mobile selection
/// can be asserted without a companion process.
const remote = vi.hoisted(() => ({
  call: vi.fn<(name: string, args?: Record<string, unknown>) => Promise<unknown>>(() =>
    Promise.resolve("from the phone"),
  ),
  listen: vi.fn<(event: string, cb: unknown) => Promise<() => void>>(() =>
    Promise.resolve(() => {}),
  ),
}));
vi.mock("./remote", () => ({ remote }));

import * as api from "./tauri";
import * as hooks from "./hooks";
import { call, listen } from "./transport";
import type { NotifyPrefs, UiPrefs } from "./tauri";

beforeEach(() => {
  local.call.mockClear();
  local.listen.mockClear();
});

// Sample arguments. Values are arbitrary; what matters is that each one
// arrives under the SAME key the Rust command expects, which is the
// shorthand-property key in `tauri.ts`.
const id = "PR_kwDOAbCdEf";
const repo = "octocat/hello-world";
const number = 7;
const runId = 42;
const threadId = "PRRT_kwDOAbCdEf";
const refId = "REF_kwDOAbCdEf";
const branch = "feature/spoon";
const merged = true;
const expectedHead = "0123abcd";
const enable = true;
const enabled = true;
const needs = true;
const secs = 60;
const days = 30;
const path = "/home/octocat/code/hello-world";
const repoPath = "/home/octocat/code/hello-world";
const worktreePath = "/home/octocat/code/hello-world/.worktrees/feature-spoon";
const worktreePaths = [worktreePath];
const paths = [path];
const ids = ["sha256:abc"];
const names = ["feature/spoon"];
const dirs = ["/home/octocat/code"];
const name = "hello-world_data";
const until = "24h";
const body = "Looks good.";
const verdict = "approve" as const;
const action = "merge" as const;
const prs: [string, string, number][] = [[id, repo, number]];
const requestId = 11;
const approve = true;
const replaceExisting = false;
const deviceId = 3;
const uiPrefs = { hidden_views: [] } as unknown as UiPrefs;
const notifyPrefs = { enabled: true } as unknown as NotifyPrefs;
const cleanupPrefs = { enabled: false } as unknown as CleanupPrefs;
const reports = [] as ProjectReport[];
const requests = [] as UpdateRequest[];
const filter = {} as unknown as UpdateFilter;

type Wrapper = (...a: never[]) => Promise<unknown>;
interface Row {
  fn: Wrapper;
  args: unknown[];
  command: string;
  expected: Record<string, unknown> | undefined;
}
const row = (
  fn: Wrapper,
  args: unknown[],
  command: string,
  expected?: Record<string, unknown>,
): Row => ({ fn, args, command, expected });

/// One row per exported wrapper: the command name and argument object
/// each one sent to `invoke` before the seam existed.
const ROWS: Row[] = [
  row(api.getCached, [], "get_cached"),
  row(api.refreshNow, [], "refresh_now"),
  row(api.getUiPrefs, [], "get_ui_prefs"),
  row(api.setUiPrefs, [uiPrefs], "set_ui_prefs", { prefs: uiPrefs }),
  row(api.getAutostart, [], "get_autostart"),
  row(api.setAutostart, [enabled], "set_autostart", { enabled }),
  row(api.getRemoteEnabled, [], "get_remote_enabled"),
  row(api.setRemoteEnabled, [enabled], "set_remote_enabled", { enabled }),
  row(api.assessWorktree, [repoPath, worktreePath, branch], "assess_worktree", { repoPath, worktreePath, branch }),
  row(api.getNotifyPrefs, [], "get_notify_prefs"),
  row(api.setNotifyPrefs, [notifyPrefs], "set_notify_prefs", { prefs: notifyPrefs }),
  row(api.rerunChecks, [repo, number, runId], "rerun_checks", { repo, number, runId }),
  row(api.buildTarget, [], "build_target"),
  row(api.countReviewing, [], "count_reviewing"),
  row(api.getViewer, [], "get_viewer"),
  row(api.reviewPr, [id, repo, number, verdict, body], "review_pr", { id, repo, number, verdict, body }),
  row(api.commentOnPr, [id, repo, number, body], "comment_on_pr", { id, repo, number, body }),
  row(api.resolveThread, [threadId, repo, number], "resolve_thread", { threadId, repo, number }),
  row(api.unresolveThread, [threadId, repo, number], "unresolve_thread", { threadId, repo, number }),
  row(api.replyToThread, [threadId, repo, number, body], "reply_to_thread", { threadId, repo, number, body }),
  row(api.getStats, [], "get_stats"),
  row(api.listWorktrees, [], "list_worktrees"),
  // The repository browser (#1031, #1033). Both take the repository ROOT
  // and a repository-relative path, in that order -- the root is what the
  // command re-derives against the live scan, and the path is what the
  // containment guard resolves inside it.
  row(api.repoTree, [repoPath, "src"], "repo_tree", { repoPath, path: "src" }),
  row(api.repoFile, [repoPath, "src/main.rs"], "repo_file", { repoPath, path: "src/main.rs" }),
  row(api.classifyWorktrees, [repoPath], "classify_worktrees", { repoPath }),
  // The All Repositories Status column (#1042). Same argument shape as
  // the row above and a deliberately different command: this classifies
  // the MAIN CHECKOUT only, where that one classifies every worktree.
  row(api.classifyRepoUpstream, [repoPath], "classify_repo_upstream", { repoPath }),
  row(api.actOnPr, [id, repo, number, action], "act_on_pr", { id, repo, number, action }),
  row(api.removeWorktrees, [repoPath, worktreePaths], "remove_worktrees", { repoPath, worktreePaths }),
  row(api.latestRelease, [], "latest_release"),
  row(api.dockerState, [], "docker_state"),
  row(api.dockerBuilds, [], "docker_builds"),
  row(api.dockerImages, [], "docker_images"),
  row(api.dockerDiskUsage, [], "docker_disk_usage"),
  row(api.dockerRemoveImages, [ids], "docker_remove_images", { ids }),
  row(api.dockerDanglingVolumes, [], "docker_dangling_volumes"),
  row(api.dockerRemoveVolume, [name], "docker_remove_volume", { name }),
  row(api.dockerPruneCache, [until], "docker_prune_cache", { until }),
  row(api.dockerRunningContainers, [], "docker_running_containers"),
  row(api.dockerRestart, [], "docker_restart"),
  row(api.dockerStart, [], "docker_start"),
  row(api.assessedWorktrees, [], "assessed_worktrees"),
  row(api.removeWorktreeForced, [repoPath, worktreePath], "remove_worktree_forced", { repoPath, worktreePath }),
  row(api.unlockWorktree, [repoPath, worktreePath], "unlock_worktree", { repoPath, worktreePath }),
  row(api.pruneWorktrees, [repoPath], "prune_worktrees", { repoPath }),
  row(api.claudifyCommand, [repoPath, worktreePath, branch], "claudify_command", { repoPath, worktreePath, branch }),
  row(api.claudeProposeStop, [["sess-1"]], "claude_propose_stop", { sessionIds: ["sess-1"] }),
  // The SESSION ID, never a pid: Rust re-derives the pid at the moment
  // of the stop, because the list it would have come from is 10s stale
  // and a recycled pid is somebody else's process (#1219).
  row(api.claudeStopSession, ["sess-1"], "claude_stop_session", { sessionId: "sess-1" }),
  // #1214 widened both with the terms. Passed explicitly here rather
  // than left to the default so the wire keys are asserted: a token
  // arriving under the wrong name is a launch on terms nobody chose.
  row(
    api.claudeLaunchWorktree,
    [repoPath, worktreePath, branch, { model: "opus", permissionMode: "acceptEdits" }],
    "claude_launch_worktree",
    { repoPath, worktreePath, branch, model: "opus", permissionMode: "acceptEdits" },
  ),
  row(
    api.claudeLaunchSession,
    ["sess-1", "/tmp/x", { model: "sonnet", permissionMode: "bypassPermissions" }],
    "claude_launch_session",
    {
      sessionId: "sess-1",
      cwd: "/tmp/x",
      model: "sonnet",
      permissionMode: "bypassPermissions",
    },
  ),
  row(api.claudeLaunchTerms, [], "claude_launch_terms"),
  row(
    api.claudeLaunchWorktreePreview,
    [repoPath, worktreePath, branch, { model: "opus" }],
    "claude_launch_worktree_preview",
    { repoPath, worktreePath, branch, model: "opus", permissionMode: null },
  ),
  row(
    api.claudeLaunchSessionPreview,
    ["sess-1", "/tmp/x", { permissionMode: "acceptEdits" }],
    "claude_launch_session_preview",
    { sessionId: "sess-1", cwd: "/tmp/x", model: null, permissionMode: "acceptEdits" },
  ),
  row(api.setAutoMerge, [id, repo, number, expectedHead, enable], "set_auto_merge", { id, repo, number, expectedHead, enable }),
  row(api.deleteHeadBranch, [refId, repo, number, branch, merged], "delete_head_branch", { refId, repo, number, branch, merged }),
  row(api.updatePrBranch, [id, repo, number, expectedHead], "update_pr_branch", { id, repo, number, expectedHead }),
  row(api.actOnPrs, [prs, action], "act_on_prs", { prs, action }),
  row(api.getPrDetail, [repo, number], "get_pr_detail", { repo, number }),
  row(api.sizeWorktrees, [repoPath], "size_worktrees", { repoPath }),
  row(api.pullCheckout, [path], "pull_checkout", { path }),
  row(api.fetchRefs, [path], "fetch_refs", { path }),
  row(api.removeOrphan, [path], "remove_orphan", { path }),
  row(api.removeWorktree, [repoPath, worktreePath], "remove_worktree", { repoPath, worktreePath }),
  row(api.setViewNeedsGithub, [needs], "set_view_needs_github", { needs }),
  row(api.getWorktreeDirs, [], "get_worktree_dirs"),
  row(api.setWorktreeDirs, [dirs], "set_worktree_dirs", { dirs }),
  row(api.getPollInterval, [], "get_poll_interval"),
  row(api.setPollInterval, [secs], "set_poll_interval", { secs }),
  row(api.getReviewing, [], "get_reviewing"),
  row(api.getCachedReviewing, [], "get_cached_reviewing"),
  row(api.getCycleTrend, [], "get_cycle_trend"),
  row(api.getPeriods, [], "get_periods"),
  row(api.getHistory, [days], "get_history", { days }),
  row(api.getMergedDetail, [], "get_merged_detail"),
  row(api.getAuthState, [], "get_auth_state"),
  row(api.scanArtifacts, [], "scan_artifacts"),
  row(api.readCachedScan, ["artifacts"], "read_cached_scan", { kind: "artifacts" }),
  row(api.sizeArtifacts, [paths], "size_artifacts", { paths }),
  row(api.removeArtifacts, [paths], "remove_artifacts", { paths }),
  row(api.scanVenvs, [], "scan_venvs"),
  row(api.sizeVenvs, [paths], "size_venvs", { paths }),
  row(api.removeVenvs, [paths], "remove_venvs", { paths }),
  row(api.markAssessed, [worktreePath], "mark_assessed", { worktreePath }),
  row(api.clearAssessed, [worktreePath], "clear_assessed", { worktreePath }),
  row(api.previewCleanup, [], "preview_cleanup"),
  row(api.cleanupLog, [], "cleanup_log"),
  row(api.getCleanupPrefs, [], "get_cleanup_prefs"),
  row(api.setCleanupPrefs, [cleanupPrefs], "set_cleanup_prefs", { prefs: cleanupPrefs }),
  row(api.checkPackages, [repoPath], "check_packages", { repoPath }),
  row(api.packagesMarkdown, [repoPath, reports, filter], "packages_markdown", { repoPath, reports, filter }),
  row(api.backgroundPanicked, [], "background_panicked"),
  row(api.backgroundHealth, [], "background_health"),
  row(api.toolVersions, [], "tool_versions"),
  row(api.readLogTail, [4096], "read_log_tail", { maxBytes: 4096 }),
  row(api.revealLog, [], "reveal_log"),
  row(api.claudeMdEffective, [repoPath], "claude_md_effective", { repoPath }),
  // `mode` is omitted by the caller and sent as explicit `null` (#1293):
  // the Rust side takes `Option<Mode>` and defaults to `Cached`, and a
  // missing key and a `null` decode identically there. Sent rather than
  // dropped so the argument object has one shape for both callers.
  row(api.claudeMdAdvice, [repoPath], "claude_md_advice", { repoPath, mode: null }),
  // Claudify (#1292). The target is an OBJECT naming which brief, never
  // the prompt text: Rust looks the brief up from the stored report, so
  // the text that runs is the text `brief::render` produced. A wrapper
  // that flattened this to a string would be the first step back towards
  // a prompt composed in TypeScript.
  row(
    api.claudeMdAdviceLaunch,
    [repoPath, { kind: "finding", index: 2 }],
    "claude_md_advice_launch",
    { repoPath, target: { kind: "finding", index: 2 } },
  ),
  row(
    api.claudeMdAdviceLaunchPreview,
    [repoPath, { kind: "report" }],
    "claude_md_advice_launch_preview",
    { repoPath, target: { kind: "report" } },
  ),
  row(api.scanClaudeMd, [repoPath], "scan_claude_md", { repoPath }),
  // The Claude Code cache, its session list and its aggregates (#914,
  // #917, #921). Three of these are argument-free: they operate on
  // `~/.claude` and Headstate's own database, whose locations the Rust
  // side resolves, so there is nothing for a caller to scope -- and a
  // path supplied over the wire would be a way to make the desktop read
  // somewhere else.
  //
  // Three take a path, and they split two ways. `claudeRevealPath` is
  // `Class::Local`, so its argument can only come from this machine's own
  // frontend, which holds the row it came from. The two per-session reads
  // (#959, #982) are `Class::Read` -- the phone is the case they exist
  // for -- so their argument DOES arrive over the wire, and
  // `claude_transcript_path` in `commands.rs` resolves it against
  // `~/.claude/projects` before reading a byte. That guard, not the
  // absence of a parameter, is what keeps them from being "read anything
  // and send it back".
  row(api.claudeImportTranscripts, [], "claude_import_transcripts"),
  // #1203. `Class::Read`: it walks `~/.claude/projects` without writing
  // and queries our own FTS5 index. The away-from-desk case is the same
  // one the session list serves -- the phone has no transcripts of its
  // own to search.
  row(api.claudeSearchTranscripts, ["fsevents", 20], "claude_search_transcripts", {
    query: "fsevents",
    limit: 20,
  }),
  row(api.claudeIndexCoverage, [], "claude_index_coverage"),
  row(api.claudeSessions, [], "claude_sessions"),
  // #985. Takes a session ID rather than a path, and unlike the two path
  // reads above it needs no resolution guard: the id is looked up in
  // Headstate's OWN table, so an id the store does not have returns
  // `null` rather than reaching the filesystem.
  row(api.claudeSessionsForPr, ["acme/api", 7], "claude_sessions_for_pr", { repo: "acme/api", number: 7 }),
  row(api.claudeSessionDetail, ["s1"], "claude_session_detail", { sessionId: "s1" }),
  row(api.claudeOverview, [], "claude_overview"),
  // #1212. No arguments: the report is over Headstate's own cache in
  // full, so there is nothing for a remote caller to steer.
  row(api.claudeCoverage, [], "claude_coverage"),
  row(api.claudeDefinitions, [], "claude_definitions"),
  row(api.claudePlugins, [], "claude_plugins"),
  // #1071. No arguments: the fold is over the desktop's own session list,
  // so there is no id or path for a remote caller to steer.
  row(api.claudeRestartList, [], "claude_restart_list"),
  row(api.claudeUsageProfile, [], "claude_usage_profile"),
  row(api.claudeSessionUsage, [path], "claude_session_usage", { path }),
  row(api.claudeSubagentRollup, ["s1"], "claude_subagent_rollup", { sessionId: "s1" }),
  row(api.claudeSessionEvents, ["s1"], "claude_session_events", { sessionId: "s1" }),
  row(api.claudeEventProfile, [], "claude_event_profile"),
  row(api.claudeTranscriptTail, [path], "claude_transcript_tail", { path }),
  // #1208. `cursor` rides as an explicit `null` on the first poll rather
  // than being omitted: the Rust argument is an `Option`, and a key that
  // is present-and-null and a key that is absent must not become two
  // different wire shapes for one call.
  row(api.claudeTranscriptFollow, [path, null], "claude_transcript_follow", {
    path,
    cursor: null,
  }),
  row(api.claudeRevealPath, [path], "claude_reveal_path", { path }),
  row(api.readClaudeMd, [path], "read_claude_md", { path }),
  // The Claude Code hook installer (#915). All four take no arguments: the
  // settings path and the binary path are resolved on the Rust side, which
  // is deliberate -- a path supplied over the wire would be a way to make
  // the desktop write to an arbitrary file.
  row(api.claudeHooksInventory, [], "claude_hooks_inventory"),
  row(api.claudeEffectiveSettings, [repoPath], "claude_effective_settings", { repoPath }),
  row(api.claudeConfigHealth, [], "claude_config_health"),
  row(api.claudePermissionOwnership, [], "claude_permission_ownership"),
  row(api.claudeHooksStatus, [], "claude_hooks_status"),
  row(api.claudeMcpServers, [], "claude_mcp_servers"),
  row(api.claudeInstallHooks, [], "claude_install_hooks"),
  row(api.claudeReinstallHooks, [], "claude_reinstall_hooks"),
  row(api.claudeUninstallHooks, [], "claude_uninstall_hooks"),
  row(api.listBranches, [repoPath], "list_branches", { repoPath }),
  row(api.deleteBranches, [repoPath, names], "delete_branches", { repoPath, names }),
  row(api.deleteRemoteBranches, [repoPath, names], "delete_remote_branches", { repoPath, names }),
  row(api.applyUpdatesInBackground, [repoPath, requests, branch], "apply_updates_in_background", { repoPath, requests, branch }),
  row(api.cancelUpdateRun, [repoPath], "cancel_update_run", { repoPath }),
  row(api.updateRunState, [repoPath], "update_run_state", { repoPath }),
  row(api.issuePairingToken, [], "issue_pairing_token"),
  // `replaceExisting` is sent as null when omitted, so the key is always
  // present; this row pins the explicit-value shape.
  row(api.respondToPairing, [requestId, approve, replaceExisting], "respond_to_pairing", { requestId, approve, replaceExisting }),
  row(api.listPairedDevices, [], "list_paired_devices"),
  row(api.revokePairedDevice, [deviceId], "revoke_paired_device", { id: deviceId }),
  // Both take no arguments: the health sample is of THIS machine and
  // the history is bounded on the Rust side, so there is nothing for a
  // caller to scope or to ask for more of.
  // Argument-free: the scope hierarchy is everything the TOKEN can see, so
  // there is nothing for a caller to narrow. #825.
  row(api.statsTree, [], "stats_tree"),
  // The scoped stats trio (#826). Argument order matters more here than on
  // most rows: all three take a scope kind and an optional value, and two of
  // them take a subject as well -- so a transposed pair would send a login
  // where a repository name belongs and come back with a plausible answer to
  // the wrong question.
  row(api.statsCount, ["octocat", "org", "acme", "merged", 30], "stats_count", {
    subject: "octocat",
    scopeKind: "org",
    scopeValue: "acme",
    measure: "merged",
    days: 30,
  }),
  // No `subject` at all, which is the board's defining property: it asks
  // about everyone in the scope. A subject here would render a leaderboard
  // with one name on it.
  row(api.statsBoard, ["org", "acme", "merged", 30], "stats_board", {
    scopeKind: "org",
    scopeValue: "acme",
    measure: "merged",
    days: 30,
  }),
  row(api.statsSeries, ["octocat", "org", "acme", 30], "stats_series", {
    subject: "octocat",
    scopeKind: "org",
    scopeValue: "acme",
    days: 30,
  }),
  // The reviews-GIVEN board. No `subject` and no `measure`: it asks about
  // everyone in `logins`, and its measure is fixed (a review counts against a
  // pull request that MERGED, so reviewing work still in flight does not move
  // somebody up the board).
  row(api.statsReviewers, ["org", "acme", 30, ["octocat", "hubot"]], "stats_reviewers", {
    scopeKind: "org",
    scopeValue: "acme",
    days: 30,
    logins: ["octocat", "hubot"],
  }),
  row(api.systemHealth, [], "system_health"),
  row(api.systemHealthHistory, [], "system_health_history"),
  row(api.healthAlerts, [], "health_alerts"),
  row(api.systemFootprint, [], "system_footprint"),
  // Also argument-free, and for the same reason: it reads THIS machine
  // (or, over the wire, the paired desktop). The ~5s it costs is a
  // property of `nettop`, not of anything a caller could narrow.
  row(api.systemNetworkProcesses, [], "system_network_processes"),
  // Update All (#1012). ARGUMENT-FREE, and that is the property worth
  // pinning here: the set comes from the desktop's own scan roots,
  // re-derived inside the command, never from a path list the caller
  // supplies. A row that grew an argument would be a way to pull
  // arbitrary directories, and this test is where that would show up.
  row(api.updateAllRepositories, [], "update_all_repositories"),
  row(api.cancelUpdateAll, [], "cancel_update_all"),
  row(api.updateAllState, [], "update_all_state"),
];

describe("tauri.ts wrappers through the transport", () => {
  it.each(ROWS.map((r) => [r.command, r] as const))("%s", async (_command, r) => {
    await (r.fn as (...a: unknown[]) => Promise<unknown>)(...r.args);
    expect(local.call).toHaveBeenCalledTimes(1);
    expect(local.call).toHaveBeenCalledWith(r.command, r.expected);
  });

  it("covers every wrapper tauri.ts exports", () => {
    // A wrapper added without a row here would otherwise be the one
    // whose arguments silently drift.
    const exported = Object.values(api).filter((v) => typeof v === "function");
    const covered = new Set<unknown>(ROWS.map((r) => r.fn));
    for (const fn of exported) expect(covered.has(fn)).toBe(true);
    expect(ROWS).toHaveLength(exported.length);
  });

  /// The advice cache's `mode` reaches the wire when the caller sets it.
  ///
  /// A separate assertion for the same reason the terms one is, and it
  /// earns its place: the row above pins the DEFAULT path, where `mode`
  /// is `null` and the Rust side reads `Mode::Cached`. If `mode` were
  /// dropped on the way out, that row would still pass and Refresh
  /// (#1293) would silently be a cached read -- a no-op exactly when a
  /// user presses it.
  it("sends the advice mode the caller chose", async () => {
    await api.claudeMdAdvice("/repos/hello-world", "fresh");
    expect(local.call).toHaveBeenCalledWith("claude_md_advice", {
      repoPath: "/repos/hello-world",
      mode: "fresh",
    });
  });

  /// The pre-#1214 call shape still works and still says "no terms".
  ///
  /// A separate assertion rather than a second ROWS entry, because
  /// `covers every wrapper` counts rows against exports. What it pins
  /// is that omitting the terms sends them as explicit `null` rather
  /// than omitting the keys -- the same shape `respondToPairing` uses
  /// for its optional argument, so the wire is not two shapes depending
  /// on what the caller passed.
  it("sends null terms when the caller chooses none", async () => {
    await api.claudeLaunchSession("sess-1", "/tmp/x");
    expect(local.call).toHaveBeenCalledWith("claude_launch_session", {
      sessionId: "sess-1",
      cwd: "/tmp/x",
      model: null,
      permissionMode: null,
    });
  });

  it("resolves with what the transport returns and rejects with what it throws", async () => {
    local.call.mockResolvedValueOnce([]);
    await expect(api.getCached()).resolves.toEqual([]);
    local.call.mockRejectedValueOnce("not authenticated");
    await expect(api.refreshNow()).rejects.toBe("not authenticated");
  });
});

/// The events the desktop poll loop emits, and the hook that subscribes
/// to each. The remote transport will re-emit them under these names,
/// so the hooks must reach them through the seam and nowhere else.
const POLL_EVENTS: [string, () => unknown][] = [
  ["prs-updated", hooks.usePullRequests],
  ["poll-state", hooks.usePollState],
  ["poll-error", hooks.usePollError],
  ["prs-truncated", hooks.useTruncation],
  ["prs-incomplete", hooks.useIncomplete],
  ["store-error", hooks.useStoreError],
  ["worktree-removal-progress", hooks.useRemovalProgress],
  ["reviewing-short", hooks.useReviewShortfall],
  // Missing from this list until #850, which is the entry that showed
  // why the list needed a completeness check at all rather than a
  // careful author. `useUpdateProgress` does reach it through the seam,
  // so there was no live bug -- but nothing here would have noticed if
  // it did not, and the failure a missing row hides is silent and
  // desktop-invisible: the phone's page never fills in, with no error.
  // `mirroredConstants.test.ts` now asserts this list against
  // `EVENT_NAMES` in `remote/events.rs`, the way the wrapper test below
  // asserts its rows against `tauri.ts`' exports.
  ["update-run-progress", hooks.useUpdateProgress],
  ["update-run-done", hooks.useUpdateRunOutcome],
  // The tenth allowlisted name (#657). It is here for the same reason
  // as the rest: the phone reaches it through this seam, so a hook that
  // imported Tauri's `listen` directly would work on the desktop and
  // silently never fire on the phone.
  ["branch-scan-progress", () => hooks.useBranchScan("/code/app")],
  // The eleventh (#724). Same seam, same reason — and the phone is the
  // client that needs it most: it has no window to leave open and
  // watch a ten-minute deletion in.
  ["branch-delete-progress", () => hooks.useBranchDeleteProgress("/code/app")],
  // The twelfth (#754). Sizes stream in per worktree because the walk
  // has no useful upper bound, and a hook that imported Tauri's
  // `listen` directly would work on the desktop and silently never
  // fire on the phone — which is the whole reason this list exists.
  ["worktree-size", () => hooks.useWorktreeSizes("/code/app")],
  // The thirteenth (#830). Verdicts stream per worktree because
  // classification makes an UNBOUNDED number of git calls per worktree --
  // up to four per changed file -- so a per-call timeout never bounded
  // the pass and a 111-worktree repository never resolved its safety
  // column at all. Here for the same reason as the entry above: a hook
  // that imported Tauri's `listen` directly would work on the desktop
  // and silently never fire on the phone.
  ["worktree-safety", () => hooks.useWorktreeSafety("/code/app")],
  // The fourteenth (#1093). PR Stats collects a window a group of days at
  // a time over minutes, so what the page can show is what arrives while
  // it is open — and a hook that imported Tauri's `listen` directly would
  // work on the desktop and silently never fire on the phone, which is
  // the client with no window to leave open and wait in.
  ["stats-backfill-progress", () => hooks.useStatsBackfill("board|merged|*|org:X")],
];

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return createElement(QueryClientProvider, { client: qc }, children);
}

describe("poll-loop events through the transport", () => {
  it.each(POLL_EVENTS)("%s is subscribed via transport.listen", (event, hook) => {
    const { unmount } = renderHook(() => hook(), { wrapper });
    const events = local.listen.mock.calls.map((c) => c[0]);
    expect(events).toContain(event);
    unmount();
  });

  it("passes the event name and callback through unchanged", async () => {
    const cb = () => {};
    const un = await listen("prs-updated", cb);
    expect(local.listen).toHaveBeenCalledWith("prs-updated", cb);
    expect(typeof un).toBe("function");
  });

  it("call passes name and args through unchanged", async () => {
    await call("get_history", { days: 7 });
    expect(local.call).toHaveBeenCalledWith("get_history", { days: 7 });
  });
});

describe("transport selection", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it("defaults to desktop when VITE_TARGET is unset", () => {
    // The Vite define fills in the default at build time; the test
    // environment has no .env, so this is the define at work.
    expect(import.meta.env.VITE_TARGET).toBe("desktop");
  });

  it("selects the remote transport for mobile", async () => {
    vi.stubEnv("VITE_TARGET", "mobile");
    vi.resetModules();
    const mod = await import("./transport");
    await expect(mod.call("get_cached")).resolves.toBe("from the phone");
    expect(remote.call).toHaveBeenCalledWith("get_cached", undefined);
    expect(local.call).not.toHaveBeenCalled();
    const cb = () => {};
    await mod.listen("prs-updated", cb);
    expect(remote.listen).toHaveBeenCalledWith("prs-updated", cb);
  });

  it("refuses a target it does not know", async () => {
    vi.stubEnv("VITE_TARGET", "toaster");
    vi.resetModules();
    await expect(import("./transport")).rejects.toThrow(/VITE_TARGET/);
  });
});
