import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider, focusManager } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PrDetail, PullRequest } from "@/types/pr";
import { stubViewport } from "@/test-utils";

const invoke = vi.hoisted(() => vi.fn<(name: string, args?: Record<string, unknown>) => Promise<unknown>>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => {}) }));
// Optional local integrations are outside this regression. Detail, viewer,
// review/action hooks, query observers and rendered controls remain production.
vi.mock("../api/hooks", async (original) => ({
  ...await original<object>(),
  useWorktrees: () => ({ repos: [], unreadable: [], isError: false }),
  useUiPrefs: () => ({ prefs: { terminal_command: "" } }),
  useClaudeSessionsForPr: () => ({ state: "done", links: [], elsewhere: [] }),
  useReviewGates: () => ({ data: undefined }),
}));
import { PrDetailView } from "./PrDetailView";

function detail(number = 7, merge_status: PrDetail["merge_status"] = "clean"): PrDetail {
  return {
    id: `PR_${number}`, number, title: `Synthetic change ${number}`, repo: "octocat/hello-world",
    url: `https://github.com/octocat/hello-world/pull/${number}`, state: "open", is_draft: false,
    author: "author", body: `Retained description ${number}`, head_ref: "topic", base_ref: "main",
    head_oid: `head-${number}`, head_ref_id: null, merge_status, review: "none", latest_reviews: [],
    merge_queue_enabled: false, in_merge_queue: false, additions: 1, deletions: 0, changed_files: 1,
    unresolved_threads: 0, comment_count: 0, comments: [], review_threads: [], review_threads_total: 0,
    checks: [{ name: `Synthetic check ${number}`, state: "pending", url: "", run_id: null }], checks_total: 1,
  };
}
let qc: QueryClient;
let fail: boolean;
let reads: number;
let status: PrDetail["merge_status"];
const key = ["pr-detail", "octocat/hello-world", 7];
function mount(number = 7) {
  return render(<QueryClientProvider client={qc}><PrDetailView repo="octocat/hello-world" number={number} onBack={() => {}} /></QueryClientProvider>);
}
beforeEach(() => {
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  fail = false; reads = 0; status = "clean";
  stubViewport(1200);
  invoke.mockImplementation(async (name, args) => {
    if (name === "get_viewer") return "reviewer";
    if (name === "get_ready_stacks") return (args?.rows as object[]).map(row => ({ ...row, valid_for_ms: 60_000, stack: { kind: "none" } }));
    if (name === "get_pr_detail") {
      reads++;
      if (fail) throw new Error("Synthetic refresh unavailable");
      return detail(args?.number as number, status);
    }
    throw new Error(`Unexpected command: ${name}`);
  });
});
afterEach(() => { cleanup(); qc.clear(); focusManager.setFocused(undefined); vi.useRealTimers(); stubViewport(null); });

async function loaded(number = 7) {
  await screen.findByText(`Retained description ${number}`);
  expect(screen.getByText(`Synthetic check ${number}`)).toBeTruthy();
}
async function retained() {
  await screen.findByRole("alert");
  expect(screen.getByText("Retained description 7")).toBeTruthy();
  expect(screen.getByText("Synthetic check 7")).toBeTruthy();
  expect(screen.getByText("Synthetic change 7")).toBeTruthy();
  expect(screen.getByRole("link", { name: /github/i })).toBeTruthy();
  for (const button of screen.queryAllByRole("button", { name: /^Approve$/ })) expect(button.matches(":disabled")).toBe(false);
}

