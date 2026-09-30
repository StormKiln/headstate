import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { StackMergeOutcome } from "@/api/tauri";
import type { PrDetail, PrStack } from "@/types/pr";

type Merge = (
  repo: string,
  number: number,
  action: string,
  expectedHead: string,
) => Promise<StackMergeOutcome>;
const mergeStack = vi.hoisted(() => vi.fn<Merge>());
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), info: vi.fn() }));

vi.mock("../api/hooks", () => ({
  useActOnPr: () => vi.fn(() => Promise.resolve()),
  useMergeStack: () => mergeStack,
}));
vi.mock("sonner", () => ({ toast }));

import { PrActions } from "./PrActions";

/// #30 sits at position 3 of GitHub stack #7: #10 (merged), #20 (open)
/// beneath it, #40 above.
const stack: PrStack = {
  kind: "stacked",
  native: true,
  stack_number: 7,
  position: 3,
  size: 4,
  position_exact: true,
  size_exact: true,
  below: 20,
  members: [
    { position: 1, number: 10, title: "Base layer", state: "merged" },
    { position: 2, number: 20, title: "Second layer", state: "open" },
    { position: 3, number: 30, title: "Third layer", state: "open" },
    { position: 4, number: 40, title: "Top layer", state: "open" },
  ],
  members_complete: true,
};

const pr = (over: Partial<PrDetail> = {}): PrDetail => ({
  id: "PR_stack",
  number: 30,
  title: "Third layer",
  url: "u",
  state: "open",
  is_draft: false,
  body: "",
  author: "someone",
  repo: "acme/widgets",
  head_ref: "feat-c",
  head_oid: "head-30",
  head_ref_id: null,
  base_ref: "feat-b",
  merge_status: "clean",
  review: "approved",
  additions: 1,
  deletions: 0,
  changed_files: 1,
  unresolved_threads: 0,
  comment_count: 0,
  comments: [],
  review_threads: [],
  review_threads_total: 0,
  latest_reviews: [],
  merge_queue_enabled: true,
  in_merge_queue: false,
  checks: [],
  checks_total: 0,
  stack,
  ...over,
});

beforeEach(() => {
  mergeStack.mockReset();
  toast.success.mockClear();
  toast.error.mockClear();
  toast.info.mockClear();
});

const openConfirm = (label = "Queue 2 pull requests…") =>
  fireEvent.click(screen.getByRole("button", { name: label }));

