/// The app shell's browser harness page (#1583).
///
/// NOT part of the app bundle: `harness/shell.html` loads it, and only
/// `vite.harness.config.ts` builds that page. It mounts the SAME tree
/// `main.tsx` does -- `ErrorBoundary`, `AuthGate`, `App`, the toaster --
/// with Tauri's IPC answered by `mockIPC` from generated fixtures: long
/// enough lists that every sidebar and page scrolls (300 pull requests
/// over 120 repositories, 1,400 Claude Code sessions, 60 repositories of
/// worktrees, a transcript of every record kind forty times over).
///
/// `scripts/check-shell-scroll.mjs` drives it and measures whether the
/// DOCUMENT ever scrolls, which jsdom cannot: it does no layout.
///
/// # Query parameters
///
/// - `view`: the `View` to open.
/// - `state`: JSON merged into the filter store (`claudePage`,
///   `claudeSelected`, `selectedPr`, ...).
/// - `pollError`: emit a `poll-error` event after mount, so `AuthGate`
///   shows its banner -- the banner #1583 found above the shell.
///
/// A command with no answer here REJECTS, so a page renders its error
/// state rather than a `null` it would misread. `window.__shell`
/// lists what was asked and can raise a toast.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Toaster, toast } from "sonner";
import App from "../App";
import { AuthGate } from "../components/AuthGate";
import { ErrorBoundary } from "../components/ErrorBoundary";
import { everyRecord } from "../components/transcript/fixtures";
import { PR_FIXTURES } from "../fixtures/prs";
import "../index.css";
import { useFilters, type View } from "../store/filters";
import type { PullRequest } from "../types/pr";
import type { PageCursor, TranscriptMessage, TranscriptPage, TranscriptWindow } from "../types/transcript";
import "sonner/dist/styles.css";

const PRS: PullRequest[] = Array.from({ length: 300 }, (_, i) => ({
  ...PR_FIXTURES[i % PR_FIXTURES.length],
  id: `PR_h${i}`,
  number: 1000 + i,
  repo: `octocat/repo-${i % 120}`,
  title: `Harness pull request ${i}`,
}));

const SESSIONS = Array.from({ length: 1400 }, (_, i) => ({
  session_id: `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`,
  name: `Harness session ${i}`,
  opening_prompt: `Do the harness thing number ${i}`,
  cwd: `/Users/acme/code/widget-${i % 40}`,
  git_branch: `feat/branch-${i}`,
  last_activity_at: new Date(Date.UTC(2026, 8, 1) - i * 3_600_000).toISOString(),
  liveness: i < 5 ? { state: "running", pid: 1000 + i, status: null } : { state: "dead", why: 0 },
  cwd_state: { state: "exists" },
  kind: { kind: "own" },
  subagents: 0,
  waiting: { state: "no", reason: "never-observed" },
  context_pressure: null,
}));

const WORKTREES = {
  repos: Array.from({ length: 60 }, (_, r) => ({
    identity: `octocat/widget-${r}`,
    name: `widget-${r}`,
    path: `/Users/acme/code/widget-${r}`,
    worktrees: Array.from({ length: 8 }, (_, w) => ({
      path: w === 0 ? `/Users/acme/code/widget-${r}` : `/Users/acme/code/widget-${r}/.wt/wt-${w}`,
      branch: w === 0 ? "main" : `feat/wt-${w}`,
      head: `abc${r}${w}`,
      size_bytes: 1_000_000 * (w + 1),
      safety: w === 0 ? { kind: "main_checkout" } : { kind: "safe" },
      is_main: w === 0,
      merged_at: null,
      upstream: null,
      last_commit: "2026-09-01T10:00:00Z",
    })),
  })),
  unreadable: [],
};

/// A long transcript: every record kind, many times over, under fresh ids.
const MESSAGES: TranscriptMessage[] = Array.from({ length: 40 }, (_, k) =>
  everyRecord().map((m) => ({ ...m, id: `${m.id}-${k}`, turn_id: m.turn_id === null ? null : `${m.turn_id}-${k}` })),
).flat();
const PAGE: TranscriptPage = {
  messages: MESSAGES,
  truncated: false,
  bytes_read: 400_000,
  file_bytes: 400_000,
  machinery_records: [],
  unparseable_records: 0,
  duplicate_records: 0,
};
const cursor = (offset: number): PageCursor => ({ offset, behind_digest: "harness" });
const WINDOW: TranscriptWindow = {
  page: PAGE,
  start: cursor(0),
  end: cursor(PAGE.file_bytes),
  at_start: true,
  at_end: true,
  rewritten: false,
  position: { first: null, last: null, total: null, exact: false, basis: "bytes" },
  seam: { first_model: null, last_model: null },
  bytes_scanned: 0,
};

