import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UiPrefs } from "@/api/tauri";
import type { IssueBatch, JiraConnection, JiraIssue, JiraLink, LinkIndex } from "@/types/jira";
import { renderWithQuery } from "@/test-utils";
import { createQueryClient } from "@/api/queryClient";
import { QueryClientProvider } from "@tanstack/react-query";
import { useFilters } from "@/store/filters";
import { useJiraView } from "@/store/jira";

const connection = vi.hoisted(() => vi.fn<() => Promise<JiraConnection>>());
const links = vi.hoisted(() => vi.fn<() => Promise<LinkIndex>>());
const issues = vi.hoisted(() => vi.fn<(keys: string[]) => Promise<IssueBatch>>());
vi.mock("@/api/tauri", () => ({ jiraConnection: connection, jiraLinks: links, jiraIssues: issues }));

import { JiraProvider } from "./JiraContext";
import { PrRowTicketChips, TicketChips, TicketDriftNotices } from "./TicketChips";

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
  jira_status_expectations: {},
  jira_chips_on_prs: true,
  jira_branch_template: "",
  jira_no_ticket_marker: false,
  jira_merge_grace_mins: 15,
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
  links.mockReset().mockResolvedValue(index([]));
  issues.mockReset();
});
afterEach(cleanup);

const URL = "https://github.com/octocat/hello-world/pull/7";
const ROW = { prUrl: URL };

/// What the desktop's index says the pull request at `URL` names.
const prLink = (key: string, found_in: "title" | "branch" = "title"): JiraLink => ({
  key,
  found_in,
  current: true,
  target: {
    kind: "pr",
    source: { provider: "github", host: "github.com" },
    role: "authored",
    repo: "octocat/hello-world",
    number: 7,
    title: "t",
    url: URL,
    head_ref: "b",
    is_draft: false,
    state: null,
    author: null,
  },
});
const index = (found: JiraLink[], over: Partial<LinkIndex> = {}): LinkIndex => ({
  links: found,
  projects: ["ABC", "XY"],
  error: null,
  ...over,
});

const mount = (p: UiPrefs, found: JiraLink[]) => {
  links.mockResolvedValue(index(found));
  return renderWithQuery(
    <JiraProvider prefs={p}>
      <TicketChips row={ROW} />
    </JiraProvider>,
  );
};

