import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
  for (const button of screen.queryAllByRole("button", { name: /^Approve$/ })) expect(button.matches(":disabled")).toBe(true);
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
