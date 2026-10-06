import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import type { PrDetail, PullRequest, PusherAsk, RowPusher } from "@/types/pr";
import { stubViewport } from "@/test-utils";
import publications from "../../src-tauri/tests/fixtures/review-reliability-publications.json";

const boundary = vi.hoisted(() => ({
  invoke: vi.fn<(name: string, args?: Record<string, unknown>) => Promise<unknown>>(),
  listeners: new Map<string, Set<(event: { payload: unknown }) => void>>(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: boundary.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async (name: string, callback: (event: { payload: unknown }) => void) => {
  const callbacks = boundary.listeners.get(name) ?? new Set();
  callbacks.add(callback); boundary.listeners.set(name, callbacks);
  return () => { callbacks.delete(callback); };
} }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => {}) }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));
// Only unrelated local integrations are replaced. Inventory, event handling,
// advisory demand, detail, viewer and review operation hooks are production.
vi.mock("../api/hooks", async original => ({ ...await original<object>(),
  useWorktrees: () => ({ repos: [], unreadable: [], isError: false }),
  useUiPrefs: () => ({ prefs: { terminal_command: "" } }),
  useClaudeSessionsForPr: () => ({ state: "done", links: [], elsewhere: [] }),
  useReviewGates: () => ({ data: undefined, refetch: vi.fn(async () => {}), isFetching: false }),
}));
import { useReviewing } from "@/api/hooks";
import { refreshWithState } from "@/api/sourceRefreshHooks";
import { remoteEventError } from "@/api/wireContract";
import { ReadyStrip } from "./ReadyStrip";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrDetailView } from "./PrDetailView";

const cleanupClients: QueryClient[] = [];
function Observer({ name }: { name: string }) {
  const inventory = useReviewing(false);
  const [selected, select] = useState<PullRequest | null>(null);
  return <section data-testid={name}><ReadyStrip prs={inventory.data ?? []} onOpen={select} />
    {selected && <div data-testid={`${name}-detail`}><PrDetailView repo={selected.repo} number={selected.number} onBack={() => select(null)} /></div>}
  </section>;
}
function detail(head = "head-1"): PrDetail {
  return { id: "PR_1", number: 1, title: "Synthetic review 1", repo: "octocat/repo-1",
    url: "https://github.com/octocat/repo-1/pull/1", state: "open", is_draft: false,
    author: "synthetic-author", body: "Retained integrated description", head_ref: "topic", base_ref: "main",
    head_oid: head, head_ref_id: null, merge_status: "clean", review: "none", latest_reviews: [],
    merge_queue_enabled: false, in_merge_queue: false, additions: 1, deletions: 0, changed_files: 1,
    unresolved_threads: 0, comment_count: 2, comments: [1, 2].map(number => ({
      id: `comment-${number}`, author: `synthetic-commenter-${number}`, created_at: "2026-10-01T12:00:00Z", author_is_bot: false,
      body: `<details><summary>Open report</summary>${head} open content</details><details><summary>Closed report</summary>${head} closed content</details>`,
    })), review_threads: [], review_threads_total: 0,
    checks: [{ name: "Integrated check", state: "success", url: "", run_id: null }], checks_total: 1 };
}
// Test-body promises are not canceled by a Vitest timeout. Guard their
// continuation before any next assertion, event emission or IPC invocation.
function ownedCase() {
  const controller = new AbortController();
  return {
    dispose: () => controller.abort(),
    step: async <T,>(work: () => Promise<T>): Promise<T> => {
      controller.signal.throwIfAborted();
      const value = await work();
      controller.signal.throwIfAborted();
      return value;
    },
  };
}
const ownedCases: ReturnType<typeof ownedCase>[] = [];

async function emit(listeners: typeof boundary.listeners, stage: keyof typeof publications.frames, revision?: number) {
  const payload = { ...publications.frames[stage], ...(revision === undefined ? {} : { revision, receipt_revision: revision }) };
  expect(remoteEventError("source-poll-status", payload)).toBeNull();
  await act(async () => { for (const callback of listeners.get("source-poll-status") ?? []) callback({ payload }); });
}
afterEach(() => { for (const scope of ownedCases.splice(0)) scope.dispose(); cleanup(); for (const client of cleanupClients.splice(0)) client.clear(); boundary.listeners.clear(); vi.clearAllMocks(); stubViewport(null); });