describe("mounted detail refresh with real query hooks", () => {
  it.each([1200, 390])("retains fetched content and recovers after manual failure at width %s", async width => {
    stubViewport(width); mount(); await loaded();
    fail = true;
    await act(async () => { await qc.refetchQueries({ queryKey: key }); });
    await retained();
    fail = false;
    fireEvent.click(screen.getByRole("button", { name: /retry refresh/i }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(screen.getAllByRole("button", { name: /^Approve$/ }).some(b => !b.matches(":disabled"))).toBe(true);
  });
  it("retains fetched content after stale focus refresh fails", async () => {
    mount(); await loaded();
    qc.setQueryData(key, detail(), { updatedAt: Date.now() - 31_000 });
    fail = true;
    act(() => { focusManager.setFocused(false); focusManager.setFocused(true); });
    await retained();
  });
  it("retains fetched content after mergeability timer failure without retry spinning", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true }); status = "unknown";
    mount(); await loaded(); fail = true;
    await act(async () => { await vi.advanceTimersByTimeAsync(3_100); });
    await retained();
    const atFailure = reads;
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
    expect(reads).toBe(atFailure);
  });
  it.each([false, true])("keeps initial failure distinct from full detail (row placeholder %s)", async seeded => {
    if (seeded) qc.setQueryData(["prs"], [detail() as unknown as PullRequest]);
    fail = true; mount();
    await screen.findByText("Could not load this pull request");
    expect(screen.queryByText("Retained description 7")).toBeNull();
    expect(screen.queryByText("Synthetic check 7")).toBeNull();
    expect(qc.getQueryData(key)).toBeUndefined();
    expect(screen.getByRole("link", { name: /github/i })).toBeTruthy();
  });
  it("does not carry the previous PR's retained content or warning into a different PR", async () => {
    const view = mount(); await loaded(); fail = true;
    await act(async () => { await qc.refetchQueries({ queryKey: key }); });
    await retained(); fail = false;
    view.rerender(<QueryClientProvider client={qc}><PrDetailView repo="octocat/hello-world" number={8} onBack={() => {}} /></QueryClientProvider>);
    await loaded(8);
    expect(screen.queryByText("Retained description 7")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

it("keeps the last fetched view while one explicit retry is pending", async () => {
  mount(); await loaded(); fail = true;
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  await retained();
  let finish!: (value: PrDetail) => void;
  invoke.mockImplementation((name) => name === "get_pr_detail"
    ? new Promise<PrDetail>(resolve => { finish = resolve; }) : Promise.resolve("reviewer"));
  const before = invoke.mock.calls.filter(([name]) => name === "get_pr_detail").length;
  fireEvent.click(screen.getByRole("button", { name: /retry refresh/i }));
  await screen.findByRole("button", { name: "Refreshing…" });
  const retry = screen.getByRole("button", { name: "Refreshing…" });
  expect(retry.matches(":disabled")).toBe(true);
  fireEvent.click(retry);
  expect(invoke.mock.calls.filter(([name]) => name === "get_pr_detail").length).toBe(before + 1);
  expect(screen.getByText("Retained description 7")).toBeTruthy();
  await act(async () => { finish(detail()); });
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
});

it("lets retained conversations expand while their mutation controls stay paused", async () => {
  const withThread = { ...detail(), review_threads_total: 1, review_threads: [{
    id: "thread", path: "synthetic.ts", line: 1, is_resolved: true, is_outdated: false,
    viewer_can_reply: false, viewer_can_resolve: false, viewer_can_unresolve: true,
    comments: [], comment_count: 0,
  }] };
  qc.setQueryData(key, withThread);
  mount(); await loaded(); fail = true;
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  await screen.findByRole("alert");
  const toggle = screen.getByRole("button", { name: /synthetic.ts/ });
  expect(toggle.matches(":disabled")).toBe(false);
  fireEvent.click(toggle);
  expect(screen.getByRole("button", { name: "Reopen" }).matches(":disabled")).toBe(true);
});

it("ignores a delayed failed refresh after switching to another PR", async () => {
  const view = mount(); await loaded();
  let rejectOld!: (error: Error) => void;
  invoke.mockImplementation((name, args) => name !== "get_pr_detail" ? Promise.resolve("reviewer")
    : args?.number === 7 ? new Promise((_resolve, reject) => { rejectOld = reject; })
    : Promise.resolve(detail(8)));
  let refresh!: Promise<void>;
  act(() => { refresh = qc.refetchQueries({ queryKey: key }); });
  view.rerender(<QueryClientProvider client={qc}><PrDetailView repo="octocat/hello-world" number={8} onBack={() => {}} /></QueryClientProvider>);
  await loaded(8);
  await act(async () => { rejectOld(new Error("Old refresh failed")); await refresh; });
  expect(screen.getByText("Retained description 8")).toBeTruthy();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.queryByText("Retained description 7")).toBeNull();
});

it("keeps zero-row unknown checks visible without inventing a complete empty result", async () => {
  const unknown = { ...detail(), checks: [], checks_total: 0, checks_coverage: { state: "unknown", total: null } };
  invoke.mockImplementation(async (name) => name === "get_pr_detail" ? unknown : name === "get_viewer" ? "reviewer" : []);
  mount();
  await screen.findByText("Retained description 7");
  expect(screen.getByText(/remaining checks could not be confirmed/i)).toBeTruthy();
});

it("owns selected ancestry after primary renders and blocks ordinary merge while unknown", async () => {
  let finish!: (value: unknown) => void;
  invoke.mockImplementation((name, args) => {
    if (name === "get_pr_detail") return Promise.resolve({ ...detail(), stack: { kind: "unknown" } });
    if (name === "get_viewer") return Promise.resolve("reviewer");
    if (name === "get_ready_stacks") return new Promise(resolve => { finish = () => resolve((args?.rows as object[]).map(row => ({ ...row, valid_for_ms: 60_000, stack: { kind: "none" } }))); });
    return Promise.resolve(null);
  });
  mount(); await loaded();
  for (const button of screen.getAllByRole("button", { name: /^Merge$/ })) expect(button.matches(":disabled")).toBe(true);
  await waitFor(() => expect(finish).toBeDefined());
  await act(async () => { finish(null); });
  await waitFor(() => expect(screen.getAllByRole("button", { name: /^Merge$/ }).every(b => !b.matches(":disabled"))).toBe(true));
  expect(invoke.mock.calls.filter(([name]) => name === "get_pr_detail")).toHaveLength(1);
});


it.each([1, 2])("routes actual selected-detail native stack size %s through StackMerge at both widths", async size => {
  const stack = { kind: "stacked", native: true, stack_number: 3, position: size, size, position_exact: true, size_exact: true, below: size === 1 ? null : 6,
    members_complete: true, members: Array.from({ length: size }, (_, i) => ({ position: i + 1, number: 8 - size + i, title: "Synthetic layer", state: "open" })) };
  invoke.mockImplementation(async (name, args) => {
    if (name === "get_pr_detail") return { ...detail(), stack: { kind: "unknown" } };
    if (name === "get_viewer") return "reviewer";
    if (name === "get_ready_stacks") return (args?.rows as object[]).map(row => ({ ...row, valid_for_ms: 60_000, stack }));
    if (name === "merge_stack") return { kind: "enqueued" };
    return null;
  });
  stubViewport(size === 1 ? 390 : 1200);
  mount(); await loaded();
  await screen.findAllByText(new RegExp(`stack ${size}/${size}`));
  const buttons = await screen.findAllByRole("button", { name: size === 1 ? /^Merge$/ : /Merge 2 pull requests…/ });
  if (size === 1) expect(buttons.length).toBeGreaterThanOrEqual(2);
  else {
    expect(buttons).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: /^Merge$/ }).every(b => b.matches(":disabled"))).toBe(true);
  }
  expect(buttons.every(b => !b.matches(":disabled"))).toBe(true);
  fireEvent.click(buttons[0]);
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: `Merge ${size} pull request${size === 1 ? "" : "s"}` }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("merge_stack", expect.objectContaining({ expectedHead: "head-7" })));
  expect(invoke.mock.calls.some(([name]) => name === "act_on_pr")).toBe(false);
});

