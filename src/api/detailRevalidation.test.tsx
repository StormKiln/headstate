import type { ReactNode } from "react";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "../fixtures/prs";
import targetedPublications from "../../src-tauri/tests/fixtures/targeted-fact-publications.json";
import { assertRemoteReply } from "./wireContract";
import type { PrDetail, PullRequest } from "../types/pr";
const boundary = vi.hoisted(() => ({ invoke: vi.fn(), detail: vi.fn(), listeners: new Map<string, (event: { payload: unknown }) => void>() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: boundary.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async (name: string, cb: (event: { payload: unknown }) => void) => { boundary.listeners.set(name, cb); return () => boundary.listeners.delete(name); } }));
import { acceptDetailFacts, detailNeedsRevalidation, retireDetailOwnership } from "./detailRevalidation";
import { usePrDetail, useReviewPr, useActOnPr } from "./hooks";
import { useSourceRefresh, refreshWithState, readRetained, retireSourceOwnership } from "./sourceRefreshHooks";
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
  await act(async () => boundary.listeners.get("source-poll-status")?.({ payload: { source: { provider: "github", host: "github.com" }, list: "reviewing", owner: "synthetic-viewer", phase: "ready", error: null, session, revision: ++revision, receipt_revision: revision, coverage: "complete", prs: [{ ...row, ...patch }] } }));
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
it("rejects an old session from the other source list before detail acceptance", async () => {
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
  await act(async () => { await expect(refreshWithState(qc, "authored")).rejects.toThrow("The desktop session changed"); });
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

async function inventory(prs: PullRequest[], coverage: import("./sourceRefresh").SourceCoverage = "complete", extra = {}) {
  await act(async () => boundary.listeners.get("source-poll-status")?.({ payload: { source: { provider: "github", host: "github.com" }, list: "reviewing", owner: "synthetic-viewer", phase: "ready", error: null, session: "session", revision: ++revision, receipt_revision: revision, coverage, prs, ...extra } }));
}
it.each(["OPEN", "MERGED"] as const)("probes a complete disappearance once and accepts %s without predicting it", async state => {
  vi.useFakeTimers(); const { qc, wrapper } = setup();
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await inventory([row]); await inventory([]); await inventory([]);
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(qc.getQueryData<PrDetail>(key)?.body).toBe(detail.body);
  expect(qc.getQueryData<PrDetail>(key)?.state).toBe("OPEN");
  await act(async () => finish({ ...detail, state }));
  await inventory([]);
  await act(async () => { await vi.advanceTimersByTimeAsync(600000); });
  expect(qc.getQueryData<PrDetail>(key)?.state).toBe(state);
  expect(boundary.detail).toHaveBeenCalledTimes(1);
});
it("marks an absent inactive detail stale and reads once on revisit", async () => {
  const { qc, wrapper } = setup(); boundary.detail.mockResolvedValue(detail);
  await inventory([row]); await inventory([]);
  expect(qc.getQueryState(key)?.isInvalidated).toBe(true);
  expect(boundary.detail).not.toHaveBeenCalled();
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await waitFor(() => expect(boundary.detail).toHaveBeenCalledTimes(1));
});
it.each(["unknown", { partial: { total: 3 } }, { partial: { total: null } }] as const)("does not infer absence from %j coverage", async coverage => {
  const { qc, wrapper } = setup(); renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await inventory([row]); await inventory([], coverage);
  expect(boundary.detail).not.toHaveBeenCalled();
  expect(qc.getQueryState(key)?.isInvalidated).toBe(false);
});
it("does not infer absence from the first empty receipt or a different session", async () => {
  const { wrapper } = setup(); renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await inventory([]); await inventory([row]); await inventory([], "complete", { session: "new" });
  await inventory([], "complete", { session: "session" });
  expect(boundary.detail).not.toHaveBeenCalled();
});

it("coalesces both inventory removals but retains independent membership", async () => {
  vi.useFakeTimers(); const { wrapper, qc } = setup();
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  const authored = async (prs: PullRequest[]) => {
    boundary.invoke.mockImplementation((name: string) => name === "get_pr_detail" ? boundary.detail() : Promise.resolve({ request_id: "fixture", update: { source: { provider: "github", host: "github.com" }, list: "authored", session: "session", owner: "synthetic-viewer", revision: ++revision, receipt_revision: revision, phase: "ready", error: null, coverage: "complete", prs } }));
    await act(async () => { await refreshWithState(qc, "authored"); });
  };
  await inventory([row]); await authored([row]);
  await inventory([]); await authored([]);
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  await act(async () => finish(detail));
  await inventory([]); await authored([]);
  await act(async () => { await vi.advanceTimersByTimeAsync(600000); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  // Re-observation in reviewing establishes only that list's membership.
  await inventory([row]); await authored([]);
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  await inventory([]); expect(boundary.detail).toHaveBeenCalledTimes(2);
  await act(async () => finish(detail));
});
it("a positive new fact overtaking a missing-row probe gets one necessary catch-up", async () => {
  const { wrapper, qc } = setup(); let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValue({ ...detail, head_oid: "h2" });
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await inventory([row]); await inventory([]); await inventory([{ ...row, head_oid: "h2" }]);
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  await act(async () => finish(detail));
  await waitFor(() => expect(qc.getQueryData<PrDetail>(key)?.head_oid).toBe("h2"));
  expect(boundary.detail).toHaveBeenCalledTimes(2);
});
it.each(["status", "retained", "unrelated", "account", "retired"])("does not turn %s non-observation into absence", async mode => {
  const { qc, wrapper } = setup(); renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  if (mode === "retained") {
    boundary.invoke.mockResolvedValueOnce({ source: { provider: "github", host: "github.com" }, list: "reviewing", session: "session", ownership: { state: "credential_bound", owner: "synthetic-viewer" }, data: { state: "available", prs: [row], coverage: "complete", fetched_at: "2026-01-01T00:00:00Z", stale_secs: 86400 } });
    await act(async () => { await readRetained(qc, "reviewing"); });
  } else await inventory([row]);
  if (mode === "status") await inventory([], "complete", { receipt_revision: null, prs: null });
  else if (mode === "unrelated") await inventory([], "complete", { list: "authored" });
  else if (mode === "account") {
    act(() => { qc.setQueryData(["viewer"], "other-viewer"); });
    await inventory([]);
  } else if (mode === "retired") {
    const old = boundary.listeners.get("source-poll-status");
    act(() => retireSourceOwnership(qc));
    await act(async () => old?.({ payload: { source: { provider: "github", host: "github.com" }, list: "reviewing", session: "session", revision: 99, receipt_revision: 99, prs: [], coverage: "complete", phase: "ready", error: null } }));
  } else await inventory([]);
  expect(boundary.detail).not.toHaveBeenCalled();
});
it("retains detail on a failed disappearance probe and recovers once with bounded backoff", async () => {
  vi.useFakeTimers(); const { qc, wrapper } = setup();
  boundary.detail.mockRejectedValueOnce(new Error("probe unavailable")).mockResolvedValue(detail);
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await inventory([row]); await inventory([]);
  expect(qc.getQueryData<PrDetail>(key)?.body).toBe(detail.body);
  await inventory([]); expect(boundary.detail).toHaveBeenCalledTimes(1);
  await act(async () => { await vi.advanceTimersByTimeAsync(15000); });
  expect(boundary.detail).toHaveBeenCalledTimes(2);
  await act(async () => { await vi.advanceTimersByTimeAsync(600000); });
  expect(boundary.detail).toHaveBeenCalledTimes(2);
});
it("matches disappearance to the same PR despite repository casing changes", async () => {
  const { qc, wrapper } = setup(); boundary.detail.mockResolvedValue(detail);
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await inventory([row]); await inventory([{ ...row, repo: row.repo.toUpperCase() }]);
  expect(boundary.detail).not.toHaveBeenCalled();
  await inventory([]);
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(qc.getQueryData<PrDetail>(key)?.state).toBe("OPEN");
});

it.each(["disappearance", "changed head"] as const)("revalidates %s on the active alias and only stales the older inactive alias", async mode => {
  vi.useFakeTimers();
  const { qc, wrapper } = setup();
  const upperRepo = row.repo.toUpperCase(); const upperKey = ["pr-detail", upperRepo, 1];
  qc.setQueryData(upperKey, { ...detail, repo: upperRepo });
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  renderHook(() => usePrDetail(upperRepo, 1), { wrapper });
  await inventory([row]);
  const next = mode === "disappearance" ? [] : [{ ...row, repo: upperRepo, head_oid: "h2" }];
  await inventory(next); await inventory(next);
  expect(qc.getQueryState(key)?.isInvalidated).toBe(true);
  expect(qc.getQueryState(upperKey)?.isInvalidated).toBe(true);
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(qc.getQueryData<PrDetail>(upperKey)?.body).toBe(detail.body);
  await act(async () => finish({ ...detail, repo: upperRepo, head_oid: mode === "disappearance" ? "h1" : "h2" }));
  expect(detailNeedsRevalidation(qc, upperRepo, 1)).toBe(false);
  expect(detailNeedsRevalidation(qc, row.repo, 1)).toBe(true);
  expect(qc.getQueryState(key)?.isInvalidated).toBe(true);
  await inventory(next);
  await act(async () => { await vi.advanceTimersByTimeAsync(600000); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
});
it("consumes the actual active alias target after an explicit full OPEN read", async () => {
  const { qc, wrapper } = setup(); const upperRepo = row.repo.toUpperCase();
  qc.setQueryData(["pr-detail", upperRepo, 1], { ...detail, repo: upperRepo });
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const hook = renderHook(() => usePrDetail(upperRepo, 1), { wrapper });
  await inventory([row]); await inventory([]);
  let reading!: ReturnType<typeof hook.result.current.refetch>;
  act(() => { reading = hook.result.current.refetch({ cancelRefetch: false }); });
  await act(async () => { finish({ ...detail, repo: upperRepo }); await reading; });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(detailNeedsRevalidation(qc, upperRepo, 1)).toBe(false);
  expect(detailNeedsRevalidation(qc, row.repo, 1)).toBe(true);
});
it.each(["disappearance", "changed head"] as const)("coalesces pending %s receipts independently for two active aliases", async mode => {
  vi.useFakeTimers(); const { qc, wrapper } = setup(); const upperRepo = row.repo.toUpperCase();
  const upperKey = ["pr-detail", upperRepo, 1]; qc.setQueryData(upperKey, { ...detail, repo: upperRepo });
  const pending: ((value: PrDetail) => void)[] = [];
  boundary.detail.mockImplementation(() => new Promise(resolve => { pending.push(resolve); }));
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  renderHook(() => usePrDetail(upperRepo, 1), { wrapper });
  await inventory([row]);
  const next = mode === "disappearance" ? [] : [{ ...row, repo: upperRepo, head_oid: "h2" }];
  await inventory(next); await inventory(next); await inventory(next);
  expect(boundary.detail).toHaveBeenCalledTimes(2);
  expect(detailNeedsRevalidation(qc, row.repo, 1)).toBe(true);
  expect(detailNeedsRevalidation(qc, upperRepo, 1)).toBe(true);
  await act(async () => pending[0]({ ...detail, head_oid: mode === "disappearance" ? "h1" : "h2" }));
  expect(detailNeedsRevalidation(qc, row.repo, 1)).toBe(false);
  expect(detailNeedsRevalidation(qc, upperRepo, 1)).toBe(true);
  await act(async () => pending[1]({ ...detail, repo: upperRepo, head_oid: mode === "disappearance" ? "h1" : "h2" }));
  expect(detailNeedsRevalidation(qc, upperRepo, 1)).toBe(false);
  await inventory(next); await act(async () => { await vi.advanceTimersByTimeAsync(600000); });
  expect(boundary.detail).toHaveBeenCalledTimes(2);
});
it("marks all inactive aliases stale without transport and consumes only the revisited alias", async () => {
  const { qc, wrapper } = setup(); const upperRepo = row.repo.toUpperCase(); const upperKey = ["pr-detail", upperRepo, 1];
  qc.setQueryData(upperKey, { ...detail, repo: upperRepo });
  boundary.detail.mockResolvedValue({ ...detail, repo: upperRepo });
  await inventory([row]); await inventory([]);
  expect(qc.getQueryState(key)?.isInvalidated).toBe(true);
  expect(qc.getQueryState(upperKey)?.isInvalidated).toBe(true);
  expect(boundary.detail).not.toHaveBeenCalled();
  renderHook(() => usePrDetail(upperRepo, 1), { wrapper });
  await waitFor(() => expect(boundary.detail).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(detailNeedsRevalidation(qc, upperRepo, 1)).toBe(false));
  expect(detailNeedsRevalidation(qc, row.repo, 1)).toBe(true);
  expect(qc.getQueryState(key)?.isInvalidated).toBe(true);
});

it.each(targetedPublications.terminal_details.map(detail => [detail.state, detail] as const))("consumes native removal emitted before the %s detail command returns without another read", async (_state, nativeDetail) => {
  vi.useFakeTimers();
  const { qc, wrapper } = setup();
  await inventory([row]);
  qc.removeQueries({ queryKey: key, exact: true });
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  await inventory([]);
  assertRemoteReply("get_pr_detail", nativeDetail);
  await act(async () => finish({ ...nativeDetail, repo: row.repo, number: row.number, id: row.id, head_oid: row.head_oid } as PrDetail));
  await act(async () => { await vi.advanceTimersByTimeAsync(600000); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(detailNeedsRevalidation(qc, row.repo, 1)).toBe(false);
});

it.each(targetedPublications.terminal_details.map(detail => [detail.state, detail] as const))("consumes native removal delivered after the %s detail reply without another read", async (_state, nativeDetail) => {
  vi.useFakeTimers();
  const { qc, wrapper } = setup();
  const actual = { ...nativeDetail, repo: row.repo, number: row.number, id: row.id, head_oid: row.head_oid } as PrDetail;
  await inventory([row]);
  qc.removeQueries({ queryKey: key, exact: true });
  boundary.detail.mockResolvedValue(actual);
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await act(async () => { await vi.advanceTimersByTimeAsync(1); });
  expect(qc.getQueryData<PrDetail>(key)?.state).toBe(actual.state);
  await inventory([]);
  await act(async () => { await vi.advanceTimersByTimeAsync(600000); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(detailNeedsRevalidation(qc, row.repo, 1)).toBe(false);
});

it.each(["positive-membership", "different-node", "different-head", "manual-cache", "new-session"])("does not suppress absence read after %s", async mode => {
  vi.useFakeTimers();
  const { qc, wrapper } = setup();
  const actual = { ...targetedPublications.terminal_details[0], repo: row.repo, number: row.number, id: mode === "different-node" ? "OTHER_NODE" : row.id, head_oid: mode === "different-head" ? "other-head" : row.head_oid } as PrDetail;
  await inventory([row]);
  qc.removeQueries({ queryKey: key, exact: true });
  boundary.detail.mockResolvedValue(actual);
  if (mode === "manual-cache") qc.setQueryData(key, actual);
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await act(async () => { await vi.advanceTimersByTimeAsync(1); });
  if (mode === "positive-membership") await inventory([row]);
  const source = mode === "new-session" ? { session: "new-session" } : {};
  if (mode === "new-session") await inventory([row], "complete", source);
  const before = boundary.detail.mock.calls.length;
  await inventory([], "complete", source);
  await act(async () => { await vi.advanceTimersByTimeAsync(1); });
  expect(boundary.detail).toHaveBeenCalledTimes(before + 1);
});

it("retires the successful terminal witness with its old account query", async () => {
  vi.useFakeTimers();
  const { qc, wrapper } = setup();
  const actual = { ...targetedPublications.terminal_details[0], repo: row.repo, number: row.number, id: row.id, head_oid: row.head_oid } as PrDetail;
  await inventory([row]); qc.removeQueries({ queryKey: key, exact: true });
  boundary.detail.mockResolvedValue(actual);
  const mounted = renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await act(async () => { await vi.advanceTimersByTimeAsync(1); });
  await inventory([row], "complete", { owner: "other-viewer" });
  expect(qc.getQueryData(key)).toBeUndefined();
  mounted.unmount();
  qc.setQueryData(["viewer"], "other-viewer");
  qc.setQueryData(key, actual); // Display-only cache is not a successful read witness.
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  await act(async () => acceptDetailFacts(qc, { rows: [row], coverage: "complete", list: "reviewing", session: "new-owner-session" }));
  const before = boundary.detail.mock.calls.length;
  await act(async () => acceptDetailFacts(qc, { rows: [], coverage: "complete", list: "reviewing", session: "new-owner-session" }));
  await act(async () => { await vi.advanceTimersByTimeAsync(1); });
  expect(boundary.detail).toHaveBeenCalledTimes(before + 1);
});

it.each([{ in_merge_queue: true }, { is_draft: true }, { review: "approved" as const }, { merge_status: "behind" as const }])("consumes its own fresh scalar publication before detail returns: %j", async patch => {
  vi.useFakeTimers(); const { qc, wrapper } = setup();
  await inventory([row]);
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  void qc.invalidateQueries({ queryKey: key, exact: true });
  await act(async () => {});
  await inventory([{ ...row, ...patch }]);
  await act(async () => finish({ ...detail, ...patch }));
  await act(async () => { await vi.advanceTimersByTimeAsync(600000); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(detailNeedsRevalidation(qc, row.repo, 1)).toBe(false);
});

it.each(["identity", "checks", "comments"])("does not let a matching scalar readback consume an overtaken %s target", async reason => {
  const { qc, wrapper } = setup(); await inventory([{ ...row, observation: { ...row.observation!, detail_fields: ["comments"] } }]);
  let finish!: (value: PrDetail) => void;
  const next = { ...detail, in_merge_queue: true, ...(reason === "identity" ? { id: "replacement-node" } : {}) };
  boundary.detail.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValue(next);
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  void qc.invalidateQueries({ queryKey: key, exact: true }); await act(async () => {});
  const patch = reason === "identity" ? { id: "replacement-node" } : reason === "checks" ? { ci: "failure" as const } : { comment_count: 2, observation: { ...row.observation!, detail_fields: ["comments" as const] } };
  await inventory([{ ...row, ...patch, in_merge_queue: true }]);
  await act(async () => finish({ ...detail, in_merge_queue: true }));
  await waitFor(() => expect(boundary.detail).toHaveBeenCalledTimes(2));
  expect(qc.getQueryData<PrDetail>(key)?.id).toBe(next.id);
});

it("still revalidates a terminal detail when a positive source observes a reopened new head", async () => {
  const { qc, wrapper } = setup();
  const terminal = targetedPublications.terminal_details[0] as PrDetail;
  qc.setQueryData(key, terminal);
  boundary.detail.mockResolvedValue({ ...detail, state: "open", head_oid: "reopened-head" });
  renderHook(() => usePrDetail(row.repo, 1), { wrapper });
  expect(boundary.detail).not.toHaveBeenCalled();
  await inventory([{ ...row, head_oid: "reopened-head" }]);
  await waitFor(() => expect(qc.getQueryData<PrDetail>(key)?.state).toBe("open"));
  expect(boundary.detail).toHaveBeenCalledTimes(1);
});

it("accepts the complete post-approval read and its own scalar publication without a second request", async () => {
  vi.useFakeTimers();
  const { qc, wrapper } = setup();
  const receipt = { review_id: "review-1", state: "APPROVED", actor: "synthetic-viewer", commit_oid: "h1", submitted_at: "2026-10-07T00:00:00Z", pr_id: detail.id, repo: row.repo, number: 1 };
  const fresh: PrDetail = { ...detail, body: "Newly retrieved full content", review: "approved", latest_reviews: [{ id: "review-1", state: "APPROVED", author: "synthetic-viewer", commit_oid: "h1", submitted_at: receipt.submitted_at }] };
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValue(fresh);
  boundary.invoke.mockImplementation((name: string) => {
    if (name === "get_pr_detail") return boundary.detail();
    if (name === "review_pr_at_head") return Promise.resolve({ outcome: "acknowledged", receipt });
    return Promise.resolve();
  });
  const view = renderHook(() => ({ detail: usePrDetail(row.repo, 1), review: useReviewPr() }), { wrapper });
  await publish();
  await act(async () => { await view.result.current.review(row.id, row.repo, 1, "approve", "", "h1", "synthetic-viewer"); });
  await publish({ review: "approved" });
  await act(async () => finish(fresh));
  await act(async () => { await vi.advanceTimersByTimeAsync(120000); });
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(qc.getQueryData<PrDetail>(key)?.body).toBe("Newly retrieved full content");
  expect(detailNeedsRevalidation(qc, row.repo, 1)).toBe(false);
});

it("coalesces manual detail refresh with a pending acknowledged review readback", async () => {
  const { qc, wrapper } = setup();
  const receipt = { review_id: "review-1", state: "APPROVED", actor: "synthetic-viewer", commit_oid: "h1", submitted_at: "2026-10-07T00:00:00Z", pr_id: detail.id, repo: row.repo, number: 1 };
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  boundary.invoke.mockImplementation((name: string) => {
    if (name === "get_pr_detail") return boundary.detail();
    if (name === "review_pr_at_head") return Promise.resolve({ outcome: "acknowledged", receipt });
    return Promise.resolve();
  });
  const view = renderHook(() => ({ detail: usePrDetail(row.repo, 1), review: useReviewPr() }), { wrapper });
  await act(async () => { await view.result.current.review(row.id, row.repo, 1, "approve", "", "h1", "synthetic-viewer"); });
  const refresh = view.result.current.detail.refetch({ cancelRefetch: false });
  await act(async () => finish(detail));
  await refresh;
  expect(boundary.detail).toHaveBeenCalledTimes(1);
  expect(qc.getQueryData<PrDetail>(key)?.latest_reviews).toContainEqual(expect.objectContaining({ id: "review-1", state: "APPROVED" }));
});

it.each(["cancelled", "account", "source", "head", "identity"] as const)("discards %s post-approval readback without replacing newer cached content", async cause => {
  const { qc, wrapper } = setup();
  const receipt = { review_id: "review-1", state: "APPROVED", actor: "synthetic-viewer", commit_oid: "h1", submitted_at: "2026-10-07T00:00:00Z", pr_id: detail.id, repo: row.repo, number: 1 };
  let finish!: (value: PrDetail) => void;
  boundary.detail.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  boundary.invoke.mockImplementation((name: string) => {
    if (name === "get_pr_detail") return boundary.detail();
    if (name === "review_pr_at_head") return Promise.resolve({ outcome: "acknowledged", receipt });
    return Promise.resolve();
  });
  const view = renderHook(() => ({ detail: usePrDetail(row.repo, 1), review: useReviewPr() }), { wrapper });
  await act(async () => { await view.result.current.review(row.id, row.repo, 1, "approve", "", "h1", "synthetic-viewer"); });
  await act(async () => {
    if (cause === "cancelled") await qc.cancelQueries({ queryKey: key, exact: true });
    if (cause === "account") qc.setQueryData(["viewer"], "new-viewer");
    if (cause === "source") retireDetailOwnership(qc);
    qc.setQueryData(key, { ...detail, body: "Newer cached content", ...(cause === "head" ? { head_oid: "h2" } : {}) });
  });
  await act(async () => finish({ ...detail, body: "Stale readback", ...(cause === "identity" ? { number: 999 } : {}) }));
  expect(qc.getQueryData<PrDetail>(key)?.body).toBe("Newer cached content");
  const lines = boundary.invoke.mock.calls.filter(([name]) => name === "diag_log").map(([, args]) => args.line as string);
  expect(lines.some(line => line.includes(`outcome=discarded cause=${cause}`))).toBe(true);
  expect(lines.join("\n")).not.toMatch(/synthetic|Newer cached|Stale readback|h1|h2|new-viewer/);
});
