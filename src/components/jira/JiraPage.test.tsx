import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UiPrefs } from "@/api/tauri";
import type { IssueBatch, JiraConnection, JiraIssue, JiraLink, KeyLookup, LinkIndex } from "@/types/jira";
import { renderWithQuery } from "@/test-utils";
import { useFilters } from "@/store/filters";
import { useJiraView } from "@/store/jira";

const connection = vi.hoisted(() => vi.fn<() => Promise<JiraConnection>>());
const links = vi.hoisted(() => vi.fn<() => Promise<LinkIndex>>());
const find = vi.hoisted(() => vi.fn<(key: string, more: boolean) => Promise<KeyLookup>>());
const issues = vi.hoisted(() => vi.fn<(keys: string[]) => Promise<IssueBatch>>());
const assigned = vi.hoisted(() => vi.fn<() => Promise<IssueBatch>>());
const worktrees = vi.hoisted(() => ({ data: [] as unknown[], isPending: false }));
vi.mock("@/api/tauri", () => ({
  jiraConnection: connection,
  jiraLinks: links,
  jiraFind: find,
  jiraIssues: issues,
  jiraAssigned: assigned,
}));
vi.mock("@/api/hooks", () => ({ useWorktrees: () => worktrees }));
vi.mock("@/lib/useIsMobile", () => ({ useIsMobile: () => false }));

import { JiraProvider } from "./JiraContext";
import { JiraBrowser } from "./JiraBrowser";
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
  jira_status_expectations: {},
  jira_chips_on_prs: true,
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

/// A link as the desktop's index reports it: pull request #7 names ABC-1
/// in its title.
const prLink = (key = "ABC-1", number = 7): JiraLink => ({
  key,
  found_in: "title",
  current: true,
  target: {
    kind: "pr",
    source: { provider: "github", host: "github.com" },
    role: "authored",
    repo: "octocat/hello-world",
    number,
    title: `${key}: fix login`,
    url: `https://github.com/octocat/hello-world/pull/${number}`,
    head_ref: "abc-1-fix-login",
    is_draft: false,
    state: null,
    author: null,
  },
});
const index = (found: JiraLink[]): LinkIndex => ({ links: found, projects: ["ABC"], error: null });

const EPIC = issue("ABC-2", { summary: "Login epic", issue_type: { name: "Epic", hierarchy_level: 1 } });
const STORY = issue("ABC-1", { parent_key: "ABC-2" });
const TODO = issue("ABC-3", { summary: "Not started", status: { name: "To Do", category: "new" } });