it("does not attach an old-head advisory response after the displayed head changes", async () => {
  let finish!: () => void;
  invoke.mockImplementation((name, args) => {
    if (name === "get_viewer") return Promise.resolve("reviewer");
    if (name === "get_pr_detail") return Promise.resolve({ ...detail(), stack: { kind: "unknown" } });
    if (name === "get_ready_stacks") return new Promise(resolve => { finish = () => resolve((args?.rows as object[]).map(row => ({ ...row, valid_for_ms: 60_000, stack: { kind: "none" } }))); });
    return Promise.resolve(null);
  });
  mount(); await loaded(); await waitFor(() => expect(finish).toBeDefined());
  const old = finish;
  await act(async () => { qc.setQueryData(key, { ...detail(), head_oid: "new-head", stack: { kind: "unknown" } }); });
  await act(async () => old());
  for (const b of screen.getAllByRole("button", { name: /^Merge$/ })) expect(b.matches(":disabled")).toBe(true);
});


it("accepts legacy detail wire replies and validates optional coverage without coercion", async () => {
  const { assertRemoteReply } = await import("@/api/wireContract");
  expect(() => assertRemoteReply("get_pr_detail", detail())).not.toThrow();
  for (const state of ["complete", "partial", "unknown"]) {
    expect(() => assertRemoteReply("get_pr_detail", { ...detail(), checks_coverage: { state, total: state === "unknown" ? null : 2 } })).not.toThrow();
  }
  for (const checks_coverage of [{ state: "assumed", total: 0 }, { state: "partial" }, { state: "unknown", total: "private value" }]) {
    expect(() => assertRemoteReply("get_pr_detail", { ...detail(), checks_coverage })).toThrow(/checks_coverage/);
  }
});

