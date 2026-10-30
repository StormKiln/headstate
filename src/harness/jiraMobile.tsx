/// My Jira issues on a phone: the harness page behind the #2047 screenshots.
///
/// NOT part of the app bundle: `harness/jira-mobile.html` loads it, and
/// only `vite.harness-jira.config.ts` builds that page, with
/// `VITE_TARGET=mobile`. So this runs the PHONE's transport: every desktop
/// command goes to the companion's `remote_call` and its reply is checked
/// against the wire contract, exactly as on a paired phone. `mockIPC`
/// plays the companion, answering from the synthetic data below.
///
/// Synthetic only -- this is a public repo, and `scripts/check-privacy.sh`
/// reads it. Keys are single-digit (`ABC-1`), the Jira site is
/// `jira.example.internal`, the people are `Dana Example` and friends,
/// and the pull requests are the `octocat` fixtures.
///
/// `scripts/jira-mobile-screenshots.mjs` drives it at 390x844 and saves
/// the PNGs to `docs/screenshots/jira/`.
///
/// # Query parameters
///
/// - `view`: the `View` to open (`jira`, `my-prs`).
/// - `state`: JSON merged into the filter store.
/// - `select`: a Jira key to open in My Jira issues.
/// - `filter`: My Jira issues' filter (`all`, `linked`, `assigned`, `attention`).
///
/// A command with no answer here REJECTS. `window.__jira.unanswered`
/// names each one, so the driver can say what the page asked for that the
/// harness did not supply.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Toaster } from "sonner";
import App from "../App";
import { AuthGate } from "../components/AuthGate";
import { ErrorBoundary } from "../components/ErrorBoundary";
import { PR_FIXTURES } from "../fixtures/prs";
import "../index.css";
import { useFilters, type View } from "../store/filters";
import { useJiraView, type JiraFilter } from "../store/jira";
import { REQUIRED_PROTOCOL_VERSION } from "../lib/protocol";
import type { IssueBatch, JiraIssue, JiraLink, KeyLookup, KeySearch, LinkIndex, PrState } from "../types/jira";
import type { PullRequest } from "../types/pr";
import "sonner/dist/styles.css";

const SITE = "jira.example.internal";
const NOW = Date.now();
const ago = (minutes: number) => new Date(NOW - minutes * 60_000).toISOString();

const issue = (key: string, over: Partial<JiraIssue>): JiraIssue => ({
  key,
  id: key,
  summary: `Summary of ${key}`,
  issue_type: { name: "Story", hierarchy_level: 0 },
  status: { name: "In Progress", category: "in_progress" },
  assignee: "Dana Example",
  parent_key: null,
  parent_checked: true,
  project_key: key.split("-")[0],
  url: `https://${SITE}/browse/${key}`,
  updated: null,
  read_at: ago(1),
  ...over,
});

/// One of each drift state the page can show, so the screenshots and the
/// demo cover them all (#2049):
///
/// | Key   | Ticket      | Pull requests                        | Shown             |
/// |-------|-------------|--------------------------------------|-------------------|
/// | ABC-2 | In Review   | #42 open, #40 merged (searched)      | R3: 1 of 2 merged |
/// | ABC-3 | In Progress | #41 yours, left your list; merged    | R2, as a fact     |
/// | ABC-4 | To Do       | spoon-knife #7 open                  | R1                |
/// | ABC-5 | Done        | #43 open                             | R4                |
/// | ABC-6 | In Progress | #39 merged; the search was truncated | R2, qualified     |
/// | ABC-7 | In Review   | #38 merged four minutes ago          | R2, grace period  |
/// | XY-2  | In Progress | #37 closed without merging           | R5                |
/// | XY-1  | To Do       | none; assigned to you                | Start work        |
///
/// ABC-9 is an initiative over both epics, ABC-1 and XY-3.
const INITIATIVE = issue("ABC-9", {
  summary: "Make sign-in dependable",
  issue_type: { name: "Initiative", hierarchy_level: 2 },
  assignee: "Robin Example",
});
const EPIC = issue("ABC-1", {
  summary: "Sign-in that survives a network change",
  issue_type: { name: "Epic", hierarchy_level: 1 },
  assignee: "Robin Example",
  parent_key: "ABC-9",
});
const EPIC_XY = issue("XY-3", {
  summary: "Release tooling",
  issue_type: { name: "Epic", hierarchy_level: 1 },
  assignee: "Robin Example",
  parent_key: "ABC-9",
});
const ISSUES: JiraIssue[] = [
  INITIATIVE,
  EPIC,
  EPIC_XY,
  issue("ABC-2", { summary: "Retry the fetch client on a dropped connection", status: { name: "In Review", category: "in_progress" }, parent_key: "ABC-1" }),
  issue("ABC-3", { summary: "Keep the session across a Wi-Fi to cellular handover", parent_key: "ABC-1" }),
  issue("ABC-4", { summary: "Explain a lost sign-in in words", status: { name: "To Do", category: "new" }, parent_key: "ABC-1" }),
  issue("ABC-5", { summary: "Fix the flaky timezone test", issue_type: { name: "Bug", hierarchy_level: 0 }, status: { name: "Done", category: "done" } }),
  issue("ABC-6", { summary: "Refresh the token before it expires", parent_key: "ABC-1" }),
  issue("ABC-7", { summary: "Show when the session was last renewed", status: { name: "In Review", category: "in_progress" }, parent_key: "ABC-1" }),
  issue("XY-1", { summary: "Tidy the release notes template", status: { name: "To Do", category: "new" }, parent_key: "XY-3", read_at: ago(9), stale: true }),
  issue("XY-2", { summary: "Sign release builds in CI", parent_key: "XY-3" }),
];
const byKey = new Map(ISSUES.map((i) => [i.key, i]));