beforeEach(() => {
  connection.mockReset().mockResolvedValue({ state: "logged_in", site: "jira.example.internal", account: null });
  links.mockReset().mockResolvedValue(index([prLink()]));
  // As the desktop answers: asked and not returned is `missing`.
  issues.mockReset().mockImplementation((keys) => {
    const found = [EPIC, STORY].filter((i) => keys.includes(i.key));
    return Promise.resolve(batch(found, { missing: keys.filter((k) => !found.some((i) => i.key === k)) }));
  });
  assigned.mockReset().mockResolvedValue(batch([TODO]));
  find.mockReset().mockImplementation((key) => Promise.resolve({ key, links: [], search: { state: "not_asked", reason: "already known from your pull requests" } }));
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

describe("My Jira issues", () => {
  it("groups a linked story under its epic, beside assigned work with no pull request", async () => {
    mount(<JiraBrowser />);
    const epic = await screen.findByRole("button", { name: /ABC-2 Login epic/ });
    expect(epic).toBeTruthy();
    expect(await screen.findByRole("button", { name: /ABC-1 Summary of ABC-1/ })).toBeTruthy();
    expect(await screen.findByRole("button", { name: /ABC-3 Not started/ })).toBeTruthy();
    expect(screen.getByText("No epic")).toBeTruthy();
    await quiet();
  });

  it("filters to issues with pull requests, and to assigned ones", async () => {
    mount(<JiraBrowser />);
    await screen.findByRole("button", { name: /ABC-3 Not started/ });
    await screen.findByRole("button", { name: /ABC-1 Summary/ });
    fireEvent.click(screen.getByRole("button", { name: "With PRs" }));
    expect(screen.queryByRole("button", { name: /ABC-3/ })).toBeNull();
    expect(await screen.findByRole("button", { name: /ABC-1 Summary/ })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Assigned" }));
    expect(screen.queryByRole("button", { name: /ABC-1 Summary/ })).toBeNull();
    expect(await screen.findByRole("button", { name: /ABC-3 Not started/ })).toBeTruthy();
    await quiet();
  });

  it("shows a selected issue's pull requests and opens one in Headstate", async () => {
    useJiraView.setState({ selected: "ABC-1" });
    mount(<JiraPage />);
    expect(await screen.findByRole("heading", { name: /ABC-1 Summary of ABC-1/ })).toBeTruthy();
    const prs = screen.getByRole("region", { name: "Pull requests" });
    expect(within(prs).getByText(/from the title/)).toBeTruthy();
    fireEvent.click(within(prs).getByRole("button", { name: "Open in Headstate" }));
    expect(useFilters.getState().view).toBe("my-prs");
    expect(useFilters.getState().selectedPr).toMatchObject({ repo: "octocat/hello-world", number: 7 });
    await quiet();
  });

  it("says plainly when nothing of yours names the selected issue", async () => {
    useJiraView.setState({ selected: "ABC-3" });
    mount(<JiraPage />);
    expect(await screen.findByText("None of your pull requests or worktrees names ABC-3.")).toBeTruthy();
    await quiet();
  });

  it("explains, rather than lists nothing, when Jira is not connected", async () => {
    connection.mockResolvedValue({ state: "not_logged_in", detail: "" });
    mount(<JiraBrowser />);
    expect(await screen.findByText(/Jira is not connected/)).toBeTruthy();
    expect(issues).not.toHaveBeenCalled();
    await quiet();
  });

  it("explains how linking works when no key is found and nothing is assigned", async () => {
    assigned.mockResolvedValue(batch([]));
    links.mockResolvedValue(index([]));
    mount(<JiraBrowser />);
    expect(await screen.findByText(/none of your open pull requests or worktrees name one/)).toBeTruthy();
    await quiet();
  });

  it("lists only issues that disagree with an open pull request under Needs attention", async () => {
    const stale = issue("ABC-1", { parent_key: "ABC-2", status: { name: "To Do", category: "new" } });
    issues.mockImplementation((keys) => Promise.resolve(batch([EPIC, stale].filter((i) => keys.includes(i.key)))));
    mount(<JiraBrowser />);
    await screen.findByRole("button", { name: /ABC-1 Summary/ });
    fireEvent.click(screen.getByRole("button", { name: "Needs attention" }));
    expect(await screen.findByRole("button", { name: /ABC-1 Summary/ })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /ABC-3/ })).toBeNull();
    await quiet();
  });

  /// Kept by the desktop after it leaves the open list (#2041): listed,
  /// said to be no longer open, and never a drift warning.
  it("lists a pull request that has left the open list as no longer open", async () => {
    const stale = issue("ABC-1", { status: { name: "To Do", category: "new" } });
    issues.mockImplementation((keys) => Promise.resolve(batch([stale].filter((i) => keys.includes(i.key)))));
    links.mockResolvedValue(index([{ ...prLink(), current: false }]));
    useJiraView.setState({ selected: "ABC-1" });
    mount(<JiraPage />);
    const prs = await screen.findByRole("region", { name: "Pull requests" });
    expect(within(prs).getByText(/no longer open/)).toBeTruthy();
    expect(within(prs).queryByRole("status")).toBeNull();
    await quiet();
  });

  /// #2041: a teammate's merged pull request, found by the desktop, and
  /// the ticket it left behind (#2042 R2).
  it("lists a merged pull request found on GitHub, and says the ticket did not move", async () => {
    links.mockResolvedValue(index([]));
    find.mockResolvedValue({
      key: "ABC-1",
      links: [
        {
          ...prLink(),
          current: false,
          target: { ...(prLink().target as Extract<JiraLink["target"], { kind: "pr" }>), role: "searched", number: 9, url: "https://github.com/octocat/hello-world/pull/9", state: "merged", author: "dana" },
        },
      ],
      search: { state: "asked", at: "2026-10-28T12:00:00Z", total: 1, truncated: false },
    });
    useJiraView.setState({ selected: "ABC-1" });
    mount(<JiraPage />);
    const region = await screen.findByRole("region", { name: "Found on GitHub" });
    expect(await within(region).findByText(/octocat\/hello-world#9 · merged · dana/)).toBeTruthy();
    expect(await within(region).findByText("A pull request for ABC-1 has merged, but the ticket is still In Progress.")).toBeTruthy();
    await quiet();
  });

  it("searches GitHub only when asked, once the database already knew", async () => {
    useJiraView.setState({ selected: "ABC-1" });
    mount(<JiraPage />);
    fireEvent.click(await screen.findByRole("button", { name: "Search GitHub" }));
    await quiet();
    expect(find).toHaveBeenCalledWith("ABC-1", true);
    expect(find).toHaveBeenCalledWith("ABC-1", false);
  });

  describe("Go to ticket", () => {
    const type = async (text: string) => {
      const box = await screen.findByRole("searchbox", { name: "Go to ticket" });
      fireEvent.change(box, { target: { value: text } });
      fireEvent.keyDown(box, { key: "Enter" });
    };

    it("opens the one pull request that names the key", async () => {
      useFilters.getState().setView("jira");
      mount(<JiraBrowser />);
      await type("abc-1");
      expect(useFilters.getState().view).toBe("my-prs");
      expect(useFilters.getState().selectedPr).toMatchObject({ number: 7 });
      await quiet();
    });

    /// A key nothing of yours names still opens, and resolves -- it must
    /// not read "Loading…" forever because it is not in the tree.
    it("opens an issue no pull request names, and resolves it", async () => {
      mount(
        <>
          <JiraBrowser />
          <JiraPage />
        </>,
      );
      await type("https://jira.example.internal/browse/ABC-9");
      expect(useJiraView.getState().selected).toBe("ABC-9");
      expect(await screen.findByRole("heading", { name: /ABC-9 Cannot view this issue/ })).toBeTruthy();
      expect(screen.getByText("None of your pull requests or worktrees names ABC-9.")).toBeTruthy();
      await quiet();
    });

    it.each([
      ["XYZ-1", "XYZ is not one of your Jira projects."],
      ["hello", "Type a Jira key such as ABC-1, or paste a link to an issue."],
    ])("explains %j instead of going nowhere", async (text, message) => {
      mount(<JiraBrowser />);
      await type(text);
      expect(screen.getByText(message)).toBeTruthy();
      expect(useJiraView.getState().selected).toBeNull();
      await quiet();
    });
  });
});