it("shows a partial green subset as incomplete while preserving an authoritative clean merge verdict", async () => {
  invoke.mockImplementation(async (name, args) => {
    if (name === "get_viewer") return "reviewer";
    if (name === "get_pr_detail") return { ...detail(), checks: [{ name: "Observed passing check", state: "success", url: "", run_id: null }], checks_total: 5, checks_coverage: { state: "partial", total: 5 } };
    if (name === "get_ready_stacks") return (args?.rows as object[]).map(row => ({ ...row, valid_for_ms: 60_000, stack: { kind: "none" } }));
    return null;
  });
  mount();
  await screen.findByText("Observed passing check");
  expect(screen.getByText(/Showing 1 of 5 checks.*could not be confirmed/)).toBeTruthy();
  await waitFor(() => expect(screen.getAllByRole("button", { name: /^Merge$/ }).every(b => !b.matches(":disabled"))).toBe(true));
});

it("does not schedule selected ancestry from placeholders or hidden full detail", async () => {
  const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
  let finish!: (value: PrDetail) => void;
  qc.setQueryData(["prs"], [detail() as unknown as PullRequest]);
  invoke.mockImplementation((name, args) => {
    if (name === "get_viewer") return Promise.resolve("reviewer");
    if (name === "get_pr_detail") return new Promise(resolve => { finish = resolve; });
    if (name === "get_ready_stacks") return Promise.resolve((args?.rows as object[]).map(row => ({ ...row, valid_for_ms: 60_000, stack: { kind: "none" } })));
    return Promise.resolve(null);
  });
  const view = mount();
  await waitFor(() => expect(finish).toBeDefined());
  expect(invoke.mock.calls.some(([name]) => name === "get_ready_stacks")).toBe(false);
  await act(async () => finish(detail()));
  await loaded();
  expect(invoke.mock.calls.some(([name]) => name === "get_ready_stacks")).toBe(false);
  visibility.mockReturnValue("visible");
  await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
  await waitFor(() => expect(invoke.mock.calls.filter(([name]) => name === "get_ready_stacks")).toHaveLength(1));
  view.unmount(); visibility.mockRestore();
});

it("does not renew an aged native stack receipt after a delayed advisory reply", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  invoke.mockImplementation(async (name, args) => {
    if (name === "get_viewer") return "reviewer";
    if (name === "get_pr_detail") return { ...detail(), stack: { kind: "none" } };
    if (name === "get_ready_stacks") {
      await new Promise(resolve => setTimeout(resolve, 3_000));
      return (args?.rows as object[]).map(row => ({ ...row, stack: { kind: "none" }, valid_for_ms: 5_000 }));
    }
    return null;
  });
  mount(); await loaded();
  await act(async () => { await vi.advanceTimersByTimeAsync(3_050); });
  expect(screen.getAllByRole("button", { name: /^Merge$/ }).every(b => !b.matches(":disabled"))).toBe(true);
  await act(async () => { await vi.advanceTimersByTimeAsync(2_050); });
  expect(screen.getAllByRole("button", { name: /^Merge$/ }).every(b => b.matches(":disabled"))).toBe(true);
  expect(invoke.mock.calls.filter(([name]) => name === "get_ready_stacks")).toHaveLength(1);
});

it("keeps ancestry unknown when a legacy advisory reply has no verifiable lifetime", async () => {
  invoke.mockImplementation(async (name, args) => {
    if (name === "get_viewer") return "reviewer";
    if (name === "get_pr_detail") return { ...detail(), stack: { kind: "none" } };
    if (name === "get_ready_stacks") return (args?.rows as object[]).map(row => ({ ...row, stack: { kind: "none" } }));
    return null;
  });
  mount(); await loaded();
  await waitFor(() => expect(invoke.mock.calls.filter(([name]) => name === "get_ready_stacks")).toHaveLength(1));
  for (const button of screen.getAllByRole("button", { name: /^Merge$/ })) expect(button.matches(":disabled")).toBe(true);
  expect(screen.getByRole("link", { name: "GitHub" })).toBeTruthy();
});

