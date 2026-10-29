import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { StartMode, StartPlan, Started } from "@/api/tauri";
import type { JiraIssue } from "@/types/jira";
import { renderWithQuery } from "@/test-utils";

const plan = vi.hoisted(() => vi.fn<(repo: string, branch: string, mode: StartMode) => Promise<StartPlan>>());
const start = vi.hoisted(() => vi.fn<(repo: string, branch: string, mode: StartMode, shown: string[]) => Promise<Started>>());
const worktrees = vi.hoisted(() => ({
  data: [{ name: "proj", path: "/code/proj", identity: "octocat/proj", worktrees: [] }] as unknown[],
  isPending: false,
  isError: false,
  error: null,
}));
vi.mock("@/api/tauri", () => ({ startWorkPlan: plan, startWork: start }));
vi.mock("@/api/hooks", () => ({ useWorktrees: () => worktrees }));

import { StartWork } from "./StartWork";

const ISSUE: JiraIssue = {
  key: "ABC-1",
  summary: "Fix login redirect",
  issue_type: { name: "Story", hierarchy_level: 0 },
  status: { name: "To Do", category: "new" },
  assignee: "Dana",
  parent_key: null,
  parent_checked: true,
  project_key: "ABC",
  url: null,
  updated: null,
};

const planFor = (branch: string, mode: StartMode, over: Partial<StartPlan> = {}): StartPlan => ({
  mode,
  branch,
  cwd: "/code/proj",
  program: "git",
  args:
    mode === "worktree"
      ? ["worktree", "add", "--no-track", "-b", branch, "--", `/code/proj-${branch}`, "origin/main"]
      : ["branch", "--no-track", "--", branch, "origin/main"],
  base: "origin/main",
  worktree_path: mode === "worktree" ? `/code/proj-${branch}` : null,
  keys: branch.toLowerCase().includes("abc-1") ? ["ABC-1"] : [],
  refusal: null,
  ...over,
});

beforeEach(() => {
  plan.mockReset().mockImplementation((_r, b, m) => Promise.resolve(planFor(b, m)));
  start.mockReset();
});
afterEach(cleanup);

const open = async () => {
  renderWithQuery(<StartWork issue={ISSUE} />);
  fireEvent.click(screen.getByRole("button", { name: "Start work" }));
  fireEvent.change(await screen.findByRole("combobox"), { target: { value: "/code/proj" } });
};

describe("Start work", () => {
  it("proposes a branch from the key and summary, and shows the argv before running it", async () => {
    await open();
    expect((screen.getByRole("textbox") as HTMLInputElement).value).toBe("abc-1-fix-login-redirect");
    expect(await screen.findByText("/code/proj-abc-1-fix-login-redirect", { selector: "code" })).toBeTruthy();
    expect(plan).toHaveBeenCalledWith("/code/proj", "abc-1-fix-login-redirect", "worktree");
    expect(start).not.toHaveBeenCalled();

    start.mockResolvedValue({ branch: "abc-1-fix-login-redirect", head: "0123456789abcdef", worktree_path: "/code/proj-abc-1-fix-login-redirect" });
    fireEvent.click(screen.getByRole("button", { name: "Create worktree" }));
    await waitFor(() => expect(start).toHaveBeenCalled());
    // The argv shown is the argv sent.
    expect(start.mock.calls[0]).toEqual(["/code/proj", "abc-1-fix-login-redirect", "worktree", planFor("abc-1-fix-login-redirect", "worktree").args]);
    expect(await screen.findByText(/Git lists a worktree at/)).toBeTruthy();
    expect(screen.getByText("0123456")).toBeTruthy();
  });

  it("refuses cleanly, and cannot be confirmed", async () => {
    plan.mockImplementation((_r, b, m) => Promise.resolve(planFor(b, m, { refusal: { state: "branch_exists", branch: b } })));
    await open();
    expect(await screen.findByText(/already exists in this repository/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Create worktree" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("warns when an edited name will not link to the ticket, and plans branch-only", async () => {
    await open();
    fireEvent.click(screen.getByRole("radio", { name: "Branch only" }));
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "fix-login" } });
    expect(await screen.findByText(/does not carry ABC-1/)).toBeTruthy();
    expect(plan).toHaveBeenLastCalledWith("/code/proj", "fix-login", "branch");
    expect(screen.getByRole("button", { name: "Create branch" })).toBeTruthy();
  });

  it("says what failed when the desktop refuses at run time", async () => {
    start.mockRejectedValue("a branch named abc-1-fix-login-redirect already exists");
    await open();
    await screen.findByText("/code/proj-abc-1-fix-login-redirect", { selector: "code" });
    fireEvent.click(screen.getByRole("button", { name: "Create worktree" }));
    expect(await screen.findByText("a branch named abc-1-fix-login-redirect already exists")).toBeTruthy();
  });
});