/// The `octocat` fixture pull requests, retitled to name tickets. #43 is
/// made ready for review, so its Done ticket disagrees with it (R4).
const TITLES: Record<number, string> = {
  42: "ABC-2: Add retry to the fetch client",
  43: "ABC-5: Fix flaky timezone test",
  7: "ABC-4: Explain a lost sign-in in words",
};
const PRS: PullRequest[] = PR_FIXTURES.map((pr) => ({
  ...pr,
  title: TITLES[pr.number] ?? pr.title,
  is_draft: pr.number === 43 ? false : pr.is_draft,
}));

const GITHUB = { provider: "github", host: "github.com" } as const;
const prLink = (key: string, pr: PullRequest): JiraLink => ({
  key,
  found_in: "title",
  current: true,
  target: {
    kind: "pr",
    source: GITHUB,
    role: "authored",
    repo: pr.repo,
    number: pr.number,
    title: pr.title,
    url: pr.url,
    head_ref: pr.head_ref,
    is_draft: pr.is_draft,
    state: null,
    author: pr.author,
  },
});

/// A pull request in `octocat/hello-world` that is not in the open list:
/// one a search found, or one of yours that has left the list.
const elsewhere = (
  key: string,
  number: number,
  title: string,
  over: { role: "authored" | "searched"; state: PrState | null; merged_at?: string; author?: string },
): JiraLink => ({
  key,
  found_in: "title",
  current: false,
  target: {
    kind: "pr",
    source: GITHUB,
    role: over.role,
    repo: "octocat/hello-world",
    number,
    title: `${key}: ${title}`,
    url: `https://github.com/octocat/hello-world/pull/${number}`,
    head_ref: `${key.toLowerCase()}-${title.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`,
    is_draft: false,
    state: over.state,
    author: over.author ?? "octocat",
    merged_at: over.merged_at ?? null,
  },
});

const LINKS: JiraLink[] = [
  ...PRS.flatMap((pr) => {
    const key = /^([A-Z]+-\d+):/.exec(pr.title)?.[1];
    return key ? [prLink(key, pr)] : [];
  }),
  // Yours, opened earlier and since gone from your open list. The index
  // cannot tell merged from closed; the search below says which.
  elsewhere("ABC-3", 41, "Keep the session on a network change", { role: "authored", state: null }),
];
const INDEX: LinkIndex = { links: LINKS, projects: ["ABC", "XY"], error: null };

/// What `jira_find`'s GitHub search found, per key.
const SEARCHED: JiraLink[] = [
  elsewhere("ABC-2", 40, "Retry idempotent requests only", { role: "searched", state: "merged", merged_at: ago(300), author: "robin-example" }),
  elsewhere("ABC-3", 41, "Keep the session on a network change", { role: "searched", state: "merged", merged_at: ago(120) }),
  elsewhere("ABC-6", 39, "Refresh the token early", { role: "searched", state: "merged", merged_at: ago(200), author: "robin-example" }),
  elsewhere("ABC-7", 38, "Show the last renewal time", { role: "searched", state: "merged", merged_at: ago(4) }),
  elsewhere("XY-2", 37, "Sign builds with the release key", { role: "searched", state: "closed" }),
];

/// A search that covered everything it set out to, except ABC-6's, which
/// matched more than it read. A key with nothing searched is answered from
/// the index, as the desktop does.
const searchOf = (key: string): KeySearch => {
  const hits = SEARCHED.filter((l) => l.key === key).length;
  if (hits === 0) return { state: "not_asked", reason: "already known from your pull requests" };
  const truncated = key === "ABC-6";
  return {
    state: "asked",
    at: ago(2),
    total: truncated ? 25 : hits,
    read: truncated ? 20 : hits,
    truncated,
    scope: ["octocat"],
    unsearched: [],
  };
};