it("submits the viewed head after a background failure without a review preflight", async () => {
  mount(); await loaded(); fail = true;
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  await screen.findByRole("alert");
  const existing = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (name, args) => name === "review_pr_at_head"
    ? { outcome: "acknowledged", receipt: { review_id: "REVIEW-7", state: "APPROVED", actor: "reviewer", commit_oid: "head-7", submitted_at: "2026-10-01T12:00:00Z", pr_id: "PR_7", repo: "octocat/hello-world", number: 7 } }
    : existing(name, args));
  const before = reads;
  fireEvent.click(screen.getAllByRole("button", { name: /^Approve$/ })[0]);
  await waitFor(() => expect(invoke.mock.calls.filter(([name]) => name === "review_pr_at_head")).toHaveLength(1));
  const request = invoke.mock.calls.find(([name]) => name === "review_pr_at_head")![1]?.request;
  expect(request).toMatchObject({ expected_head: "head-7", expected_viewer: "reviewer" });
  expect(reads - before).toBeLessThanOrEqual(1); // optional post-write read, never a preflight
  expect(screen.getAllByRole("button", { name: /^Approved$/ }).length).toBeGreaterThan(0);
});

it("shares the actual mounted review across observers, lagging refresh and remount", async () => {
  const base = invoke.getMockImplementation()!;
  let finish!: (value: unknown) => void;
  const posted = new Promise(resolve => { finish = resolve; });
  invoke.mockImplementation((name, args) => name === "review_pr_at_head" ? posted : base(name, args));
  const first = mount(); await loaded();
  const second = mount();
  fireEvent.click(screen.getAllByRole("button", { name: /^Approve$/ })[0]);
  await waitFor(() => expect(invoke.mock.calls.filter(([name]) => name === "review_pr_at_head")).toHaveLength(1));
  for (const button of screen.queryAllByRole("button", { name: /^Approve$/ })) expect(button.matches(":disabled")).toBe(true);
  await act(async () => finish({ outcome: "acknowledged", receipt: {
    review_id: "REVIEW-7", state: "APPROVED", actor: "reviewer", commit_oid: "head-7",
    submitted_at: "2026-10-01T12:00:00Z", pr_id: "PR_7", repo: "octocat/hello-world", number: 7,
  } }));
  await waitFor(() => expect(screen.getAllByRole("button", { name: /^Approved$/ }).length).toBeGreaterThan(1));
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  first.unmount(); second.unmount(); mount();
  await waitFor(() => expect(screen.getAllByRole("button", { name: /^Approved$/ }).length).toBeGreaterThan(0));
  expect(invoke.mock.calls.filter(([name]) => name === "review_pr_at_head")).toHaveLength(1);
});

it.each([new Error("Synthetic response was lost"), "Synthetic string failure"])("keeps uncertain submitted outcomes check-first after remount: %s", async error => {
  const base = invoke.getMockImplementation()!;
  invoke.mockImplementation((name, args) => name === "review_pr_at_head" ? Promise.reject(error) : base(name, args));
  const view = mount(); await loaded();
  fireEvent.click(screen.getAllByRole("button", { name: /^Approve$/ })[0]);
  await screen.findByText(error instanceof Error ? error.message : error);
  view.unmount(); mount();
  await screen.findByRole("link", { name: "Check review on GitHub" });
  for (const button of screen.queryAllByRole("button", { name: /^Approve$/ })) expect(button.matches(":disabled")).toBe(true);
  expect(invoke.mock.calls.filter(([name]) => name === "review_pr_at_head")).toHaveLength(1);
});

