import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UiPrefs } from "@/api/tauri";
import type { IssueBatch, JiraConnection, JiraIssue, LinkIndex } from "@/types/jira";
import { renderWithQuery } from "@/test-utils";
import { useJiraView } from "@/store/jira";

const connection = vi.hoisted(() => vi.fn<() => Promise<JiraConnection>>());
const links = vi.hoisted(() => vi.fn<() => Promise<LinkIndex>>());
const issues = vi.hoisted(() => vi.fn<(keys: string[]) => Promise<IssueBatch>>());
const assigned = vi.hoisted(() => vi.fn<() => Promise<IssueBatch>>());
const worktrees = vi.hoisted(() => ({ data: [] as unknown[], isPending: false }));
vi.mock("@/api/tauri", () => ({
  jiraConnection: connection,
  jiraLinks: links,
  jiraIssues: issues,
  jiraAssigned: assigned,
}));
vi.mock("@/api/hooks", () => ({ useWorktrees: () => worktrees }));
vi.mock("@/lib/useIsMobile", () => ({ useIsMobile: () => true }));

import { JiraProvider } from "./JiraContext";
import { JiraPage } from "./JiraPage";

const prefs: UiPrefs = {
  hidden_views: [],
  close_hides_to_tray: true,
  announce_updates: true,
  claude_integrations_enabled: false,
  diagnostic_logging: false,
  terminal_command: "",
  stale_venv_days: 0,
  battery_low_percent: 0,
  jira_enabled: true,
  jira_excluded_projects: [],
};

const issue = (key: string, over: Partial<JiraIssue> = {}): JiraIssue => ({
  key,
  summary: `Summary of ${key}`,
  issue_type: { name: "Story", hierarchy_level: 0 },
  status: { name: "In Progress", category: "in_progress" },
  assignee: "Dana",
  parent_key: null,
  parent_checked: true,
  project_key: key.split("-")[0],
  url: `https://jira.example.internal/browse/${key}`,
  updated: null,
  ...over,
});
const batch = (found: JiraIssue[], over: Partial<IssueBatch> = {}): IssueBatch => ({
  issues: found,
  missing: [],
  unasked: [],
  error: null,
  truncated: false,
  ...over,
});

/// As the desktop's index reports it: pull request #7 names ABC-1.
const INDEX: LinkIndex = {
  links: [
    {
      key: "ABC-1",
      found_in: "title",
      current: true,
      target: {
        kind: "pr",
        source: { provider: "github", host: "github.com" },
        role: "authored",
        repo: "octocat/hello-world",
        number: 7,
        title: "ABC-1: fix login",
        url: "https://github.com/octocat/hello-world/pull/7",
        head_ref: "abc-1-fix-login",
        is_draft: false,
      },
    },
  ],
  projects: ["ABC"],
  error: null,
};

const EPIC = issue("ABC-2", { summary: "Login epic", issue_type: { name: "Epic", hierarchy_level: 1 } });
const STORY = issue("ABC-1", { parent_key: "ABC-2" });
const TODO = issue("ABC-3", { summary: "Not started", status: { name: "To Do", category: "new" } });

beforeEach(() => {
  connection.mockReset().mockResolvedValue({ state: "logged_in", site: "jira.example.internal", account: null });
  links.mockReset().mockResolvedValue(INDEX);
  // As the desktop answers: asked and not returned is `missing`.
  issues.mockReset().mockImplementation((keys) => {
    const found = [EPIC, STORY].filter((i) => keys.includes(i.key));
    return Promise.resolve(batch(found, { missing: keys.filter((k) => !found.some((i) => i.key === k)) }));
  });
  assigned.mockReset().mockResolvedValue(batch([TODO]));
  worktrees.data = [];
  useJiraView.setState({ selected: null, filter: "all" });
});
afterEach(cleanup);

/// Let every load the page starts finish inside act(): the loader gathers
/// keys for 15 ms, and each answer that names a parent starts another
/// round. Also keeps one test's batch from being answered by the next
/// test's mocks.
const quiet = () => act(() => new Promise((r) => setTimeout(r, 60)));

const mount = (ui: React.ReactNode) => renderWithQuery(<JiraProvider prefs={prefs}>{ui}</JiraProvider>);

/// On a phone the sidebar is a sheet, so the page itself is the list until
/// an issue is picked, and offers a way back (#2047).
describe("My Jira issues on a phone", () => {
  it("is the list until an issue is picked, then the issue with a way back", async () => {
    mount(<JiraPage />);
    fireEvent.click(await screen.findByRole("button", { name: /ABC-1 Summary of ABC-1/ }));
    expect(await screen.findByRole("heading", { name: /ABC-1 Summary of ABC-1/ })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "All issues" }));
    expect(await screen.findByRole("searchbox", { name: "Go to ticket" })).toBeTruthy();
    expect(useJiraView.getState().selected).toBeNull();
    await quiet();
  });
});
