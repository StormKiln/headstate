import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UiPrefs } from "@/api/tauri";
import type { IssueBatch, JiraConnection, JiraIssue, ProjectList } from "@/types/jira";
import { renderWithQuery } from "@/test-utils";
import { useFilters } from "@/store/filters";
import { useJiraView } from "@/store/jira";

const connection = vi.hoisted(() => vi.fn<() => Promise<JiraConnection>>());
const projects = vi.hoisted(() => vi.fn<() => Promise<ProjectList>>());
const issues = vi.hoisted(() => vi.fn<(keys: string[]) => Promise<IssueBatch>>());
const assigned = vi.hoisted(() => vi.fn<() => Promise<IssueBatch>>());
const worktrees = vi.hoisted(() => ({ data: [] as unknown[], isPending: false }));
vi.mock("@/api/tauri", () => ({
  jiraConnection: connection,
  jiraProjects: projects,
  jiraIssues: issues,
  jiraAssigned: assigned,
}));
vi.mock("@/api/hooks", () => ({ useWorktrees: () => worktrees }));
vi.mock("@/lib/useIsMobile", () => ({ useIsMobile: () => false }));

import { JiraProvider } from "./JiraContext";
import { JiraBrowser } from "./JiraBrowser";
import { JiraPage } from "./JiraPage";
import type { BoardPr } from "./useJiraBoard";

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

const pr: BoardPr = {
  source: { provider: "github", host: "github.com" },
  repo: "octocat/hello-world",
  number: 7,
  title: "ABC-1: fix login",
  url: "https://github.com/octocat/hello-world/pull/7",
  is_draft: false,
  head_ref: "abc-1-fix-login",
};

const EPIC = issue("ABC-2", { summary: "Login epic", issue_type: { name: "Epic", hierarchy_level: 1 } });
const STORY = issue("ABC-1", { parent_key: "ABC-2" });
const TODO = issue("ABC-3", { summary: "Not started", status: { name: "To Do", category: "new" } });

beforeEach(() => {
  connection.mockReset().mockResolvedValue({ state: "logged_in", site: "jira.example.internal", account: null });
  projects.mockReset().mockResolvedValue({ projects: [{ key: "ABC", name: "Alpha" }], error: null });
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

describe("My Jira issues", () => {
  it("groups a linked story under its epic, beside assigned work with no pull request", async () => {
    mount(<JiraBrowser prs={[pr]} />);
    const epic = await screen.findByRole("button", { name: /ABC-2 Login epic/ });
    expect(epic).toBeTruthy();
    expect(await screen.findByRole("button", { name: /ABC-1 Summary of ABC-1/ })).toBeTruthy();
    expect(await screen.findByRole("button", { name: /ABC-3 Not started/ })).toBeTruthy();
    expect(screen.getByText("No epic")).toBeTruthy();
    await quiet();
  });

  it("filters to issues with pull requests, and to assigned ones", async () => {
    mount(<JiraBrowser prs={[pr]} />);
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
    mount(<JiraPage prs={[pr]} />);
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
    mount(<JiraPage prs={[pr]} />);
    expect(await screen.findByText("None of your open pull requests or worktrees names ABC-3.")).toBeTruthy();
    await quiet();
  });

  it("explains, rather than lists nothing, when Jira is not connected", async () => {
    connection.mockResolvedValue({ state: "not_logged_in", detail: "" });
    mount(<JiraBrowser prs={[pr]} />);
    expect(await screen.findByText(/Jira is not connected/)).toBeTruthy();
    expect(issues).not.toHaveBeenCalled();
    await quiet();
  });

  it("explains how linking works when no key is found and nothing is assigned", async () => {
    assigned.mockResolvedValue(batch([]));
    mount(<JiraBrowser prs={[{ ...pr, title: "Fix login", head_ref: "fix-login" }]} />);
    expect(await screen.findByText(/none of your open pull requests or worktrees name one/)).toBeTruthy();
    await quiet();
  });

  it("lists only issues that disagree with an open pull request under Needs attention", async () => {
    const stale = issue("ABC-1", { parent_key: "ABC-2", status: { name: "To Do", category: "new" } });
    issues.mockImplementation((keys) => Promise.resolve(batch([EPIC, stale].filter((i) => keys.includes(i.key)))));
    mount(<JiraBrowser prs={[pr, { ...pr, number: 8, title: "No ticket", head_ref: "chore" }]} />);
    await screen.findByRole("button", { name: /ABC-1 Summary/ });
    fireEvent.click(screen.getByRole("button", { name: "Needs attention" }));
    expect(await screen.findByRole("button", { name: /ABC-1 Summary/ })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /ABC-3/ })).toBeNull();
    await quiet();
  });

  describe("Go to ticket", () => {
    const type = async (text: string) => {
      const box = await screen.findByRole("searchbox", { name: "Go to ticket" });
      fireEvent.change(box, { target: { value: text } });
      fireEvent.keyDown(box, { key: "Enter" });
    };

    it("opens the one pull request that names the key", async () => {
      useFilters.getState().setView("jira");
      mount(<JiraBrowser prs={[pr]} />);
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
          <JiraBrowser prs={[pr]} />
          <JiraPage prs={[pr]} />
        </>,
      );
      await type("https://jira.example.internal/browse/ABC-9");
      expect(useJiraView.getState().selected).toBe("ABC-9");
      expect(await screen.findByRole("heading", { name: /ABC-9 Cannot view this issue/ })).toBeTruthy();
      expect(screen.getByText("None of your open pull requests or worktrees names ABC-9.")).toBeTruthy();
      await quiet();
    });

    it.each([
      ["XYZ-1", "XYZ is not one of your Jira projects."],
      ["hello", "Type a Jira key such as ABC-1, or paste a link to an issue."],
    ])("explains %j instead of going nowhere", async (text, message) => {
      mount(<JiraBrowser prs={[pr]} />);
      await type(text);
      expect(screen.getByText(message)).toBeTruthy();
      expect(useJiraView.getState().selected).toBeNull();
      await quiet();
    });
  });
});