describe("Stack merge (#1468)", () => {
  /// The confirmation names every pull request the merge lands -- the open
  /// ones beneath, and this one -- bottom first, and nothing above it.
  it("confirms with every pull request it lands before acting", () => {
    render(<PrActions pr={pr()} />);
    openConfirm();
    expect(mergeStack).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog");
    const items = within(dialog)
      .getAllByRole("listitem")
      .map((li) => li.textContent);
    expect(items).toEqual(["#20 — Second layer", "#30 — Third layer"]);
    expect(within(dialog).getByText(/This queues #20 and #30 together/)).toBeTruthy();
  });

  it("queues through the merge queue with the head the user saw", async () => {
    mergeStack.mockResolvedValue({ kind: "enqueued" });
    render(<PrActions pr={pr()} />);
    openConfirm();
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Queue 2 pull requests" }));
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    expect(mergeStack).toHaveBeenCalledWith("acme/widgets", 30, "merge_queue", "head-30");
  });

  it("merges directly when the base branch does not queue", async () => {
    mergeStack.mockResolvedValue({ kind: "merged", sha: "abc" });
    render(<PrActions pr={pr({ merge_queue_enabled: false })} />);
    openConfirm("Merge 2 pull requests…");
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Merge 2 pull requests" }));
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    expect(mergeStack.mock.calls[0][2]).toBe("direct_merge");
  });

  it("does nothing when the confirmation is cancelled", () => {
    render(<PrActions pr={pr()} />);
    openConfirm();
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }));
    expect(mergeStack).not.toHaveBeenCalled();
  });

  it("reports GitHub's reason when the merge fails", async () => {
    mergeStack.mockResolvedValue({ kind: "failed", message: "Required status check is failing." });
    render(<PrActions pr={pr()} />);
    openConfirm();
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Queue 2 pull requests" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(toast.error.mock.calls[0][1]).toEqual({ description: "Required status check is failing." });
  });

  /// Still running on GitHub is not a failure, and must not read as one.
  it("reports a merge still in progress without calling it a failure", async () => {
    mergeStack.mockResolvedValue({ kind: "in_progress", message: "Submitted to GitHub and still in progress there" });
    render(<PrActions pr={pr()} />);
    openConfirm();
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Queue 2 pull requests" }));
    await waitFor(() => expect(toast.info).toHaveBeenCalled());
    expect(toast.error).not.toHaveBeenCalled();
    expect(String(toast.info.mock.calls[0][0])).toMatch(/still in progress/);
  });

  /// Without GitHub's whole membership the confirmation could understate
  /// what lands, so the #1452 gate stays instead.
  it("falls back to the disabled button when the membership is incomplete", () => {
    render(<PrActions pr={pr({ stack: { ...stack, members_complete: false } })} />);
    expect(screen.queryByRole("button", { name: "Add stack to merge queue" })).toBeNull();
    expect((screen.getByRole("button", { name: "Add to merge queue" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("carries the plain button's availability reason", () => {
    render(<PrActions pr={pr({ merge_queue_enabled: false, merge_status: "dirty" })} />);
    const button = screen.getByRole("button", { name: "Merge 2 pull requests…" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect(button.title).toBe("merge conflicts");
  });
});


describe("selected PR scope (#1601)", () => {
  it("keeps selected-only intent visible and requires an explicit broader action", () => {
    render(<PrActions pr={pr()} />);
    const individual = screen.getByRole("button", { name: "Add to merge queue" }) as HTMLButtonElement;
    expect(individual.disabled).toBe(true);
    expect(individual.title).toMatch(/#20.*first/);
    expect(screen.getByText(/cannot queue #30 alone/i)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Queue 2 pull requests…" })).toBeTruthy();
    openConfirm();
    expect(screen.getByRole("dialog").textContent).toMatch(/readiness.*unknown/i);
    expect(mergeStack).not.toHaveBeenCalled();
  });

  it.each([false, true])("labels a one-member plan individually (queue=%s)", async (queue) => {
    const one = { ...stack, members: stack.members!.map((m) => m.number === 20 ? { ...m, state: "merged" } : m) };
    mergeStack.mockResolvedValue(queue ? { kind: "enqueued" } : { kind: "merged", sha: null });
    render(<PrActions pr={pr({ stack: one, merge_queue_enabled: queue })} />);
    fireEvent.click(screen.getByRole("button", { name: queue ? "Add to merge queue" : "Merge" }));
    expect(screen.getByRole("dialog").textContent).not.toMatch(/#20/);
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: queue ? "Queue 1 pull request" : "Merge 1 pull request" }));
    await waitFor(() => expect(mergeStack).toHaveBeenCalledWith("acme/widgets", 30, queue ? "merge_queue" : "direct_merge", "head-30"));
  });

  it.each([
    [{ is_draft: true }, /draft/i],
    [{ review: "review_required" }, /approval required/i],
    [{ review: "changes_requested" }, /changes requested/i],
  ])("names a predecessor's known blocker %j", (facts, expected) => {
    const blocked = { ...stack, members: stack.members!.map((m) => m.number === 20 ? { ...m, ...facts } : m) };
    render(<PrActions pr={pr({ stack: blocked })} />);
    const group = screen.getByRole("button", { name: "Queue 2 pull requests…" }) as HTMLButtonElement;
    expect(group.disabled).toBe(true);
    expect(group.title).toMatch(expected);
    expect(group.title).toContain("#20");
  });
});

it("keeps native enqueue disabled while selected mergeability is being refreshed", () => {
  render(<PrActions pr={pr({ merge_status: "unknown" })} />);
  const action = screen.getByRole("button", { name: "Queue 2 pull requests…" }) as HTMLButtonElement;
  expect(action.disabled).toBe(true);
  expect(action.title).toMatch(/checking/i);
});

it("requires a fresh confirmation when the viewed head changes", () => {
  const view = render(<PrActions pr={pr()} />);
  openConfirm();
  view.rerender(<PrActions pr={pr({ head_oid: "new-head" })} />);
  const confirm = within(screen.getByRole("dialog")).getByRole("button", { name: "Queue 2 pull requests" }) as HTMLButtonElement;
  expect(confirm.disabled).toBe(true);
  expect(screen.getByRole("dialog").textContent).toMatch(/changed/i);
  fireEvent.click(confirm);
  expect(mergeStack).not.toHaveBeenCalled();
});

it("cannot submit an open confirmation after a readiness blocker arrives", () => {
  const view = render(<PrActions pr={pr()} />);
  openConfirm();
  view.rerender(<PrActions pr={pr({ merge_status: "unknown" })} />);
  const confirm = within(screen.getByRole("dialog")).getByRole("button", { name: "Queue 2 pull requests" }) as HTMLButtonElement;
  expect(confirm.disabled).toBe(true);
  fireEvent.click(confirm);
  expect(mergeStack).not.toHaveBeenCalled();
});

it("keeps the compact header's single-PR intent and leaves the broader action in the body", () => {
  render(<PrActions pr={pr()} compact />);
  const buttons = screen.getAllByRole("button");
  expect(buttons).toHaveLength(1);
  expect(buttons[0].textContent).toBe("Add to merge queue");
  expect((buttons[0] as HTMLButtonElement).disabled).toBe(true);
  expect(buttons[0].title).toMatch(/#20.*first/);
});


it.each([
  ["failure", /checks failing/i],
  ["error", /checks failing/i],
  ["pending", /checks pending/i],
  ["expected", /checks pending/i],
])("reports aggregate %s checks as advisory when required-check policy is unknown", (checks, expected) => {
  const advisory = { ...stack, members: stack.members!.map((member) => member.number === 20 ? { ...member, checks } : member) };
  render(<PrActions pr={pr({ stack: advisory })} />);
  const action = screen.getByRole("button", { name: "Queue 2 pull requests…" }) as HTMLButtonElement;
  expect(action.disabled).toBe(false);
  fireEvent.click(action);
  const dialog = screen.getByRole("dialog");
  expect(dialog.textContent).toMatch(expected);
  expect(dialog.textContent).toMatch(/required.check policy.*unknown/i);
  const confirm = within(dialog).getByRole("button", { name: "Queue 2 pull requests" }) as HTMLButtonElement;
  expect(confirm.disabled).toBe(false);
});