it.each([1200, 390])("preserves converged own approval through later lag and remount at width %s", async width => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  stubViewport(width);
  let current = detail();
  const base = invoke.getMockImplementation()!;
  const receipt = { review_id: "REVIEW-7", state: "APPROVED", actor: "reviewer", commit_oid: "head-7", submitted_at: "2026-10-01T12:00:00Z", pr_id: "PR_7", repo: "octocat/hello-world", number: 7 };
  invoke.mockImplementation(async (name, args) => {
    if (name === "get_pr_detail") return current;
    if (name === "review_pr_at_head") {
      const head = (args?.request as { expected_head: string }).expected_head;
      return { outcome: "acknowledged", receipt: { ...receipt, commit_oid: head, review_id: head === "head-7" ? receipt.review_id : "REVIEW-NEW-HEAD" } };
    }
    return base(name, args);
  });
  let view = mount(); await loaded();
  const sibling = mount();
  fireEvent.click(screen.getAllByRole("button", { name: /^Approve$/ })[0]);
  await waitFor(() => expect(screen.getAllByRole("button", { name: /^Approved$/ }).length).toBeGreaterThan(0));
  current = { ...detail(), latest_reviews: [{ author: "reviewer", state: "APPROVED", id: receipt.review_id, commit_oid: "head-7", submitted_at: receipt.submitted_at }] };
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  await act(async () => { await vi.advanceTimersByTimeAsync(16 * 60_000); });
  fireEvent.change(screen.getAllByRole("textbox")[0], { target: { value: "A deliberate different verdict" } });
  expect(screen.getAllByRole("button", { name: "Request changes" }).some(button => !button.matches(":disabled"))).toBe(true);
  sibling.unmount();
  for (const latest_reviews of [[], [{ author: "reviewer", state: "CHANGES_REQUESTED", id: "OLDER", commit_oid: "head-7", submitted_at: "2026-10-01T11:00:00Z" }]]) {
    current = { ...detail(), latest_reviews };
    await act(async () => { await qc.refetchQueries({ queryKey: key }); });
    view.unmount(); view = mount(); await loaded();
    expect(screen.getAllByRole("button", { name: /^Approved$/ }).length).toBeGreaterThan(1);
    expect(invoke.mock.calls.filter(([name]) => name === "review_pr_at_head")).toHaveLength(1);
  }
  current = { ...detail(), latest_reviews: [{ author: "reviewer", state: "DISMISSED", id: receipt.review_id, commit_oid: "head-7", submitted_at: receipt.submitted_at }] };
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  await waitFor(() => expect(screen.getAllByRole("button", { name: /^Approve$/ }).every(button => !button.matches(":disabled"))).toBe(true));
  current = { ...detail(), head_oid: "new-head", latest_reviews: [] };
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  await act(async () => { await vi.advanceTimersByTimeAsync(50); });
  fireEvent.click(screen.getAllByRole("button", { name: /^Approve$/ })[0]);
  await waitFor(() => expect(invoke.mock.calls.filter(([name]) => name === "review_pr_at_head")).toHaveLength(2));
  expect(invoke.mock.calls.filter(([name]) => name === "review_pr_at_head")[1][1]?.request).toMatchObject({ expected_head: "new-head" });
});

it.each([1200, 390])("does not revive an older approval after an acknowledged different verdict expires at width %s", async width => {
  vi.useFakeTimers({ shouldAdvanceTime: true }); stubViewport(width);
  let current = detail(); let writes = 0;
  const base = invoke.getMockImplementation()!;
  const first = { author: "reviewer", state: "APPROVED", id: "R1", commit_oid: "head-7", submitted_at: "2026-10-01T12:01:00Z" };
  invoke.mockImplementation(async (name, args) => {
    if (name === "get_pr_detail") return current;
    if (name === "review_pr_at_head") {
      writes++;
      return { outcome: "acknowledged", receipt: { review_id: `R${writes}`, state: writes === 2 ? "CHANGES_REQUESTED" : "APPROVED", actor: "reviewer", commit_oid: "head-7", submitted_at: `2026-10-01T12:0${writes}:00Z`, pr_id: "PR_7", repo: "octocat/hello-world", number: 7 } };
    }
    return base(name, args);
  });
  let view = mount(); await loaded();
  fireEvent.click(screen.getAllByRole("button", { name: /^Approve$/ })[0]);
  await waitFor(() => expect(writes).toBe(1));
  current = { ...detail(), latest_reviews: [first] };
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  fireEvent.change(screen.getByRole("textbox"), { target: { value: "A deliberate different verdict" } });
  await waitFor(() => expect(screen.getByRole("button", { name: "Request changes" }).matches(":disabled")).toBe(false));
  current = detail();
  fireEvent.click(screen.getByRole("button", { name: "Request changes" }));
  await waitFor(() => expect(writes).toBe(2));
  await act(async () => { await vi.advanceTimersByTimeAsync(16 * 60_000); });
  for (const latest_reviews of [[], [first]]) {
    current = { ...detail(), latest_reviews };
    await act(async () => { await qc.refetchQueries({ queryKey: key }); });
    view.unmount(); view = mount(); await loaded();
    await screen.findByRole("link", { name: "Check review on GitHub" });
    expect(screen.queryAllByRole("button", { name: /^Approved$/ })).toHaveLength(0);
    expect(writes).toBe(2);
  }
  fireEvent.click(screen.getByRole("button", { name: "I checked on GitHub; allow another review" }));
  await waitFor(() => expect(screen.getAllByRole("button", { name: /^Approve$/ }).every(button => !button.matches(":disabled"))).toBe(true));
  fireEvent.click(screen.getAllByRole("button", { name: /^Approve$/ })[0]);
  await waitFor(() => expect(writes).toBe(3));
});