it.each([1200, 390])("stitches publication payloads into independent Ready/detail observers at width %s", async width => {
  stubViewport(width);
  const scope = ownedCase(); ownedCases.push(scope);
  // Each case owns its event bus and clients. A timed-out body must never
  // publish into the next case's observers when its pending await resumes.
  const listeners: typeof boundary.listeners = new Map();
  boundary.listeners = listeners;
  const clients: QueryClient[] = [];
  const publish = (stage: keyof typeof publications.frames, revision?: number) => scope.step(() => emit(listeners, stage, revision));
  let failedDetail = false;
  let firstDetail = true;
  let releaseDetail!: (value: PrDetail) => void;
  const pendingDetail = new Promise<PrDetail>(resolve => { releaseDetail = resolve; });
  let detailCalls = 0;
  let holdChangedDetail = false;
  let releaseChangedDetail!: (value: PrDetail) => void;
  const changedDetail = new Promise<PrDetail>(resolve => { releaseChangedDetail = resolve; });
  let finishRefresh: (value: unknown) => void = () => { throw new Error("refresh not started"); };
  let refreshRequest: unknown;
  let refreshSettled = false;
  let head = "head-1";
  boundary.invoke.mockImplementation(async (name, args) => {
    if (name === "get_reviewing") {
      refreshRequest = args?.requestId;
      return new Promise(resolve => { finishRefresh = resolve; });
    }
    if (name === "get_viewer") return "synthetic-viewer";
    if (name === "get_ready_stacks") return (args?.rows as object[]).map(row => ({ ...row, valid_for_ms: 60_000, stack: { kind: "none" } }));
    if (name === "get_ready_pushers") return (args?.rows as PusherAsk[]).map(row => ({ ...row, rules: { state: "unreadable", reason: "Synthetic rules unavailable" }, last_pusher: { state: "unknown", reason: "Synthetic pusher unavailable" } } satisfies RowPusher));
    if (name === "get_pr_detail") {
      detailCalls++;
      if (firstDetail) { firstDetail = false; return pendingDetail; }
      if (failedDetail) throw new Error("Synthetic read outage");
      if (holdChangedDetail) return changedDetail;
      return detail(head);
    }
    if (name === "review_pr_at_head") return { outcome: "acknowledged", receipt: { review_id: "REVIEW_1", state: "APPROVED", actor: "synthetic-viewer", commit_oid: "head-1", submitted_at: "2026-10-01T12:00:00Z", pr_id: "PR_1", repo: "octocat/repo-1", number: 1 } };
    if (name === "refresh_now") throw new Error("Synthetic read outage");
    throw new Error(`Unexpected IPC command ${name}`);
  });
  for (const name of ["desktop", "companion"]) {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client); cleanupClients.push(client);
    render(<QueryClientProvider client={client}><Observer name={name} /></QueryClientProvider>);
  }
  await scope.step(() => waitFor(() => expect(listeners.get("source-poll-status")?.size).toBe(2)));
  await publish("complete");
  const headings = screen.getAllByRole("heading", { name: /Ready for review \(20\)/ });
  expect(headings).toHaveLength(2);
  const count = (value: number) => { for (const heading of headings) expect(heading.textContent).toContain(`Ready for review (${value})`); };
  await publish("large");
  count(275);
  let refreshing!: Promise<PullRequest[]>;
  await scope.step(() => act(async () => { refreshing = refreshWithState(clients[0], "reviewing"); }));
  void refreshing.then(() => { refreshSettled = true; });
  expect(refreshRequest).toBeTypeOf("string");
  expect(refreshSettled).toBe(false);
  await publish("partial");
  expect(screen.queryByText("Synthetic review 276")).toBeNull();
  expect(screen.queryByText("Membership unconfirmed")).toBeNull(); // The failed finite step did not recheck every other row.
  const desktop = within(screen.getByTestId("desktop"));
  fireEvent.click(desktop.getByText("Synthetic review 1"));
  const detailRoot = screen.getByTestId("desktop-detail");
  const selectedDetail = within(detailRoot);
  const detailKey = ["pr-detail", "octocat/repo-1", 1];
  expect(detailCalls).toBe(1);
  expect(clients[0].getQueryState(detailKey)).toMatchObject({ status: "pending", fetchStatus: "fetching", data: undefined });
  expect(selectedDetail.queryByText("Retained integrated description")).toBeNull();
  // Own the real query completion rather than racing a broad DOM retry against
  // its one-second deadline. The list refresh remains independently pending.
  const queryCompletion = clients[0].getQueryCache().find({ queryKey: detailKey, exact: true })!.promise!;
  await scope.step(() => act(async () => {
    releaseDetail(detail(head));
    await queryCompletion;
  }));
  const detailState = clients[0].getQueryState(detailKey);
  const detailDiagnostic = JSON.stringify({ calls: detailCalls, status: detailState?.status, fetchStatus: detailState?.fetchStatus, error: detailState?.error });
  expect(detailState, detailDiagnostic).toMatchObject({ status: "success", fetchStatus: "idle" });
  await scope.step(() => selectedDetail.findByText("Retained integrated description", {}, {
    onTimeout: error => new Error(`${detailDiagnostic}\n${error.message}`),
  }));
  const comment = selectedDetail.getByText("synthetic-commenter-1").closest("button")!;
  const closedComment = selectedDetail.getByText("synthetic-commenter-2").closest("button")!;
  fireEvent.click(comment);
  const disclosures = [...detailRoot.querySelectorAll("details")];
  expect(disclosures).toHaveLength(2);
  fireEvent.click(disclosures[0].querySelector("summary")!);
  expect(disclosures.map(node => node.open)).toEqual([true, false]);
  expect(closedComment.getAttribute("aria-expanded")).toBe("false");
  failedDetail = true;
  await scope.step(() => act(async () => { await clients[0].refetchQueries({ queryKey: ["pr-detail", "octocat/repo-1", 1] }); }));
  await publish("cooldown");
  count(275);
  expect(selectedDetail.getByText("Retained integrated description")).toBeTruthy();
  expect(selectedDetail.getByRole("alert")).toBeTruthy();
  fireEvent.click(selectedDetail.getAllByRole("button", { name: "Approve" })[0]);
  await scope.step(() => waitFor(() => expect(boundary.invoke.mock.calls.filter(([name]) => name === "review_pr_at_head")).toHaveLength(1)));
  expect(boundary.invoke.mock.calls.find(([name]) => name === "review_pr_at_head")?.[1]).toMatchObject({ request: { expected_head: "head-1", expected_viewer: "synthetic-viewer" } });
  await publish("acknowledged");
  await publish("confirmed");
  await publish("recovered");
  // The command is still pending while event receipts and a foreground review
  // change the mounted list. An old reply cannot replace the newer receipt.
  expect(refreshSettled).toBe(false);
  await scope.step(() => act(async () => {
    finishRefresh({ request_id: refreshRequest, update: publications.frames.large });
    await refreshing;
  }));
  count(274);
  expect(selectedDetail.getAllByRole("button", { name: "Approved" }).length).toBeGreaterThan(0);
  failedDetail = false;
  await scope.step(() => act(async () => { await clients[0].refetchQueries({ queryKey: ["pr-detail", "octocat/repo-1", 1] }); }));
  expect(selectedDetail.getAllByRole("button", { name: "Approved" }).length).toBeGreaterThan(0);
  head = "head-1-new";
  holdChangedDetail = true;
  const beforeChangedHead = detailCalls;
  await publish("changed_head");
  // The accepted provider receipt itself must start this read. A manual
  // refetch here would conceal a disconnected source-to-detail callback.
  await scope.step(() => waitFor(() => expect(detailCalls).toBe(beforeChangedHead + 1)));
  expect(clients[0].getQueryState(detailKey)).toMatchObject({ fetchStatus: "fetching", data: { head_oid: "head-1" } });
  expect(selectedDetail.getByText("Retained integrated description")).toBeTruthy();
  expect(disclosures.map(node => node.open)).toEqual([true, false]);
  const changedCompletion = clients[0].getQueryCache().find({ queryKey: detailKey, exact: true })!.promise!;
  // New receipts with unchanged provider facts are accepted but cause no work.
  await publish("changed_head", 27);
  expect(detailCalls).toBe(beforeChangedHead + 1);
  await scope.step(() => act(async () => {
    releaseChangedDetail(detail(head));
    await changedCompletion;
  }));
  expect(clients[0].getQueryState(detailKey)).toMatchObject({ status: "success", fetchStatus: "idle", data: { head_oid: "head-1-new" } });
  await scope.step(() => waitFor(() => expect(disclosures[0].textContent).toContain("head-1-new open content")));
  await publish("changed_head", 28);
  expect(detailCalls).toBe(beforeChangedHead + 1);
  count(275);
  const after = [...detailRoot.querySelectorAll("details")];
  expect(after[0]).toBe(disclosures[0]); expect(after[1]).toBe(disclosures[1]);
  expect(after.map(node => node.open)).toEqual([true, false]);
  expect(comment.getAttribute("aria-expanded")).toBe("true");
  expect(closedComment.getAttribute("aria-expanded")).toBe("false");
  expect(after[0].textContent).toContain("head-1-new open content");
  fireEvent.click(selectedDetail.getByRole("link", { name: /^GitHub$/ }));
  await scope.step(() => waitFor(() => expect(openUrl).toHaveBeenCalledWith("https://github.com/octocat/repo-1/pull/1")));
  expect(screen.getByTestId("desktop-detail")).toBe(detailRoot);
  expect(selectedDetail.getAllByRole("button", { name: "Approve" }).some(button => !button.matches(":disabled"))).toBe(true);
  expect(boundary.invoke.mock.calls.filter(([name]) => name === "review_pr_at_head")).toHaveLength(1);
  // Another provider source must not clear either GitHub inventory.
  const foreign = { ...publications.frames.changed_head, source: { provider: "github", host: "other.synthetic.invalid" }, revision: 999, receipt_revision: 999, prs: [] };
  expect(remoteEventError("source-poll-status", foreign)).toBeNull();
  await scope.step(() => act(async () => { for (const callback of listeners.get("source-poll-status") ?? []) callback({ payload: foreign }); }));
  count(275);
  expect(detailCalls).toBe(beforeChangedHead + 1);
  const advisory = boundary.invoke.mock.calls.filter(([name]) => name === "get_ready_stacks" || name === "get_ready_pushers");
  expect(advisory.length).toBeGreaterThan(0);
  expect(advisory.every(([, args]) => (args?.rows as object[]).length <= 8)).toBe(true);
  expect(advisory.reduce((sum, [, args]) => sum + (args?.rows as object[]).length, 0)).toBeLessThan(275);
  expect(publications.cycle_http_attempts).toHaveLength(6);
  expect(publications.cycle_http_attempts.every(count => count <= 3)).toBe(true);
});


it("retires a deferred old case before it can invoke or alter successor mounted state", async () => {
  const old = ownedCase();
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  const oldBody = (async () => {
    await old.step(() => pending);
    await boundary.invoke("old-case-write");
    screen.getByTestId("successor").textContent = "contaminated";
  })();
  // A runner timeout/afterEach retires ownership while work is pending.
  old.dispose();
  const successor = vi.fn(async () => undefined);
  boundary.invoke.mockImplementation(successor);
  render(<div data-testid="successor">new case</div>);
  const outcome = oldBody.then(() => "continued", error => error.name);
  release();
  expect(await outcome).toBe("AbortError");
  expect(successor).not.toHaveBeenCalled();
  expect(screen.getByTestId("successor").textContent).toBe("new case");
});
