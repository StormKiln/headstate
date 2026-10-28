import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UiPrefs } from "@/api/tauri";
import type { IssueBatch, JiraConnection, JiraIssue, ProjectList } from "@/types/jira";
import { renderWithQuery } from "@/test-utils";
import { useFilters } from "@/store/filters";
import { useJiraView } from "@/store/jira";

const connection = vi.hoisted(() => vi.fn<() => Promise<JiraConnection>>());
const projects = vi.hoisted(() => vi.fn<() => Promise<ProjectList>>());
const issues = vi.hoisted(() => vi.fn<(keys: string[]) => Promise<IssueBatch>>());
vi.mock("@/api/tauri", () => ({ jiraConnection: connection, jiraProjects: projects, jiraIssues: issues }));

import { JiraProvider } from "./JiraContext";
import { TicketChips, TicketDriftNotices } from "./TicketChips";

const prefs = (over: Partial<UiPrefs> = {}): UiPrefs => ({
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
  ...over,
});

const issue = (key: string, status = "In Review", category: JiraIssue["status"]["category"] = "in_progress"): JiraIssue => ({
  key,
  summary: `Summary of ${key}`,
  issue_type: { name: "Story", hierarchy_level: 0 },
  status: { name: status, category },
  assignee: null,
  parent_key: null,
  parent_checked: true,
  project_key: key.split("-")[0],
  url: `https://jira.example.internal/browse/${key}`,
  updated: null,
});

const batch = (over: Partial<IssueBatch>): IssueBatch => ({
  issues: [],
  missing: [],
  unasked: [],
  error: null,
  truncated: false,
  ...over,
});

beforeEach(() => {
  connection.mockReset().mockResolvedValue({ state: "logged_in", site: "jira.example.internal", account: null });
  projects.mockReset().mockResolvedValue({
    projects: [
      { key: "ABC", name: "Alpha" },
      { key: "XY", name: "Ex" },
    ],
    error: null,
  });
  issues.mockReset();
});
afterEach(cleanup);

const mount = (p: UiPrefs, title: string | undefined, branch: string | null) =>
  renderWithQuery(
    <JiraProvider prefs={p}>
      <TicketChips title={title} branch={branch} />
    </JiraProvider>,
  );

describe("TicketChips", () => {
  it("renders nothing and asks nothing without a provider", () => {
    const { container } = render(<TicketChips title="ABC-1 fix" branch="abc-1" />);
    expect(container.textContent).toBe("");
    expect(connection).not.toHaveBeenCalled();
  });

  it("asks nothing while Jira is switched off", async () => {
    const { container } = mount(prefs({ jira_enabled: false }), "ABC-1 fix", null);
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent).toBe("");
    expect(connection).not.toHaveBeenCalled();
    expect(issues).not.toHaveBeenCalled();
  });

  /// One desktop call for every key on screen, not one per chip.
  it("batches the title's and the branch's keys into one call", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("ABC-1")], missing: ["XY-2"] }));
    mount(prefs(), "ABC-1: fix login", "feature/xy-2-follow-up");
    expect(await screen.findByText("ABC-1 · In Review")).toBeTruthy();
    expect(await screen.findByText("XY-2 · ?")).toBeTruthy();
    expect(issues).toHaveBeenCalledTimes(1);
    expect(issues.mock.calls[0][0].sort()).toEqual(["ABC-1", "XY-2"]);
  });

  it("says a key the desktop never reached is not checked yet, not missing", async () => {
    issues.mockResolvedValue(batch({ unasked: ["ABC-3"] }));
    mount(prefs(), "ABC-3 fix", null);
    expect(await screen.findByText("ABC-3 · not checked yet")).toBeTruthy();
  });

  it("shows a failed read as a failure", async () => {
    issues.mockRejectedValue("acli did not respond within 20s");
    mount(prefs(), "ABC-4 fix", null);
    expect(await screen.findByText("ABC-4 · not loaded")).toBeTruthy();
  });

  /// Without the project list a key cannot be told from a look-alike.
  it("renders nothing when the project list could not be read", async () => {
    projects.mockResolvedValue({ projects: [], error: "unauthorized" });
    const { container } = mount(prefs(), "ABC-5 fix", null);
    await waitFor(() => expect(projects).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent).toBe("");
    expect(issues).not.toHaveBeenCalled();
  });

  it("skips excluded projects and look-alikes", async () => {
    const { container } = mount(prefs({ jira_excluded_projects: ["ABC"] }), "ABC-6 and UTF-8", null);
    await waitFor(() => expect(projects).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent).toBe("");
    expect(issues).not.toHaveBeenCalled();
  });

  it("finds a key in a worktree's branch alone", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("XY-8", "To Do")] }));
    mount(prefs(), undefined, "xy-8-spike");
    expect(await screen.findByText("XY-8 · To Do")).toBeTruthy();
  });

  /// #2042: an open pull request whose ticket has not started is marked.
  it("marks a ticket that is still To Do on an open pull request, and says why", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("ABC-7", "Backlog", "new")] }));
    renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <TicketChips title="ABC-7 fix" branch={null} openPr={{ draft: false }} />
        <TicketDriftNotices title="ABC-7 fix" branch="" draft={false} />
      </JiraProvider>,
    );
    expect(await screen.findByLabelText("Needs attention")).toBeTruthy();
    expect(await screen.findByText("This pull request is open, but ABC-7 is still Backlog.")).toBeTruthy();
  });

  it("does not mark a draft, or a row that is not an open pull request", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("ABC-8", "To Do", "new")] }));
    renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <TicketChips title="ABC-8 fix" branch={null} openPr={{ draft: true }} />
        <TicketChips branch="abc-8-wip" />
        <TicketDriftNotices title="ABC-8 fix" branch="" draft={true} />
      </JiraProvider>,
    );
    expect(await screen.findAllByText("ABC-8 · To Do")).toHaveLength(2);
    expect(screen.queryByLabelText("Needs attention")).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("opens the issue in My Jira issues, without also opening the row", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("ABC-9")] }));
    const row = vi.fn();
    renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <div onClick={row}>
          <TicketChips title="ABC-9 fix" branch={null} />
        </div>
      </JiraProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: /ABC-9 · In Review/ }));
    expect(useFilters.getState().view).toBe("jira");
    expect(useJiraView.getState().selected).toBe("ABC-9");
    expect(row).not.toHaveBeenCalled();
  });
});
