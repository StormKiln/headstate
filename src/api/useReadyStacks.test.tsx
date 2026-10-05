import { act, cleanup, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "@/fixtures/prs";
import type { PrStack } from "@/types/pr";

const invoke = vi.hoisted(() => vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { useReadyStacks } from "./useReadyStacks";
import * as dispatch from "./advisoryDispatch";
import { prKey } from "@/lib/prIdentity";

let qc: QueryClient;
beforeEach(() => {
  vi.useFakeTimers();
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  invoke.mockReset();
  qc.setQueryData(["viewer"], "octocat");
});
afterEach(() => {
  cleanup();
  qc.clear();
  vi.useRealTimers();
});

const rows = [PR_FIXTURES[0], { ...PR_FIXTURES[0], id: "other", number: PR_FIXTURES[0].number + 1 }];
const exact: PrStack = {
  kind: "stacked", native: true, stack_number: 7, position: 5, size: 8,
  position_exact: true, size_exact: true, below: 4,
};
function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}
async function advance(ms: number) {
  await act(async () => { await vi.advanceTimersByTimeAsync(ms); });
}

it("refreshes mounted unknown and off-list membership without head changes, once per minute without duplicate row demand", async () => {
  let stack: PrStack = { kind: "unknown" };
  invoke.mockImplementation(async () => rows.map((pr) => ({ repo: pr.repo, number: pr.number, head_oid: pr.head_oid, base_ref: pr.base_ref, valid_for_ms: stack.kind === "unknown" ? 5_000 : 60_000, stack })));
  const view = renderHook(() => useReadyStacks(rows), { wrapper });
  await advance(1);
  expect(view.result.current.of(rows[0])).toEqual({ kind: "unknown" });
  expect(invoke).toHaveBeenCalledTimes(2);
  stack = exact;
  await advance(29_998);
  expect(invoke).toHaveBeenCalledTimes(2);
  await advance(2);
  await advance(2);
  expect(view.result.current.of(rows[0])).toEqual(exact);
  expect(invoke).toHaveBeenCalledTimes(4);
  expect(invoke.mock.calls[2][1]?.rows).toHaveLength(1);
  stack = { ...exact, position: 4, size: 7 };
  await advance(60_000);
  await advance(2);
  expect(view.result.current.of(rows[0])).toEqual(stack);
  expect(invoke).toHaveBeenCalledTimes(6);
  view.unmount();
  await advance(120_000);
  expect(invoke).toHaveBeenCalledTimes(6);
});

it("does not multiply pending metadata requests when refresh timers fire", async () => {
  let finish: ((value: unknown) => void) | undefined;
  invoke.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  renderHook(() => useReadyStacks(rows), { wrapper });
  await advance(1);
  expect(invoke).toHaveBeenCalledTimes(1);
  await advance(180_000);
  expect(invoke).toHaveBeenCalledTimes(1);
  await act(async () => { finish!([]); });
});

it("limits a large mounted owner to one finite demand window and stops hidden work", async () => {
  const many = Array.from({ length: 120 }, (_, i) => ({ ...rows[0], id: `task4-${i}`, number: i + 1, base_ref: `base-${i % 44}` }));
  invoke.mockImplementation(async (_command, args) => (args?.rows as typeof many).map((pr) => ({ ...pr, valid_for_ms: 60_000, stack: { kind: "unknown" } })));
  const view = renderHook(() => useReadyStacks(many), { wrapper });
  await advance(10);
  expect(invoke).toHaveBeenCalledTimes(8);
  expect(invoke.mock.calls[0][1]?.rows).toHaveLength(1);
  view.unmount();
  await advance(120_000);
  expect(invoke).toHaveBeenCalledTimes(8);
});

it("rejects legacy and mismatched head/base evidence and isolates account receipts", async () => {
  let owner = "octocat";
  invoke.mockImplementation(async (_command, args) => (args?.rows as typeof rows).map(pr => ({ ...pr, valid_for_ms: 60_000, head_oid: owner === "octocat" ? "wrong" : pr.head_oid, stack: exact })));
  const view = renderHook(() => useReadyStacks(rows), { wrapper });
  await advance(2);
  expect(view.result.current.of(rows[0])).toEqual({ kind: "unknown" });
  owner = "other-viewer";
  await act(async () => { qc.setQueryData(["viewer"], owner); });
  await advance(3);
  expect(view.result.current.of(rows[0])).toEqual(exact);
  expect(invoke).toHaveBeenCalledTimes(4);
});

it("shares two owners and removes hidden queued demand without canceling a live observer", async () => {
  let finish: ((v: unknown) => void) | undefined;
  invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const a = renderHook(() => useReadyStacks(rows), { wrapper });
  const b = renderHook(() => useReadyStacks(rows), { wrapper });
  await advance(1);
  a.unmount();
  await act(async () => finish!(rows.map(pr => ({ ...pr, valid_for_ms: 60_000, stack: exact }))));
  await advance(2);
  expect(b.result.current.of(rows[0])).toEqual(exact);
  expect(invoke).toHaveBeenCalledTimes(2);
  Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
  await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
  await advance(120_000);
  expect(invoke).toHaveBeenCalledTimes(2);
  Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
  b.unmount();
});

it("drops a closed owner's waiting batch while another owner's native call finishes", async () => {
  let finish: ((value: unknown) => void) | undefined;
  invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const live = renderHook(() => useReadyStacks(rows), { wrapper });
  await advance(1);
  const otherRows = rows.map(pr => ({ ...pr, number: pr.number + 100 }));
  const closed = renderHook(() => useReadyStacks(otherRows), { wrapper });
  await advance(1);
  closed.unmount();
  await act(async () => { finish!(rows.map(pr => ({ ...pr, valid_for_ms: 60_000, stack: exact }))); });
  await advance(2);
  expect(invoke).toHaveBeenCalledTimes(2);
  expect(live.result.current.of(rows[0])).toEqual(exact);
});

it("resumes a fresh qualified partial on the next window without renewing its original authority", async () => {
  const pr = rows[0];
  let calls = 0;
  invoke.mockImplementation(async () => {
    calls++;
    return [{ repo: pr.repo, number: pr.number, head_oid: pr.head_oid, base_ref: pr.base_ref,
      stack: calls <= 2 ? exact : { kind: "unknown" },
      valid_for_ms: calls === 1 ? 60_000 : calls === 2 ? 30_000 : undefined,
      advisory_progress: { outcome: calls === 1 ? "partial" : calls === 2 ? "offered" : "deferred", admitted: calls <= 2 } }];
  });
  const view = renderHook(() => useReadyStacks([pr]), { wrapper });
  await advance(2);
  const original = view.result.current.displayOf(pr)?.observedAt;
  expect(view.result.current.of(pr)).toEqual(exact);
  await advance(29_990);
  expect(calls).toBe(1);
  await advance(12);
  expect(calls).toBe(2);
  expect(view.result.current.displayOf(pr)?.observedAt).toBe(original);
  await advance(30_002);
  expect(view.result.current.of(pr)).toEqual({ kind: "unknown" });
  expect(view.result.current.displayOf(pr)).toMatchObject({ observedAt: original, freshness: "retained" });
});


it("does not retain detail priority after the coalesced detail observer unmounts", async () => {
  const spy = vi.spyOn(dispatch, "advisoryDispatch");
  invoke.mockImplementation(async (_command, args) => (args?.rows as typeof rows).map(pr => ({ ...pr, valid_for_ms: 60_000, stack: exact, advisory_progress: { outcome: "offered", admitted: true } })));
  const strip = renderHook(() => useReadyStacks(rows.slice(0, 1)), { wrapper });
  await advance(2);
  const detail = renderHook(() => useReadyStacks(rows.slice(0, 1), new Set(), true, "detail"), { wrapper });
  await advance(2);
  detail.unmount();
  spy.mockClear();
  await advance(60_002);
  expect(spy.mock.calls.length).toBeGreaterThan(0);
  expect(spy.mock.calls.every(call => call[2] === "stack")).toBe(true);
  strip.unmount();
  spy.mockRestore();
});


it("bounds recurring partial priority while healthy tail identities make useful progress", async () => {
  const population = Array.from({ length: 12 }, (_, i) => ({ ...rows[0], number: 100 + i }));
  const seen = new Set<number>();
  invoke.mockImplementation(async (_command, args) => (args?.rows as typeof rows).map(pr => {
    seen.add(pr.number);
    const poison = pr.number === 106 || pr.number === 107;
    return { ...pr, valid_for_ms: poison ? 5_000 : 60_000, stack: poison ? { kind: "unknown" } : exact,
      advisory_progress: { outcome: poison ? "partial" : "offered", admitted: true } };
  }));
  const view = renderHook(() => useReadyStacks(population, new Set(population.slice(0, 6).map(prKey))), { wrapper });
  await advance(5);
  for (let cycle = 0; cycle < 6; cycle++) { await advance(30_000); await advance(5); }
  for (const pr of population.slice(8)) {
    expect(seen.has(pr.number)).toBe(true);
    expect(view.result.current.displayOf(pr)).toBeDefined();
  }
  for (const query of qc.getQueryCache().getAll().filter(q => q.queryKey[0] === "ready-stack")) {
    const state = query.meta?.advisorySchedule as { hasContinuation?: boolean; resumeBoostSpent?: boolean };
    if (state?.hasContinuation) expect(state.resumeBoostSpent).toBe(true);
  }
});

it.each([8, 9])("retains an8ms conclusive receipt after %ims transport without granting freshness, then refreshes on the ordinary window", async (latency) => {
  const pr = rows[0];
  let calls = 0;
  const publications: { at: number; expiresAt: number }[] = [];
  const unsubscribe = qc.getQueryCache().subscribe(event => {
    if (event.type === "updated" && event.action.type === "success" && event.query.queryKey[0] === "ready-stack") {
      const data = event.query.state.data as { expiresAt: number };
      publications.push({ at: performance.now(), expiresAt: data.expiresAt });
    }
  });
  invoke.mockImplementation(async () => {
    calls++;
    if (calls === 1) await new Promise(resolve => setTimeout(resolve, latency));
    return [{ ...pr, stack: { kind: "none" }, valid_for_ms: calls === 1 ? 8 : 60_000,
      last_known_stack: { value: { kind: "none" }, age_ms: calls === 1 ? 59_991 : 0 },
      advisory_progress: { outcome: "offered", admitted: true } }];
  });
  const view = renderHook(() => useReadyStacks([pr], new Set([prKey(pr)])), { wrapper });
  await advance(latency + 2);
  expect(calls).toBe(1);
  expect(publications).toHaveLength(1);
  expect(publications[0].expiresAt).toBeLessThanOrEqual(publications[0].at);
  expect(view.result.current.of(pr)).toBeUndefined();
  expect(view.result.current.displayOf(pr)).toMatchObject({ value: { kind: "none" }, freshness: "retained" });
  await advance(29_000);
  expect(calls).toBe(1);
  await advance(1_000);
  expect(calls).toBe(2);
  expect(publications).toHaveLength(2);
  expect(view.result.current.of(pr)).toEqual({ kind: "none" });
  expect(view.result.current.displayOf(pr)?.freshness).toBe("fresh");
  unsubscribe();
});
