import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createQueryClient } from "@/api/queryClient";
import type { IssueState } from "@/api/jira";
import type { JiraIssue, ProjectStatuses, TransitionResult } from "@/types/jira";

const transition = vi.hoisted(() => vi.fn<(key: string, status: string) => Promise<TransitionResult>>());
const statuses = vi.hoisted(() => vi.fn<(project: string) => Promise<ProjectStatuses>>());
const conn = vi.hoisted(() => ({ current: { kind: "local" } as Record<string, unknown> }));
vi.mock("@/api/tauri", () => ({ jiraTransition: transition, jiraProjectStatuses: statuses }));
vi.mock("@/api/connection", () => ({ useConnectionState: () => conn.current }));

import { JiraTransition } from "./JiraTransition";

const issue = (status: string): JiraIssue => ({
  key: "ABC-1",
  summary: "Fix login",
  issue_type: { name: "Story", hierarchy_level: 0 },
  status: { name: status, category: status === "To Do" ? "new" : "in_progress" },
  assignee: null,
  parent_key: null,
  parent_checked: true,
  project_key: "ABC",
  url: "https://jira.example.internal/browse/ABC-1",
  updated: null,
});

const seen: ProjectStatuses = {
  project: "ABC",
  statuses: [
    { name: "To Do", category: "new" },
    { name: "In Progress", category: "in_progress" },
    { name: "In Review", category: "in_progress" },
  ],
  truncated: false,
  error: null,
};

function mount(ui: React.ReactElement) {
  const client = createQueryClient();
  render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
  return client;
}

/// The argv words as the preview boxes them, in order.
const argvShown = () =>
  within(screen.getByLabelText("Command to run"))
    .getAllByText(/.*/, { selector: "code" })
    .map((c) => c.textContent);

describe("JiraTransition", () => {
  beforeEach(() => {
    transition.mockReset();
    statuses.mockReset().mockResolvedValue(seen);
    conn.current = { kind: "local" };
  });
  afterEach(cleanup);

  it("offers the statuses seen, says they are not the allowed moves, and confirms the exact argv first", async () => {
    mount(<JiraTransition issue={issue("To Do")} site={null} />);
    fireEvent.click(screen.getByRole("button", { name: "Move to…" }));
    expect(screen.getByText(/cannot see which moves ABC's workflow allows from To Do/)).toBeTruthy();
    await screen.findByRole("button", { name: "In Review" });
    expect(screen.queryByRole("button", { name: "To Do" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "In Review" }));

    expect(argvShown()).toEqual([
      "acli",
      "jira",
      "workitem",
      "transition",
      "--key",
      "ABC-1",
      "--status",
      "In Review",
      "--yes",
      "--json",
    ]);
    expect(transition).not.toHaveBeenCalled();
    // On the desktop itself there is no machine to name.
    expect(screen.queryByText(/not on this/)).toBeNull();
  });

  it("shows the status Jira reports after the move, and every surface holds it", async () => {
    transition.mockResolvedValue({
      key: "ABC-1",
      requested: "In Review",
      reply: { state: "accepted", message: "Work item ABC-1 has been successfully transitioned to In Review" },
      after: issue("In Testing"),
      reread_error: null,
    });
    const onDone = vi.fn();
    const client = mount(<JiraTransition issue={issue("To Do")} site={null} target="In Review" onDone={onDone} />);
    fireEvent.click(screen.getByRole("button", { name: "Move ABC-1 to In Review" }));
    fireEvent.click(screen.getByRole("button", { name: "Move to In Review" }));
    await screen.findByText("Jira accepted the move to In Review, but now reports ABC-1 as In Testing.");
    expect(transition).toHaveBeenCalledWith("ABC-1", "In Review");
    expect(onDone).toHaveBeenCalledOnce();
    const held = client.getQueryData<IssueState>(["jira-issue", "ABC-1"]);
    expect(held?.state === "found" && held.issue.status.name).toBe("In Testing");
    expect(screen.getByRole("link", { name: "Open in Jira" })).toBeTruthy();
  });

  it("shows Jira's reason for a refusal and offers to finish it in Jira", async () => {
    transition.mockResolvedValue({
      key: "ABC-1",
      requested: "Done",
      reply: { state: "refused", reason: "Resolution is required." },
      after: issue("To Do"),
      reread_error: null,
    });
    mount(<JiraTransition issue={issue("To Do")} site={null} target="Done" />);
    fireEvent.click(screen.getByRole("button", { name: "Move ABC-1 to Done" }));
    fireEvent.click(screen.getByRole("button", { name: "Move to Done" }));
    await screen.findByText(
      "Jira refused to move ABC-1 to Done: Resolution is required. Jira reports it as To Do.",
    );
    expect(screen.getByRole("link", { name: "Open in Jira" }).getAttribute("href")).toBe(
      "https://jira.example.internal/browse/ABC-1",
    );
    expect(screen.queryByText(/now reports ABC-1 as Done/)).toBeNull();
  });

  it("lets a status no issue holds be typed", async () => {
    statuses.mockResolvedValue({ ...seen, statuses: [{ name: "To Do", category: "new" }] });
    mount(<JiraTransition issue={issue("To Do")} site={null} />);
    fireEvent.click(screen.getByRole("button", { name: "Move to…" }));
    await screen.findByText("No other status was seen in ABC's recent issues.");
    const next = screen.getByRole("button", { name: "Next" });
    expect((next as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByLabelText(/Another status/), { target: { value: "Blocked" } });
    fireEvent.click(next);
    expect(argvShown()).toContain("Blocked");
  });

  /// A phone write control names the desktop it runs on (#2047).
  it("names the desktop the move runs on when driven from a phone", () => {
    conn.current = { kind: "connected", desktop: "studio", lastPoll: null, protocolVersion: 2, stale: false };
    mount(<JiraTransition issue={issue("To Do")} site={null} target="In Progress" />);
    fireEvent.click(screen.getByRole("button", { name: "Move ABC-1 to In Progress" }));
    expect(screen.getByText(/not on this/).textContent).toContain("phone");
    expect(screen.getByText("studio")).toBeTruthy();
  });

  it("says why when the desktop sent nothing", async () => {
    transition.mockRejectedValue({ state: "not_asked" });
    mount(<JiraTransition issue={issue("To Do")} site={null} target="Done" />);
    fireEvent.click(screen.getByRole("button", { name: "Move ABC-1 to Done" }));
    fireEvent.click(screen.getByRole("button", { name: "Move to Done" }));
    await screen.findByText(/Could not move ABC-1: Jira is switched off in Settings, so it was not asked/);
  });
});
