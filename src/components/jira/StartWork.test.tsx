import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { StartMode, StartPlan, Started } from "@/api/tauri";
import type { JiraIssue, ProjectStatuses, TransitionResult } from "@/types/jira";
import type { ExpectationMap } from "@/lib/jira/drift";
import { renderWithQuery } from "@/test-utils";

const plan = vi.hoisted(() => vi.fn<(repo: string, branch: string, mode: StartMode) => Promise<StartPlan>>());
const start = vi.hoisted(() => vi.fn<(repo: string, branch: string, mode: StartMode, shown: string[]) => Promise<Started>>());
const worktrees = vi.hoisted(() => ({
  data: [{ name: "proj", path: "/code/proj", identity: "octocat/proj", worktrees: [] }] as unknown[],
  isPending: false,
  isError: false,
  error: null,
}));
const transition = vi.hoisted(() => vi.fn<(key: string, status: string) => Promise<TransitionResult>>());
const statuses = vi.hoisted(() => vi.fn<(project: string) => Promise<ProjectStatuses>>());
vi.mock("@/api/tauri", () => ({
  startWorkPlan: plan,
  startWork: start,
  jiraTransition: transition,
  jiraProjectStatuses: statuses,
}));
vi.mock("@/api/hooks", () => ({ useWorktrees: () => worktrees }));

import { JiraContext, type JiraState } from "./jiraState";
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

/// #2045's "Also move ABC-1 to …": opt-in, confirmed beside git's argv,
/// and run only once git has succeeded.
describe("Start work's move in Jira", () => {
  const jira = (expectations: ExpectationMap): JiraState => ({
    projects: new Set(["ABC"]),
    site: "jira.example.internal",
    links: { byRow: new Map(), byKey: new Map() },
    expectations,
    chipsOnPrRows: true,
    branchTemplate: "",
    noTicket: null,
    mergeGraceMins: 15,
  });
  const NAMED: ExpectationMap = { ABC: { on_open: { kind: "status", name: "In Progress" } } };
  const STARTED: Started = { branch: "abc-1-fix-login-redirect", head: "0123456789abcdef", worktree_path: "/code/proj-abc-1-fix-login-redirect" };
  const result = (reply: TransitionResult["reply"], now: string): TransitionResult => ({
    key: "ABC-1",
    requested: "In Progress",
    reply,
    after: { ...ISSUE, status: { name: now, category: now === "To Do" ? "new" : "in_progress" } },
    reread_error: null,
  });

  const openWith = async (expectations: ExpectationMap) => {
    renderWithQuery(
      <JiraContext.Provider value={jira(expectations)}>
        <StartWork issue={ISSUE} />
      </JiraContext.Provider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Start work" }));
    fireEvent.change(await screen.findByRole("combobox"), { target: { value: "/code/proj" } });
    await screen.findByText("/code/proj-abc-1-fix-login-redirect", { selector: "code" });
  };
  const jiraArgv = () =>
    within(screen.getByLabelText("Jira command to run"))
      .getAllByText(/.*/, { selector: "code" })
      .map((c) => c.textContent);

  beforeEach(() => {
    transition.mockReset();
    statuses.mockReset().mockResolvedValue({ project: "ABC", statuses: [{ name: "In Progress", category: "in_progress" }], truncated: false, error: null });
  });

  it("is unchecked by default, and git alone runs", async () => {
    await openWith(NAMED);
    const box = screen.getByRole("checkbox", { name: "Also move ABC-1 to In Progress" }) as HTMLInputElement;
    expect(box.checked).toBe(false);
    expect(screen.queryByLabelText("Jira command to run")).toBeNull();
    start.mockResolvedValue(STARTED);
    fireEvent.click(screen.getByRole("button", { name: "Create worktree" }));
    expect(await screen.findByText(/Git lists a worktree at/)).toBeTruthy();
    expect(transition).not.toHaveBeenCalled();
  });

  it("shows the Jira argv in the same confirmation, and moves only after git succeeds", async () => {
    await openWith(NAMED);
    fireEvent.click(screen.getByRole("checkbox", { name: "Also move ABC-1 to In Progress" }));
    expect(jiraArgv()).toEqual(["acli", "jira", "workitem", "transition", "--key", "ABC-1", "--status", "In Progress", "--yes", "--json"]);
    expect(transition).not.toHaveBeenCalled();
    start.mockResolvedValue(STARTED);
    transition.mockResolvedValue(result({ state: "accepted", message: "ok" }, "In Progress"));
    fireEvent.click(screen.getByRole("button", { name: "Create worktree" }));
    expect(await screen.findByText("Jira now reports ABC-1 as In Progress.")).toBeTruthy();
    expect(transition).toHaveBeenCalledWith("ABC-1", "In Progress");
    expect(start.mock.invocationCallOrder[0]).toBeLessThan(transition.mock.invocationCallOrder[0]);
  });

  it("does not try the move when git fails", async () => {
    await openWith(NAMED);
    fireEvent.click(screen.getByRole("checkbox", { name: "Also move ABC-1 to In Progress" }));
    start.mockRejectedValue("a branch named abc-1-fix-login-redirect already exists");
    fireEvent.click(screen.getByRole("button", { name: "Create worktree" }));
    expect(await screen.findByText("a branch named abc-1-fix-login-redirect already exists")).toBeTruthy();
    expect(transition).not.toHaveBeenCalled();
  });

  it("shows Jira's own words when it refuses, beside the branch that was made", async () => {
    await openWith(NAMED);
    fireEvent.click(screen.getByRole("checkbox", { name: "Also move ABC-1 to In Progress" }));
    start.mockResolvedValue(STARTED);
    transition.mockResolvedValue(result({ state: "refused", reason: "No allowed transitions found for given status" }, "To Do"));
    fireEvent.click(screen.getByRole("button", { name: "Create worktree" }));
    expect(
      await screen.findByText("Jira refused to move ABC-1 to In Progress: No allowed transitions found for given status. Jira reports it as To Do."),
    ).toBeTruthy();
    expect(screen.getByText(/Git lists a worktree at/)).toBeTruthy();
  });

  it("names no status for a category expectation: the reader types it, and cannot confirm until they do", async () => {
    await openWith({});
    fireEvent.click(screen.getByRole("checkbox", { name: "Also move ABC-1 in Jira" }));
    expect(screen.getByText(/ABC expects any In Progress status here/)).toBeTruthy();
    expect(screen.queryByLabelText("Jira command to run")).toBeNull();
    expect((screen.getByRole("button", { name: "Create worktree" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByPlaceholderText("as Jira names it"), { target: { value: "Doing" } });
    expect(jiraArgv()).toContain("Doing");
    expect((screen.getByRole("button", { name: "Create worktree" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("offers no move for a ticket already where the project expects it", async () => {
    renderWithQuery(
      <JiraContext.Provider value={jira({})}>
        <StartWork issue={{ ...ISSUE, status: { name: "In Progress", category: "in_progress" } }} />
      </JiraContext.Provider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Start work" }));
    await screen.findByRole("combobox");
    expect(screen.queryByRole("checkbox")).toBeNull();
  });
});