const batch = (keys: string[]): IssueBatch => {
  const found = keys.flatMap((k) => byKey.get(k) ?? []);
  return { issues: found, missing: keys.filter((k) => !byKey.has(k)), unasked: [], error: null, truncated: false };
};

const PREFS = {
  hidden_views: [],
  close_hides_to_tray: true,
  announce_updates: false,
  claude_integrations_enabled: false,
  terminal_command: "",
  diagnostic_logging: false,
  stale_venv_days: 0,
  battery_low_percent: 0,
  jira_enabled: true,
  jira_excluded_projects: [],
  jira_status_expectations: {},
  jira_chips_on_prs: true,
  jira_branch_template: "",
  jira_no_ticket_marker: false,
  jira_merge_grace_mins: 15,
};

/// The desktop's answers, by command, as `remote_call` forwards them.
const DESKTOP: Record<string, (args: Record<string, unknown>) => unknown> = {
  get_auth_state: () => ({ ok: true, message: "" }),
  background_panicked: () => false,
  get_autostart: () => false,
  get_remote_enabled: () => true,
  get_poll_interval: () => 60,
  get_gitlab_host: () => "",
  get_worktree_dirs: () => [],
  get_discovery_exclusions: () => [],
  diag_log: () => null,
  read_cached_scan: () => null,
  get_ui_prefs: () => PREFS,
  get_cached: () => PRS,
  refresh_now: () => PRS,
  get_reviewing: () => [],
  count_reviewing: () => 0,
  list_worktrees: () => ({ repos: [], unreadable: [] }),
  jira_connection: () => ({ state: "logged_in", site: SITE, account: null }),
  jira_links: () => INDEX,
  jira_assigned: () => batch(ISSUES.filter((i) => i.assignee === "Dana Example").map((i) => i.key)),
  jira_issues: (a) => batch(a.keys as string[]),
  jira_find: (a): KeyLookup => {
    const key = a.key as string;
    return { key, links: [...LINKS, ...SEARCHED].filter((l) => l.key === key), search: searchOf(key), gitlab: null };
  },
};

/// The companion's own commands, answered for a phone paired to a
/// desktop it can reach.
const COMPANION: Record<string, (args: Record<string, unknown>) => unknown> = {
  connection_state: () => ({
    state: "connected",
    desktop: "Dana's desktop",
    last_poll: ago(1),
    protocol_version: REQUIRED_PROTOCOL_VERSION,
    stale: false,
    has_mldsa: true,
  }),
  subscribe_events: () => null,
  get_phone_measurement_prefs: () => ({ enabled: false }),
  get_phone_notify_prefs: () => null,
  get_session_mutes: () => [],
  poll_session_toasts: () => [],
  take_notification_session: () => null,
};

/// Asked by pages and panels these shots do not show, and REJECTED on
/// purpose: what they would render is not what the screenshots are of, and
/// a rejection there is an error panel nobody sees. Every other command
/// the page asks for and the harness cannot answer fails the driver.
const DECLINED = new Set(["get_notify_prefs", "get_cleanup_prefs", "get_source_snapshot", "plugin:app|version"]);

interface JiraProbe {
  asked: string[];
  unanswered: string[];
}
declare global {
  interface Window {
    __jira?: JiraProbe;
  }
}
const probe: JiraProbe = { asked: [], unanswered: [] };
window.__jira = probe;

const note = (list: string[], name: string) => {
  if (!list.includes(name)) list.push(name);
};

mockIPC(
  (cmd, raw) => {
    const args = (raw ?? {}) as Record<string, unknown>;
    if (cmd === "remote_call") {
      const command = args.command as string;
      note(probe.asked, command);
      const answer = DESKTOP[command];
      if (answer) return answer((args.args ?? {}) as Record<string, unknown>);
      if (!DECLINED.has(command)) note(probe.unanswered, command);
      throw new Error(`harness: no answer for ${command}`);
    }
    note(probe.asked, cmd);
    const answer = COMPANION[cmd];
    if (answer) return answer(args);
    if (!DECLINED.has(cmd)) note(probe.unanswered, cmd);
    throw new Error(`harness: no answer for ${cmd}`);
  },
  { shouldMockEvents: true },
);

const params = new URLSearchParams(location.search);
const view = (params.get("view") ?? "jira") as View;
useFilters.setState({ view, ...(JSON.parse(params.get("state") ?? "{}") as object) });
const select = params.get("select");
if (select) useJiraView.getState().select(select);
const filter = params.get("filter") as JiraFilter | null;
if (filter) useJiraView.getState().setFilter(filter);

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
