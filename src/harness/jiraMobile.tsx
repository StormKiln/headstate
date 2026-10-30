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
import { useJiraView } from "../store/jira";
import { REQUIRED_PROTOCOL_VERSION } from "../lib/protocol";
import type { IssueBatch, JiraIssue, JiraLink, KeyLookup, LinkIndex } from "../types/jira";
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

const EPIC = issue("ABC-1", {
  summary: "Sign-in that survives a network change",
  issue_type: { name: "Epic", hierarchy_level: 1 },
  assignee: "Robin Example",
});
const ISSUES: JiraIssue[] = [
  EPIC,
  issue("ABC-2", { summary: "Retry the fetch client on a dropped connection", status: { name: "In Review", category: "in_progress" }, parent_key: "ABC-1" }),
  issue("ABC-3", { summary: "Keep the session across a Wi-Fi to cellular handover", parent_key: "ABC-1" }),
  issue("ABC-4", { summary: "Explain a lost sign-in in words", status: { name: "To Do", category: "new" }, parent_key: "ABC-1" }),
  issue("ABC-5", { summary: "Fix the flaky timezone test", issue_type: { name: "Bug", hierarchy_level: 0 }, status: { name: "Done", category: "done" } }),
  issue("XY-1", { summary: "Tidy the release notes template", status: { name: "To Do", category: "new" }, read_at: ago(9), stale: true }),
];
const byKey = new Map(ISSUES.map((i) => [i.key, i]));

/// The `octocat` fixture pull requests, retitled to name tickets.
const TITLES: Record<number, string> = {
  42: "ABC-2: Add retry to the fetch client",
  43: "ABC-5: Fix flaky timezone test",
};
const PRS: PullRequest[] = PR_FIXTURES.map((pr) => ({ ...pr, title: TITLES[pr.number] ?? pr.title }));

const prLink = (key: string, pr: PullRequest): JiraLink => ({
  key,
  found_in: "title",
  current: true,
  target: {
    kind: "pr",
    source: { provider: "github", host: "github.com" },
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
const LINKS: JiraLink[] = PRS.flatMap((pr) => {
  const key = /^([A-Z]+-\d+):/.exec(pr.title)?.[1];
  return key ? [prLink(key, pr)] : [];
});
const INDEX: LinkIndex = { links: LINKS, projects: ["ABC", "XY"], error: null };

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
    return { key, links: LINKS.filter((l) => l.key === key), search: { state: "not_asked", reason: "already known from your pull requests" }, gitlab: null };
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
