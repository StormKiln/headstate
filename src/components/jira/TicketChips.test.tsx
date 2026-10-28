import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UiPrefs } from "@/api/tauri";
import type { IssueBatch, JiraConnection, JiraIssue, ProjectList } from "@/types/jira";
import { renderWithQuery } from "@/test-utils";

const connection = vi.hoisted(() => vi.fn<() => Promise<JiraConnection>>());
const projects = vi.hoisted(() => vi.fn<() => Promise<ProjectList>>());
const issues = vi.hoisted(() => vi.fn<(keys: string[]) => Promise<IssueBatch>>());
vi.mock("@/api/tauri", () => ({ jiraConnection: connection, jiraProjects: projects, jiraIssues: issues }));

import { JiraProvider } from "./JiraContext";
import { TicketChips } from "./TicketChips";

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

const issue = (key: string, status = "In Review"): JiraIssue => ({
  key,
  summary: `Summary of ${key}`,
  issue_type: { name: "Story", hierarchy_level: 0 },
  status: { name: status, category: "in_progress" },
  assignee: null,
  parent_key: null,
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
});
