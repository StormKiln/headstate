import type { ReactNode } from "react";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "../fixtures/prs";
import type { PrDetail, PullRequest } from "../types/pr";
const boundary = vi.hoisted(() => ({ invoke: vi.fn(), detail: vi.fn(), listeners: new Map<string, (event: { payload: unknown }) => void>() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: boundary.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async (name: string, cb: (event: { payload: unknown }) => void) => { boundary.listeners.set(name, cb); return () => boundary.listeners.delete(name); } }));
import { usePrDetail, useReviewPr, useActOnPr } from "./hooks";
import { useSourceRefresh, refreshWithState } from "./sourceRefreshHooks";
const row: PullRequest = { ...PR_FIXTURES[0], repo: "synthetic/project", number: 1, head_oid: "h1", base_ref: "main", merge_status: "clean", review: "none", ci: "success", latest_reviews: [], comment_count: 0, unresolved_threads: 0,
  observation: { state: "observed", last_observed_at: null, unknown_fields: [], retained_fields: [] } };
const detail: PrDetail = { ...row, state: "OPEN", body: "Full retained body", merge_queue_enabled: false, comments: [], review_threads: [], review_threads_total: 0, checks: [], checks_total: 0, additions: 1, deletions: 0, changed_files: 1 };
const key = ["pr-detail", row.repo, 1];
const clients: QueryClient[] = [];
function setup(observing = true) {
  boundary.invoke.mockImplementation((name: string) => name === "get_pr_detail" ? boundary.detail() : Promise.resolve());
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(qc);
  qc.setQueryData(["viewer"], "synthetic-viewer"); qc.setQueryData(key, detail);
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  if (observing) renderHook(() => useSourceRefresh("reviewing"), { wrapper });
  return { qc, wrapper };
}
let revision = 0;
async function publish(patch: Partial<PullRequest> = {}, session = "session") {
  await act(async () => boundary.listeners.get("source-poll-status")?.({ payload: { source: { provider: "github", host: "github.com" }, list: "reviewing", phase: "ready", error: null, session, revision: ++revision, receipt_revision: revision, coverage: "complete", prs: [{ ...row, ...patch }] } }));
}
afterEach(() => { cleanup(); clients.splice(0).forEach(qc => qc.clear()); boundary.listeners.clear(); boundary.invoke.mockReset(); boundary.detail.mockReset(); vi.useRealTimers(); revision = 0; });
it("revalidates the first accepted changed head behind full data and coalesces observers and receipts", async () => {
  const { qc, wrapper } = setup();
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const first = renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  const second = renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await publish({ head_oid: "h2" });
  await waitFor(() => expect(boundary.detail).toHaveBeenCalledTimes(1));
  expect(first.result.current.data?.body).toBe("Full retained body");
  expect(second.result.current.data?.head_oid).toBe("h1");
  await publish({ head_oid: "h2", updated_at: "2030-01-01T00:00:00Z" });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  await act(async () => { finish({ ...detail, head_oid: "h2" }); });
  await waitFor(() => expect(first.result.current.data?.head_oid).toBe("h2"));
  await publish({ head_oid: "h2" });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(qc.getQueryState(key)?.isInvalidated).toBe(false);
});
it("marks inactive details stale without fetching and refreshes on revisit", async () => {
  const { qc, wrapper } = setup(); boundary.detail.mockResolvedValue({ ...detail, head_oid: "h2" });
  await publish({ head_oid: "h2" });
  expect(qc.getQueryState(key)?.isInvalidated).toBe(true);
  expect(boundary.detail).not.toHaveBeenCalled();
  const hook = renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await waitFor(() => expect(hook.result.current.data?.head_oid).toBe("h2"));
  expect(boundary.detail).toHaveBeenCalledTimes(1);
});
it("coalesces an overtaken h2 read into one h3 catch-up", async () => {
  const { wrapper } = setup(); let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValue({ ...detail, head_oid: "h3" });
  const hook = renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await publish({ head_oid: "h2" });
  await waitFor(() => expect(boundary.detail).toHaveBeenCalledTimes(1));
  await publish({ head_oid: "h3" }); await publish({ head_oid: "h3" });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  await act(async () => finish({ ...detail, head_oid: "h2" }));
  await waitFor(() => expect(hook.result.current.data?.head_oid).toBe("h3"));
  expect(boundary.detail).toHaveBeenCalledTimes(2);
});
it.each(["failure", "old success"])("keeps the unresolved target after %s and recovers through bounded detail backoff", async mode => {
  vi.useFakeTimers();
  const { qc, wrapper } = setup();
  if (mode === "failure") boundary.detail.mockRejectedValueOnce(new Error("Synthetic outage"));
  else boundary.detail.mockResolvedValueOnce(detail);
  boundary.detail.mockResolvedValue({ ...detail, head_oid: "h2" });
  const hook = renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await publish({ head_oid: "h2" });
  await act(async () => { await vi.advanceTimersByTimeAsync(10); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(qc.getQueryData<PrDetail>(key)?.body).toBe("Full retained body");
  await publish({ head_oid: "h2" });
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  await act(async () => { await vi.advanceTimersByTimeAsync(10000); });
  expect(hook.result.current.data?.head_oid).toBe("h2");
  expect(boundary.detail).toHaveBeenCalledTimes(2);
  await publish({ head_oid: "h2" });
  await act(async () => { await vi.advanceTimersByTimeAsync(120000); });
  expect(boundary.detail).toHaveBeenCalledTimes(2);
});
it.each([
  { is_draft: true }, { ci: "failure" }, { merge_status: "behind" }, { review: "approved" }, { in_merge_queue: true },
  { base_ref: "release" }, { comment_count: 1 }, { unresolved_threads: 1 },
  { requested_reviewers: ["new-reviewer"], requested_reviewers_total: 1 },
  { latest_reviews: [{ author: "reviewer", state: "APPROVED" }], latest_reviews_total: 1 },
] satisfies Partial<PullRequest>[])("refreshes a positively changed same-head fact %j once", async patch => {
  const { wrapper } = setup();
  boundary.detail.mockResolvedValue({ ...detail, ...patch });
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  const observation: PullRequest["observation"] = { ...row.observation!, detail_fields: ["base", "comments", "threads", "reviewers", "reviews"] };
  await publish({ observation });
  expect(boundary.detail).not.toHaveBeenCalled();
  await publish({ ...patch, observation });
  await waitFor(() => expect(boundary.detail).toHaveBeenCalledTimes(1));
  await publish({ ...patch, observation });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
});
it("ignores retained/refused facts and old-wire discussion defaults", async () => {
  const { wrapper } = setup(); renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await publish();
  await publish({ head_oid: "h2", comment_count: 20, observation: { ...row.observation!, state: "retained" } });
  await publish({ head_oid: "", ci: "none", is_draft: true, review: "approved", in_merge_queue: true, comment_count: 20,
    observation: { ...row.observation!, unknown_fields: ["head", "ci", "draft", "review", "queue"], detail_fields: [] } });
  await publish({ comment_count: 20, requested_reviewers: ["unqualified"] });
  expect(boundary.detail).not.toHaveBeenCalled();
});
it("does not let a retired source session completion replace the full detail", async () => {
  vi.useFakeTimers(); const { qc, wrapper } = setup();
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValue({ ...detail, head_oid: "h3" });
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await publish({ head_oid: "h2" }, "old");
  await publish({ head_oid: "h3" }, "new");
  await act(async () => finish({ ...detail, head_oid: "h2" }));
  expect(qc.getQueryData<PrDetail>(key)?.head_oid).toBe("h1");
  await publish({ head_oid: "retired" }, "old");
  await act(async () => { await vi.advanceTimersByTimeAsync(15000); });
  expect(qc.getQueryData<PrDetail>(key)?.head_oid).toBe("h3");
  expect(boundary.detail).toHaveBeenCalledTimes(2);
});
it("drops old account targets and refuses their late completions", async () => {
  vi.useFakeTimers(); const { qc, wrapper } = setup(); let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await publish({ head_oid: "h2" });
  await act(async () => { qc.setQueryData(["viewer"], "other-viewer"); finish({ ...detail, head_oid: "h2" }); });
  expect(qc.getQueryData<PrDetail>(key)?.head_oid).toBe("h1");
  await act(async () => { await vi.advanceTimersByTimeAsync(120000); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
});
it("stops recovery after a same-head successful read whose full data is structurally unchanged", async () => {
  vi.useFakeTimers(); const { wrapper } = setup(); boundary.detail.mockResolvedValue(detail);
  // CI is a source aggregate; an accepted changed aggregate asks for a full
  // read even when its mapped check window happens to remain identical.
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await publish(); await publish({ ci: "failure" });
  await act(async () => { await vi.advanceTimersByTimeAsync(120000); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
});
it("defers to the head-bound review readback and preserves its confirmed authority during catch-up", async () => {
  const { qc, wrapper } = setup();
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValue(detail);
  const receipt = { review_id: "review-1", state: "APPROVED", actor: "synthetic-viewer", commit_oid: "h1", submitted_at: "2026-10-01T00:00:00Z", pr_id: detail.id, repo: row.repo, number: 1 };
  boundary.invoke.mockImplementation((name: string) => {
    if (name === "get_pr_detail") return boundary.detail();
    if (name === "review_pr_at_head") return Promise.resolve({ outcome: "acknowledged", receipt });
    if (name === "refresh_now") return Promise.resolve([row]);
    return Promise.resolve();
  });
  const hook = renderHook(() => ({ detail: usePrDetail(row.repo, 1), review: useReviewPr() }), { wrapper });
  await publish();
  await act(async () => { await hook.result.current.review(row.id, row.repo, 1, "approve", "", "h1", "synthetic-viewer"); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  await publish({ ci: "failure" }); await publish({ ci: "failure" });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(qc.getQueryData<PrDetail>(key)?.latest_reviews).toContainEqual(expect.objectContaining({ id: "review-1", state: "APPROVED" }));
  await act(async () => finish(detail));
  await waitFor(() => expect(boundary.detail).toHaveBeenCalledTimes(2));
  expect(qc.getQueryData<PrDetail>(key)?.latest_reviews).toContainEqual(expect.objectContaining({ id: "review-1", state: "APPROVED" }));
  expect(boundary.invoke.mock.calls.find(([name]) => name === "review_pr_at_head")?.[1]).toMatchObject({ request: { expected_head: "h1", expected_viewer: "synthetic-viewer" } });
  await publish({ ci: "failure" }); expect(boundary.detail).toHaveBeenCalledTimes(2);
});
it("ignores an old session accepted independently by the other source list", async () => {
  const { qc, wrapper } = setup();
  boundary.detail.mockResolvedValueOnce({ ...detail, head_oid: "h2" }).mockResolvedValue({ ...detail, head_oid: "h3" });
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await publish({ head_oid: "h2" }, "old");
  await publish({ head_oid: "h3" }, "new");
  await waitFor(() => expect(qc.getQueryData<PrDetail>(key)?.head_oid).toBe("h3"));
  expect(boundary.detail).toHaveBeenCalledTimes(2);
  boundary.invoke.mockImplementation((name: string) => {
    if (name === "get_pr_detail") return boundary.detail();
    if (name === "refresh_now") return Promise.resolve({ request_id: "unused", update: { source: { provider: "github", host: "github.com" }, list: "authored", phase: "ready", error: null, session: "old", revision: 1, receipt_revision: 1, prs: [{ ...row, head_oid: "retired" }] } });
    return Promise.resolve();
  });
  await act(async () => { await refreshWithState(qc, "authored"); });
  expect(boundary.detail).toHaveBeenCalledTimes(2);
});
it("does not treat an unknown merge-status default as a positive change", async () => {
  const { wrapper } = setup(); renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await publish(); await publish({ merge_status: "unknown" });
  expect(boundary.detail).not.toHaveBeenCalled();
});
it.each([
  { is_draft: true }, { in_merge_queue: true }, { review: "approved" }, { merge_status: "behind" },
] satisfies Partial<PullRequest>[])("keeps an unresolved comparable same-head scalar after an old successful read: %j", async patch => {
  vi.useFakeTimers(); const { qc, wrapper } = setup();
  boundary.detail.mockResolvedValueOnce(detail).mockResolvedValue({ ...detail, ...patch });
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await publish(); await publish(patch);
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(qc.getQueryData<PrDetail>(key)).toMatchObject({ is_draft: false, in_merge_queue: false, review: "none", merge_status: "clean" });
  await publish(patch);
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  await act(async () => { await vi.advanceTimersByTimeAsync(15000); });
  expect(qc.getQueryData<PrDetail>(key)).toMatchObject(patch);
  expect(boundary.detail).toHaveBeenCalledTimes(2);
  await publish(patch);
  await act(async () => { await vi.advanceTimersByTimeAsync(120000); });
  expect(boundary.detail).toHaveBeenCalledTimes(2);
});
it("does not compare source total comments to the full detail issue-comment count", async () => {
  const { wrapper } = setup(); renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  const observation: PullRequest["observation"] = { ...row.observation!, detail_fields: ["comments"] };
  await publish({ comment_count: 12, observation });
  expect(boundary.detail).not.toHaveBeenCalled();
  boundary.detail.mockResolvedValue(detail);
  await publish({ comment_count: 13, observation });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
});
it("keeps a disagreeing scalar bounded until a later accepted source fact supersedes it", async () => {
  vi.useFakeTimers(); const { qc, wrapper } = setup();
  boundary.detail.mockResolvedValue({ ...detail, merge_status: "unstable" });
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await publish(); await publish({ merge_status: "behind" });
  expect(qc.getQueryData<PrDetail>(key)?.merge_status).toBe("unstable");
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  // There is no ordering proof between unequal statuses. A positive later
  // source observation replaces the target instead of a guessed timestamp.
  await publish({ merge_status: "unstable" });
  expect(boundary.detail).toHaveBeenCalledTimes(2);
  await act(async () => { await vi.advanceTimersByTimeAsync(120000); });
  expect(boundary.detail).toHaveBeenCalledTimes(2);
});
it("catches a source head received during a cold full-detail read without replacing its placeholder", async () => {
  const { qc, wrapper } = setup(); qc.removeQueries({ queryKey: key, exact: true }); qc.setQueryData(["reviewing"], [row]);
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValue({ ...detail, head_oid: "h2" });
  const hook = renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  expect(hook.result.current.isPlaceholderData).toBe(true);
  await publish({ head_oid: "h2" });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  await act(async () => finish(detail));
  await waitFor(() => expect(hook.result.current.data?.head_oid).toBe("h2"));
  expect(boundary.detail).toHaveBeenCalledTimes(2);
});

it.each((["draft", "enqueue"] as const).flatMap(action => [true, false].map(observing => ({ action, observing }))))(
  "keeps acknowledged $action patches out of provider targets (source observer: $observing)", async ({ action, observing }) => {
    vi.useFakeTimers();
    const { qc, wrapper } = setup(observing);
    const patch = action === "draft" ? { is_draft: true } : { in_merge_queue: true };
    let answer: { request_id: string; update: import("./sourceRefresh").SourceStatus } = { request_id: "fixture", update: {
      source: { provider: "github", host: "github.com" }, list: "reviewing", phase: "ready", error: null,
      session: "session", revision: 1, receipt_revision: 1, prs: [row], coverage: "complete",
    } };
    let failed = false;
    boundary.invoke.mockImplementation((name: string) => {
      if (name === "get_pr_detail") return boundary.detail();
      if (name === "refresh_now") return Promise.resolve([]);
      if (name === "get_reviewing") return failed ? Promise.reject(new Error("Synthetic rejected read")) : Promise.resolve(answer);
      return Promise.resolve();
    });
    await act(async () => { await refreshWithState(qc, "reviewing"); });
    const pending: ((value: PrDetail) => void)[] = [];
    boundary.detail.mockImplementation(() => new Promise(resolve => { pending.push(resolve); }));
    const hook = renderHook(() => ({ detail: usePrDetail(row.repo, 1), act: useActOnPr() }), { wrapper });
    await act(async () => { await hook.result.current.act(row.id, row.repo, 1, action); });
    expect(qc.getQueryData<PullRequest[]>(["reviewing"])?.[0]).toMatchObject(patch);
    // Normal action invalidation owns exactly one held full read. A list patch
    // must not launch a competing source read that action invalidation cancels.
    expect(boundary.detail).toHaveBeenCalledTimes(1);
    await act(async () => { pending.forEach(resolve => resolve(detail)); });
    // Same receipt, older receipt and status-only replies return the preserved
    // local patch for display, never as a new provider publication.
    for (const update of [answer.update, { ...answer.update, revision: 0, receipt_revision: 0 }, { ...answer.update, revision: 2, receipt_revision: null, prs: null }]) {
      answer = { ...answer, update };
      await act(async () => { await refreshWithState(qc, "reviewing"); });
      expect(qc.getQueryData<PullRequest[]>(["reviewing"])?.[0]).toMatchObject(patch);
      expect(boundary.detail).toHaveBeenCalledTimes(1);
    }
    failed = true;
    await act(async () => { await expect(refreshWithState(qc, "reviewing")).rejects.toThrow("Synthetic rejected read"); });
    await act(async () => { await vi.advanceTimersByTimeAsync(120000); });
    expect(boundary.detail).toHaveBeenCalledTimes(1);
    failed = false;
    // A genuinely new receipt whose provider facts are unchanged also adds no
    // detail work, even though it replaces the locally patched display row.
    answer = { ...answer, update: { ...answer.update, revision: 3, receipt_revision: 2, prs: [row] } };
    await act(async () => { await refreshWithState(qc, "reviewing"); });
    expect(qc.getQueryData<PullRequest[]>(["reviewing"])?.[0]).toMatchObject({ is_draft: false, in_merge_queue: false });
    expect(boundary.detail).toHaveBeenCalledTimes(1);
    // Provider-confirmed evidence must still be compared to the last provider
    // row, even when it happens to equal the earlier acknowledged local patch.
    answer = { ...answer, update: { ...answer.update, revision: 4, receipt_revision: 3, prs: [{ ...row, ...patch }] } };
    boundary.detail.mockResolvedValue({ ...detail, ...patch });
    await act(async () => { await refreshWithState(qc, "reviewing"); });
    expect(boundary.detail).toHaveBeenCalledTimes(2);
    expect(qc.getQueryData<PrDetail>(key)).toMatchObject(patch);
    await act(async () => { await vi.advanceTimersByTimeAsync(120000); });
    expect(boundary.detail).toHaveBeenCalledTimes(2);
  },
);