const disclosureBody = '<details><summary>Report</summary>Body<details><summary>Nested</summary>Inner</details></details>';
const reportComment = { author: "report-bot", created_at: "2026-01-01T00:00:00Z", author_is_bot: true, body: "<!-- report -->\n" + disclosureBody };
it.each([1200, 390])("preserves reader disclosures through controlled pending, error and recovery at width %s", async width => {
  stubViewport(width);
  const current = { ...detail(), body: disclosureBody, comments: [reportComment, { ...reportComment, created_at: "2026-01-02T00:00:00Z" }], comment_count: 2,
    review_threads: [{ id: "RT_1", is_resolved: true, is_outdated: false, path: "src/synthetic.ts", line: 10, viewer_can_reply: true, viewer_can_resolve: false, viewer_can_unresolve: true, comments: [{ ...reportComment, author: "reviewer" }], comment_count: 1 }], review_threads_total: 1 };
  const base = invoke.getMockImplementation()!;
  invoke.mockImplementation((name, args) => name === "get_pr_detail" ? Promise.resolve(current) : base(name, args));
  const view = mount(); await screen.findByText("Synthetic check 7");
  const settled = screen.getByRole("button", { name: /src\/synthetic.ts/ });
  const group = screen.getByRole("button", { name: /Superseded/ });
  fireEvent.click(settled); fireEvent.click(group);
  const rows = screen.getAllByRole("button", { name: /report-bot/ });
  rows.filter(b => b.getAttribute("aria-expanded") === "false").forEach(b => fireEvent.click(b));
  const nodes = [...view.container.querySelectorAll("details")];
  expect(nodes).toHaveLength(8);
  nodes.forEach((node, i) => { node.open = i % 2 === 0; });
  const verify = () => {
    const after = [...view.container.querySelectorAll("details")];
    expect(after).toHaveLength(nodes.length);
    after.forEach((node, i) => { expect(node).toBe(nodes[i]); expect(node.open).toBe(i % 2 === 0); });
    for (const button of [settled, group, ...rows]) expect(button.getAttribute("aria-expanded")).toBe("true");
  };
  for (const failure of [false, true, false]) {
    let resolve!: (value: unknown) => void; let reject!: (error: Error) => void;
    invoke.mockImplementation((name, args) => name === "get_pr_detail" ? new Promise((yes, no) => { resolve = yes; reject = no; }) : base(name, args));
    let pending!: Promise<void>;
    act(() => { pending = qc.refetchQueries({ queryKey: key }); });
    await waitFor(() => expect(resolve).toBeDefined());
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
    verify();
    await act(async () => { if (failure) reject(new Error("Synthetic failure")); else resolve(current); await pending; });
    await waitFor(() => expect(!!screen.queryByRole("alert")).toBe(failure));
    verify();
  }
  expect(invoke.mock.calls.filter(([name]) => name === "get_pr_detail")).toHaveLength(4);
  fireEvent.click(rows[0]); fireEvent.click(rows[0]);
  expect([...view.container.querySelectorAll("details")].filter(node => node.open)).toHaveLength(3);
});