const detailOf = (id: string) => ({
  session_id: id,
  claude_version: "2.1.270",
  transcript_path: `/Users/acme/.claude/projects/widget/${id}.jsonl`,
  first_seen_at: "2026-09-01T09:00:00Z",
  liveness: { state: "dead", why: "exited" },
  transcript_state: { state: "exists" },
  resume: { command: `cd '/Users/acme/code/widget' && claude --resume ${id}`, caveat: null, anchored: true },
  runs: 1,
  registry_failure: null,
  kind: { kind: "own" },
  subagents: [],
  parent: null,
  unattributed: null,
  compactions: null,
  agent_types: null,
  waiting: { state: "no", reason: "never-observed" },
});

const statsRepos = (owner: string, n: number) =>
  Array.from({ length: n }, (_, i) => ({
    nameWithOwner: `${owner}/repo-${i}`,
    pushedAt: new Date(Date.UTC(2026, 8, 11) - i * 86_400_000).toISOString(),
    isArchived: false,
  }));

/// PR Stats' sidebar: forty organisations, so the column is long.
const STATS_TREE = {
  viewer: "octocat",
  orgs: Array.from({ length: 40 }, (_, o) => ({
    login: `acme-${o}`,
    name: `Acme ${o}`,
    repos: statsRepos(`acme-${o}`, 30),
    reposTotal: 30,
    members: [{ login: "octocat", name: "Mona Octocat", avatarUrl: null }],
    membersTotal: 1,
    readable: true,
  })),
  orgsTotal: 40,
  personal: statsRepos("octocat", 30),
  personalTotal: 30,
  refusedFields: 0,
  spend: { points: 2, requests: 2, unmetered: 0, remaining: 4900, resetAt: null },
};

const ANSWERS: Record<string, (args: unknown) => unknown> = {
  stats_tree: () => STATS_TREE,
  get_auth_state: () => ({ ok: true, message: "" }),
  get_ui_prefs: () => ({
    hidden_views: [],
    close_hides_to_tray: true,
    announce_updates: false,
    claude_integrations_enabled: true,
    terminal_command: "",
    diagnostic_logging: false,
    stale_venv_days: 0,
    battery_low_percent: 0,
  }),
  get_worktree_dirs: () => ["/Users/acme/code"],
  list_worktrees: () => WORKTREES,
  assessed_worktrees: () => [],
  get_reviewing: () => PRS,
  count_reviewing: () => PRS.length,
  claude_session_detail: (a) => detailOf((a as { sessionId: string }).sessionId),
  claude_transcript_page: (a) =>
    (a as { direction: string }).direction === "after"
      ? { ...WINDOW, page: { ...PAGE, messages: [], bytes_read: 0 }, start: WINDOW.end }
      : WINDOW,
  get_pr_detail: (a) => {
    const { repo, number } = a as { repo: string; number: number };
    const pr = PRS.find((p) => p.repo === repo && p.number === number) ?? PRS[0];
    return {
      ...pr,
      state: "open",
      body: Array.from({ length: 60 }, (_, i) => `Paragraph ${i} of a long description.`).join("\n\n"),
      merge_queue_enabled: false,
      additions: 100,
      deletions: 20,
      changed_files: 3,
      comments: Array.from({ length: 40 }, (_, i) => ({
        author: "octocat",
        created_at: "2026-09-01T10:00:00Z",
        body: `Comment ${i}`,
      })),
      comment_count: 40,
      review_threads: [],
      review_threads_total: 0,
      checks: [],
      checks_total: 0,
    };
  },
  claude_sessions: () => ({
    sessions: SESSIONS,
    reasons: ["pid is no longer running"],
    registry_failure: null,
    registry_unreadable: [],
    registry_unnamed: [],
  }),
  get_cached: () => PRS,
  refresh_now: () => PRS,
};

/// Commands whose answer is nothing: fire-and-forget writes.
const VOID = new Set(["diag_log", "set_view_needs_github"]);

/// What the driver reads and drives.
interface ShellProbe {
  /// Every command asked, answered or not.
  asked: string[];
  /// Raise a toast, to check the toaster still shows under the lock.
  toast: (text: string) => void;
}

declare global {
  interface Window {
    __shell?: ShellProbe;
  }
}

const probe: ShellProbe = { asked: [], toast: (text) => void toast(text) };
window.__shell = probe;

mockIPC(
  (cmd, args) => {
    if (!probe.asked.includes(cmd)) probe.asked.push(cmd);
    if (cmd in ANSWERS) return ANSWERS[cmd](args);
    if (VOID.has(cmd)) return null;
    throw new Error(`harness: no answer for ${cmd}`);
  },
  { shouldMockEvents: true },
);

const params = new URLSearchParams(location.search);
const view = (params.get("view") ?? "my-prs") as View;
useFilters.setState({ view, ...(JSON.parse(params.get("state") ?? "{}") as object) });

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    <ErrorBoundary>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <AuthGate>
          <App />
        </AuthGate>
        <Toaster theme="dark" position="bottom-right" richColors />
      </QueryClientProvider>
    </ErrorBoundary>
  </StrictMode>,
);

const pollError = params.get("pollError");
if (pollError !== null) {
  setTimeout(() => void emit("poll-error", pollError || "GitHub request failed: rate limited"), 500);
}