describe("TicketChips", () => {
  it("renders nothing and asks nothing without a provider", () => {
    const { container } = render(<TicketChips row={ROW} />);
    expect(container.textContent).toBe("");
    expect(connection).not.toHaveBeenCalled();
  });

  it("asks nothing while Jira is switched off", async () => {
    const { container } = mount(prefs({ jira_enabled: false }), [prLink("ABC-1")]);
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent).toBe("");
    expect(connection).not.toHaveBeenCalled();
    expect(issues).not.toHaveBeenCalled();
  });

  /// One desktop call for every key on screen, not one per chip.
  it("batches the title's and the branch's keys into one call", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("ABC-1")], missing: ["XY-2"] }));
    mount(prefs(), [prLink("ABC-1"), prLink("XY-2", "branch")]);
    expect(await screen.findByText("ABC-1 · In Review")).toBeTruthy();
    expect(await screen.findByText("XY-2 · ?")).toBeTruthy();
    expect(issues).toHaveBeenCalledTimes(1);
    expect(issues.mock.calls[0][0].sort()).toEqual(["ABC-1", "XY-2"]);
  });

  it("says a key the desktop never reached is not checked yet, not missing", async () => {
    issues.mockResolvedValue(batch({ unasked: ["ABC-3"] }));
    mount(prefs(), [prLink("ABC-3")]);
    expect(await screen.findByText("ABC-3 · not checked yet")).toBeTruthy();
  });

  /// A rate limit is not a failure and not "missing": the chip says so,
  /// and its title says when Headstate will ask again.
  it("shows a rate-limited key as unasked, with when it will be retried", async () => {
    // The loader holds off for the limit's length after this reply. Pin
    // the clock so the hold ends, for later tests, when it is restored.
    const now = vi.spyOn(Date, "now").mockReturnValue(1_000);
    try {
      issues.mockResolvedValue(batch({ unasked: ["ABC-5"], error: { state: "rate_limited", retry_after_secs: 42 } }));
      mount(prefs(), [prLink("ABC-5")]);
      const chip = await screen.findByText("ABC-5 · rate limited");
      expect(chip.getAttribute("title")).toMatch(/not been checked yet\. .*ask again in 42s/);
    } finally {
      now.mockRestore();
    }
  });

  it("shows a failed read as a failure", async () => {
    issues.mockRejectedValue("acli did not respond within 20s");
    mount(prefs(), [prLink("ABC-4")]);
    expect(await screen.findByText("ABC-4 · not loaded")).toBeTruthy();
  });

  /// Without the project list a key cannot be told from a look-alike.
  it("renders nothing when the project list could not be read", async () => {
    links.mockResolvedValue(index([], { projects: [], error: { state: "failed", detail: "unauthorized" } }));
    const { container } = renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <TicketChips row={ROW} />
      </JiraProvider>,
    );
    await waitFor(() => expect(links).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent).toBe("");
    expect(issues).not.toHaveBeenCalled();
  });

  it("renders nothing for a row the index has no keys for", async () => {
    links.mockResolvedValue(index([prLink("ABC-6")]));
    const { container } = renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <TicketChips row={{ prUrl: "https://github.com/octocat/hello-world/pull/8" }} />
      </JiraProvider>,
    );
    await waitFor(() => expect(links).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent).toBe("");
    expect(issues).not.toHaveBeenCalled();
  });

  it("finds a worktree's keys by its path", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("XY-8", "To Do")] }));
    links.mockResolvedValue(
      index([{ key: "XY-8", found_in: "branch", current: true, target: { kind: "worktree", repo: "r", path: "/w/xy-8", branch: "xy-8-spike" } }]),
    );
    renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <TicketChips row={{ worktreePath: "/w/xy-8" }} />
      </JiraProvider>,
    );
    expect(await screen.findByText("XY-8 · To Do")).toBeTruthy();
  });

  /// #2042: an open pull request whose ticket has not started is marked.
  it("marks a ticket that is still To Do on an open pull request, and says why", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("ABC-7", "Backlog", "new")] }));
    links.mockResolvedValue(index([prLink("ABC-7")]));
    renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <TicketChips row={ROW} openPr={{ draft: false }} />
        <TicketDriftNotices prUrl={URL} draft={false} />
      </JiraProvider>,
    );
    expect(await screen.findByLabelText("Needs attention")).toBeTruthy();
    expect(await screen.findByText("This pull request is open, but ABC-7 is still Backlog.")).toBeTruthy();
  });

  it("does not mark a draft, or a row that is not an open pull request", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("ABC-8", "To Do", "new")] }));
    links.mockResolvedValue(
      index([
        prLink("ABC-8"),
        { key: "ABC-8", found_in: "branch", current: true, target: { kind: "branch", repo_path: "/r", branch: "abc-8-wip" } },
      ]),
    );
    renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <TicketChips row={ROW} openPr={{ draft: true }} />
        <TicketChips row={{ repoPath: "/r", branch: "abc-8-wip" }} />
        <TicketDriftNotices prUrl={URL} draft={true} />
      </JiraProvider>,
    );
    expect(await screen.findAllByText("ABC-8 · To Do")).toHaveLength(2);
    expect(screen.queryByLabelText("Needs attention")).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
  });

  /// Settings › Jira › "Show tickets on pull request rows" (#2034).
  it("shows chips on PR rows by default, and none when switched off -- other rows keep theirs", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("ABC-5")] }));
    links.mockResolvedValue(
      index([
        prLink("ABC-5"),
        { key: "ABC-5", found_in: "branch", current: true, target: { kind: "branch", repo_path: "/r", branch: "abc-5-x" } },
      ]),
    );
    const view = renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <PrRowTicketChips prUrl={URL} draft={false} />
      </JiraProvider>,
    );
    expect(await screen.findByText("ABC-5 · In Review")).toBeTruthy();
    view.unmount();

    renderWithQuery(
      <JiraProvider prefs={prefs({ jira_chips_on_prs: false })}>
        <div data-testid="pr-row">
          <PrRowTicketChips prUrl={URL} draft={false} />
        </div>
        <TicketChips row={{ repoPath: "/r", branch: "abc-5-x" }} />
        <TicketDriftNotices prUrl={URL} draft={false} />
      </JiraProvider>,
    );
    expect(await screen.findByText("ABC-5 · In Review")).toBeTruthy();
    expect(screen.getByTestId("pr-row").textContent).toBe("");
  });

  /// The project's expectation from Settings reaches the chip (#2034).
  it("marks a ticket against its project's status expectation", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("ABC-6", "In Progress")] }));
    links.mockResolvedValue(index([prLink("ABC-6")]));
    renderWithQuery(
      <JiraProvider prefs={prefs({ jira_status_expectations: { ABC: { on_open: { kind: "status", name: "In Review" } } } })}>
        <TicketDriftNotices prUrl={URL} draft={false} />
      </JiraProvider>,
    );
    expect(await screen.findByText("This pull request is open, but ABC-6 is In Progress, not In Review.")).toBeTruthy();
  });

  /// #2036: how old a status is, in its tooltip -- and nothing when the
  /// desktop did not say. Absent is not zero.
  it("says how old a status is, and says nothing when that is not known", async () => {
    const readAt = new Date(Date.now() - 2 * 60_000).toISOString();
    issues.mockResolvedValue(batch({ issues: [{ ...issue("ABC-2"), read_at: readAt }, issue("XY-2")] }));
    links.mockResolvedValue(index([prLink("ABC-2"), prLink("XY-2")]));
    renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <TicketChips row={ROW} />
      </JiraProvider>,
    );
    const aged = await screen.findByRole("button", { name: /ABC-2 · In Review/ });
    expect(aged.getAttribute("title")).toMatch(/Status read 2 minutes ago\.$/);
    const unknown = await screen.findByRole("button", { name: /XY-2 · In Review/ });
    expect(unknown.getAttribute("title")).not.toMatch(/read|ago/);
    expect(screen.queryByLabelText("Not current")).toBeNull();
  });

  /// #2036: a row the desktop served from its cache after a failed read
  /// is marked, with the age of the answer shown.
  it("marks a status served stale by the desktop", async () => {
    const readAt = new Date(Date.now() - 9 * 60_000).toISOString();
    issues.mockResolvedValue(batch({ issues: [{ ...issue("ABC-3"), read_at: readAt, stale: true }], error: { state: "timed_out", secs: 20 } }));
    links.mockResolvedValue(index([prLink("ABC-3")]));
    renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <TicketChips row={ROW} />
      </JiraProvider>,
    );
    const chip = await screen.findByRole("button", { name: /ABC-3 · In Review/ });
    expect(screen.getByLabelText("Not current")).toBeTruthy();
    expect(chip.getAttribute("title")).toMatch(/Shown as read 9 minutes ago: the latest read failed\./);
  });

  /// #2036: a refetch that rejects keeps the status already shown, marked,
  /// instead of turning it into "not loaded".
  it("keeps a loaded status when a refetch fails", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("ABC-4")] }));
    links.mockResolvedValue(index([prLink("ABC-4")]));
    const client = createQueryClient();
    render(
      <QueryClientProvider client={client}>
        <JiraProvider prefs={prefs()}>
          <TicketChips row={ROW} />
        </JiraProvider>
      </QueryClientProvider>,
    );
    await screen.findByRole("button", { name: /ABC-4 · In Review/ });
    issues.mockRejectedValue(new Error("acli exploded"));
    await client.refetchQueries({ queryKey: ["jira-issue", "ABC-4"] });
    await waitFor(() => expect(screen.getByLabelText("Not current")).toBeTruthy());
    const chip = screen.getByRole("button", { name: /ABC-4 · In Review/ });
    expect(chip.getAttribute("title")).toMatch(/the latest read failed \(acli exploded\)/);
    expect(screen.queryByText(/not loaded/)).toBeNull();
  });

  it("opens the issue in My Jira issues, without also opening the row", async () => {
    issues.mockResolvedValue(batch({ issues: [issue("ABC-9")] }));
    links.mockResolvedValue(index([prLink("ABC-9")]));
    const row = vi.fn();
    renderWithQuery(
      <JiraProvider prefs={prefs()}>
        <div onClick={row}>
          <TicketChips row={ROW} />
        </div>
      </JiraProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: /ABC-9 · In Review/ }));
    expect(useFilters.getState().view).toBe("jira");
    expect(useJiraView.getState().selected).toBe("ABC-9");
    expect(row).not.toHaveBeenCalled();
  });
});