it.each([undefined, "COMMENT_ALICE"])("keeps an edited human row (%s) open when bot folding and bounded-window removal shift positions", async id => {
  const base = invoke.getMockImplementation()!;
  const human = { id, author: "alice", created_at: "2026-01-02T12:00:00Z", author_is_bot: false, body: disclosureBody };
  let comments = [reportComment, human];
  invoke.mockImplementation((name, args) => name === "get_pr_detail" ? Promise.resolve({ ...detail(), comments, comment_count: comments.length }) : base(name, args));
  const view = mount(); await loaded();
  const button = screen.getByRole("button", { name: /alice/ }); fireEvent.click(button);
  const node = view.container.querySelector("details")!; node.open = true;
  comments = [reportComment, { ...human, body: disclosureBody.replace("Body", "Edited body") }, { ...reportComment, created_at: "2026-01-03T00:00:00Z" }];
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  expect(screen.getByRole("button", { name: /alice/ })).toBe(button);
  await waitFor(() => expect(view.container.textContent).toContain("Edited body"));
  expect(screen.getByRole("button", { name: /alice/ })).toBe(button);
  expect(node.isConnected && node.open).toBe(true);
  const newBot = screen.getByRole("button", { name: /report-bot/ });
  expect(newBot.getAttribute("aria-expanded")).toBe("false");
  comments = comments.slice(1);
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  expect(screen.getByRole("button", { name: /alice/ })).toBe(button);
  expect(node.isConnected && node.open).toBe(true);
  fireEvent.click(button);
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  expect(button.getAttribute("aria-expanded")).toBe("false");
  // Becoming the only visible row changes defaultOpen, not the reader's choice.
  comments = [comments[0]];
  await act(async () => { await qc.refetchQueries({ queryKey: key }); });
  await waitFor(() => expect(screen.queryByRole("button", { name: /report-bot/ })).toBeNull());
  expect(screen.getByRole("button", { name: /alice/ })).toBe(button);
  expect(button.getAttribute("aria-expanded")).toBe("false");
});

it("resets disclosures when navigating to a different PR and returning", async () => {
  const base = invoke.getMockImplementation()!;
  invoke.mockImplementation((name, args) => name === "get_pr_detail" ? Promise.resolve({ ...detail(args?.number as number), body: disclosureBody }) : base(name, args));
  qc.setQueryData(["pr-detail", "octocat/hello-world", 8], { ...detail(8), body: disclosureBody });
  const view = mount(); await screen.findByText("Synthetic check 7");
  view.container.querySelector("details")!.open = true;
  for (const number of [8, 7]) {
    view.rerender(<QueryClientProvider client={qc}><PrDetailView repo="octocat/hello-world" number={number} onBack={() => {}} /></QueryClientProvider>);
    await screen.findByText(`Synthetic check ${number}`);
    expect(view.container.querySelector("details")!.open).toBe(false);
    view.container.querySelector("details")!.open = true;
  }
});

it("preserves disclosures on initial viewer resolution but resets on a known account switch", async () => {
  const base = invoke.getMockImplementation()!;
  let viewer!: (value: string) => void;
  invoke.mockImplementation((name, args) => name === "get_viewer" ? new Promise(resolve => { viewer = resolve; })
    : name === "get_pr_detail" ? Promise.resolve({ ...detail(), body: disclosureBody }) : base(name, args));
  const view = mount(); await screen.findByText("Synthetic check 7");
  const node = view.container.querySelector("details")!; node.open = true;
  await act(async () => { viewer("reviewer"); });
  await waitFor(() => expect(qc.getQueryData(["viewer"])).toBe("reviewer"));
  expect(view.container.querySelector("details")).toBe(node); expect(node.open).toBe(true);
  await act(async () => { qc.setQueryData(["viewer"], "another-reviewer"); });
  await waitFor(() => expect(view.container.querySelector("details")).not.toBe(node));
  expect(view.container.querySelector("details")!.open).toBe(false);
});

it("validates comment IDs on current replies while accepting old ID-less desktop/companion payloads", async () => {
  const { assertRemoteReply } = await import("@/api/wireContract");
  const legacy = { ...detail(), comments: [reportComment] };
  for (const id of [undefined, null, "COMMENT_1"]) {
    expect(() => assertRemoteReply("get_pr_detail", { ...legacy, comments: [{ ...reportComment, id }] })).not.toThrow();
  }
  expect(() => assertRemoteReply("get_pr_detail", { ...legacy, comments: [{ ...reportComment, id: 123 }] })).toThrow(/id/);
});
