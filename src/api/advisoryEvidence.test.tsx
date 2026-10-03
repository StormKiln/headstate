import { act, cleanup, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "@/fixtures/prs";
import { prKey } from "@/lib/prIdentity";
import { partitionReady } from "@/lib/readyPusher";
import type { PullRequest } from "@/types/pr";
const invoke = vi.hoisted(() => vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { useReadyStacks } from "./useReadyStacks";
import { useReadyPushers } from "./useReadyPushers";
let qc: QueryClient;
const stacked = { kind: "stacked", native: true, stack_number: 7, position: 2, size: 3, position_exact: true, size_exact: true, below: 1 };
const rows = Array.from({ length: 275 }, (_, i) => ({ ...PR_FIXTURES[0], repo: `synthetic/repo-${i % 44}`, number: i + 1, head_repo: `synthetic/repo-${i % 44}`, head_ref: `feature-${i}`, head_oid: `sha-${i}` }));
let failure = false;
let lifetime = 60_000;
function wrapper({ children }: { children: ReactNode }) { return <QueryClientProvider client={qc}>{children}</QueryClientProvider>; }
async function advance(ms: number) { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); }
beforeEach(() => {
  vi.useFakeTimers(); failure = false; lifetime = 60_000;
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(["viewer"], "octocat");
  invoke.mockReset();
  invoke.mockImplementation(async (cmd, args) => {
    if (cmd === "get_viewer") return "octocat";
    return (args?.rows as PullRequest[]).map(pr => cmd === "get_ready_stacks"
      ? { ...pr, valid_for_ms: failure ? undefined : lifetime, stack: failure ? { kind: "unknown" } : stacked }
      : { ...pr, rules_valid_for_ms: lifetime, pusher_valid_for_ms: lifetime, rules: failure ? { state: "declined", reason: "budget" } : { state: "read", require_last_push_approval: true, required_review_thread_resolution: false }, last_pusher: failure ? { state: "declined", reason: "budget" } : { state: "known", login: "octocat" } });
  });
});
afterEach(() => { cleanup(); qc.clear(); vi.useRealTimers(); });

// Removing query-owned success retention loses the chip on the first deferred read.
it("retains stack display across expiry and refused refresh without authorizing an action", async () => {
  const view = renderHook(() => useReadyStacks([rows[0]]), { wrapper });
  await advance(5);
  expect(view.result.current.of(rows[0])).toEqual(stacked);
  const observed = view.result.current.displayOf?.(rows[0])?.observedAt;
  failure = true;
  for (let cycle = 0; cycle < 3; cycle++) { await advance(30_000); await advance(5); }
  expect(view.result.current.displayOf?.(rows[0])).toMatchObject({ value: stacked, freshness: "retained", observedAt: observed });
  expect(view.result.current.of(rows[0])?.kind).not.toBe("stacked");
});

it("retains a 275-row display beyond the old five-minute GC over three rotations", async () => {
  const view = renderHook(({ population }) => useReadyStacks(population), { wrapper, initialProps: { population: rows } });
  await advance(5);
  const first = (invoke.mock.calls[0][1]?.rows as PullRequest[])[0];
  view.rerender({ population: rows.filter(pr => pr.number !== first.number) });
  for (let cycle = 0; cycle < 105; cycle++) { await advance(30_000); await advance(5); }
  expect(view.result.current.displayOf?.(first)).toMatchObject({ value: stacked, freshness: "retained" });
  expect(view.result.current.of(first)).toBeUndefined();
});

it("expires native-aged pusher authority and retains a neutral chip through a declined read", async () => {
  lifetime = 2_000;
  const view = renderHook(() => useReadyPushers([rows[0]]), { wrapper });
  await advance(5);
  expect(partitionReady([rows[0]], view.result.current.of, "auto").hidden).toBe(1);
  failure = true;
  await advance(30_005);
  expect(partitionReady([rows[0]], view.result.current.of, "auto").hidden).toBe(0);
  expect(view.result.current.displayOf?.(rows[0])).toMatchObject({ value: { pusher: { state: "viewer" } }, freshness: "retained" });
});

it("gives newly visible work a bounded turn without losing the fair offscreen lane", async () => {
  const view = renderHook(({ priority }) => useReadyStacks(rows, priority), { wrapper, initialProps: { priority: new Set<string>() } });
  await advance(5);
  const first = (invoke.mock.calls[0][1]?.rows as PullRequest[])[0];
  failure = true;
  view.rerender({ priority: new Set([prKey(first)]) });
  for (let cycle = 0; cycle < 3; cycle++) { await advance(30_000); await advance(5); }
  const asked = invoke.mock.calls.slice(1).flatMap(call => call[1]?.rows as PullRequest[]);
  expect(asked.some(pr => pr.number === first.number)).toBe(true);
  expect(new Set(asked.map(pr => pr.number)).size).toBeGreaterThan(8);
});

it("does not start another batch for unrelated population churn between finite windows", async () => {
  const view = renderHook(({ population }) => useReadyStacks(population), { wrapper, initialProps: { population: rows } });
  await advance(5);
  for (let i = 0; i < 20; i++) {
    view.rerender({ population: rows.slice(i % 2) });
    await advance(5);
  }
  expect(invoke.mock.calls.filter(call => call[0] === "get_ready_stacks")).toHaveLength(8);
});

it("stops revisiting known offscreen identities while revalidating unchanged visible rows", async () => {
  const view = renderHook(() => useReadyStacks(rows, new Set([prKey(rows[0])])), { wrapper });
  await advance(5);
  for (let cycle = 0; cycle < 75; cycle++) { await advance(30_000); await advance(5); }
  const asked = invoke.mock.calls.flatMap(call => call[1]?.rows as PullRequest[]);
  expect(asked.filter(pr => pr.number === rows[10].number)).toHaveLength(1);
  expect(asked.filter(pr => pr.number === rows[0].number).length).toBeGreaterThan(10);
  view.unmount();
});

it("clears same-viewer reset and login round trips including delayed completions", async () => {
  const view = renderHook(() => ({ stack: useReadyStacks([rows[0]]), pusher: useReadyPushers([rows[0]]) }), { wrapper });
  await advance(5);
  expect(view.result.current.stack.displayOf(rows[0])).toBeDefined();
  const pending: { cmd: string; args?: Record<string, unknown>; finish: (v: unknown) => void }[] = [];
  invoke.mockImplementation((cmd, args) => cmd === "get_viewer" ? Promise.resolve("octocat") : new Promise(finish => { pending.push({ cmd, args, finish }); }));
  await act(async () => { void qc.resetQueries(); });
  await advance(5);
  expect(view.result.current.stack.displayOf(rows[0])).toBeUndefined();
  expect(view.result.current.pusher.displayOf(rows[0])).toBeUndefined();
  const old = [...pending];
  await act(async () => { qc.setQueryData(["viewer"], "other"); });
  await advance(5);
  await act(async () => { qc.setQueryData(["viewer"], "octocat"); });
  await advance(5);
  await act(async () => { for (const call of old) call.finish((call.args?.rows as PullRequest[]).map(pr => ({ ...pr, valid_for_ms: 60_000, stack: stacked }))); });
  await advance(5);
  expect(view.result.current.stack.of(rows[0])).toBeUndefined();
  expect(view.result.current.stack.displayOf(rows[0])).toBeUndefined();
});

it("bounds inactive identity churn and rejects changed head/base/ref/repo receipts", async () => {
  const view = renderHook(({ pr }) => ({ stack: useReadyStacks([pr]), pusher: useReadyPushers([pr]) }), { wrapper, initialProps: { pr: rows[0] } });
  await advance(5);
  for (let i = 0; i < 530; i++) {
    const pr = { ...rows[0], head_oid: `changed-${i}`, base_ref: `base-${i}`, head_ref: `ref-${i}`, head_repo: `synthetic/fork-${i}` };
    view.rerender({ pr });
    expect(view.result.current.stack.displayOf(pr)).toBeUndefined();
    expect(view.result.current.pusher.displayOf(pr)).toBeUndefined();
    await advance(5);
  }
  expect(qc.getQueryCache().findAll({ queryKey: ["ready-stack"] }).length).toBeLessThanOrEqual(512);
  expect(qc.getQueryCache().findAll({ queryKey: ["ready-pushers"] }).length).toBeLessThanOrEqual(512);
  expect(view.result.current.stack.displayOf(rows[0])).toBeUndefined();
});

it("replaces retained membership and mutable pusher/policy with a contradictory success on the same head", async () => {
  const view = renderHook(() => ({ stack: useReadyStacks([rows[0]]), pusher: useReadyPushers([rows[0]]) }), { wrapper });
  await advance(5);
  failure = true;
  for (let cycle = 0; cycle < 3; cycle++) { await advance(30_000); await advance(5); }
  expect(view.result.current.stack.displayOf(rows[0])?.freshness).toBe("retained");
  invoke.mockImplementation(async (cmd, args) => (args?.rows as PullRequest[]).map(pr => cmd === "get_ready_stacks"
    ? { ...pr, valid_for_ms: 60_000, stack: { kind: "none" } }
    : { ...pr, pusher_valid_for_ms: 60_000, rules_valid_for_ms: 600_000, last_pusher: { state: "known", login: "someone-else" }, rules: { state: "read", require_last_push_approval: false, required_review_thread_resolution: false } }));
  await advance(30_000); await advance(5);
  expect(view.result.current.stack.of(rows[0])).toEqual({ kind: "none" });
  expect(view.result.current.stack.displayOf(rows[0])).toMatchObject({ value: { kind: "none" }, freshness: "fresh" });
  expect(view.result.current.pusher.of(rows[0])).toEqual({ pusher: { state: "other", login: "someone-else" }, rule: "not-required" });
});

it("keeps legacy missing-lifetime and expired-on-arrival answers out of action authority", async () => {
  lifetime = 0;
  const view = renderHook(() => ({ stack: useReadyStacks([rows[0]]), pusher: useReadyPushers([rows[0]]) }), { wrapper });
  await advance(5);
  expect(view.result.current.stack.of(rows[0])?.kind).not.toBe("stacked");
  expect(partitionReady([rows[0]], view.result.current.pusher.of, "auto").hidden).toBe(0);
});

it("coalesces a late selected detail with strip demand and promotes it at the shared next window", async () => {
  let current = 0;
  const order: { at: number; cmd: string; number: number }[] = [];
  invoke.mockImplementation(async (cmd, args) => {
    const pr = (args?.rows as PullRequest[])[0];
    order.push({ at: current, cmd, number: pr.number });
    return cmd === "get_ready_stacks" ? [{ ...pr, stack: { kind: "unknown" } }] : [{ ...pr, rules: { state: "declined", reason: "budget" }, last_pusher: { state: "declined", reason: "budget" } }];
  });
  renderHook(() => ({ stack: useReadyStacks(rows), pusher: useReadyPushers(rows) }), { wrapper });
  await advance(5);
  current = 15_000;
  await advance(15_000);
  const selected = renderHook(() => useReadyStacks([rows[274]], undefined, true, "detail"), { wrapper });
  await advance(5);
  current = 30_000;
  await advance(15_000); await advance(5);
  expect(order.find(call => call.at === 30_000)).toMatchObject({ cmd: "get_ready_stacks", number: rows[274].number });
  selected.unmount();
});

it("puts the fair offscreen lane ahead of an unreadable visible prefix on alternating windows", async () => {
  failure = true;
  const visible = new Set(rows.slice(0, 8).map(prKey));
  renderHook(() => ({ stack: useReadyStacks(rows, visible), pusher: useReadyPushers(rows, visible) }), { wrapper });
  await advance(5);
  invoke.mockClear();
  await advance(30_000); await advance(5);
  const firstStack = invoke.mock.calls.find(call => call[0] === "get_ready_stacks")!;
  const first = (firstStack[1]?.rows as PullRequest[])[0];
  expect(visible.has(prKey(first))).toBe(false);
});

it("prioritizes selected detail after a live slow request without replaying canceled old-session work", async () => {
  let finish: ((value: unknown) => void) | undefined;
  invoke.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const strip = renderHook(() => ({ stack: useReadyStacks(rows), pusher: useReadyPushers(rows) }), { wrapper });
  await advance(5);
  expect(invoke).toHaveBeenCalledTimes(1);
  const detail = renderHook(() => useReadyStacks([rows[274]], undefined, true, "detail"), { wrapper });
  await advance(5);
  expect(invoke).toHaveBeenCalledTimes(1);
  strip.unmount();
  await act(async () => { finish!([]); });
  await advance(5);
  expect(invoke.mock.calls[1][0]).toBe("get_ready_stacks");
  expect((invoke.mock.calls[1][1]?.rows as PullRequest[])[0].number).toBe(275);
  expect(invoke).toHaveBeenCalledTimes(2);
  expect(detail.result.current.of(rows[274])).toEqual(stacked);
});

it("stops all new hidden advisory demand and resumes one bounded shared window", async () => {
  renderHook(() => ({ stack: useReadyStacks(rows), pusher: useReadyPushers(rows) }), { wrapper });
  await advance(5);
  const before = invoke.mock.calls.length;
  Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
  await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
  for (let cycle = 0; cycle < 4; cycle++) { await advance(30_000); await advance(5); }
  expect(invoke.mock.calls).toHaveLength(before);
  Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
  await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
  await advance(5);
  expect(invoke.mock.calls.length - before).toBeLessThanOrEqual(16);
});

it("enforces the hard cap even with more live consumers than cache capacity", async () => {
  const views = Array.from({ length: 65 }, (_, group) => {
    const population = Array.from({ length: 8 }, (_, i) => ({ ...rows[0], number: group * 8 + i + 1 }));
    return renderHook(() => useReadyStacks(population), { wrapper });
  });
  await advance(5);
  expect(qc.getQueryCache().findAll({ queryKey: ["ready-stack"] })).toHaveLength(512);
  expect(views[64].result.current.of({ ...rows[0], number: 520 })).toEqual(stacked);
  for (const view of views.slice(0, -1)) view.unmount();
  await advance(3 * 60 * 60_000);
  expect(qc.getQueryCache().findAll({ queryKey: ["ready-stack"] })).toHaveLength(512);
});

it("alternates the first strip class across complete drains even when both windows contain eight rows", async () => {
  failure = true;
  renderHook(() => ({ pusher: useReadyPushers(rows), stack: useReadyStacks(rows) }), { wrapper });
  await advance(5);
  const first = invoke.mock.calls[0][0];
  invoke.mockClear();
  await advance(30_000); await advance(5);
  expect(invoke.mock.calls[0][0]).not.toBe(first);
});

it("does not certify a legacy reply with the unexpired lifetime of a previous receipt", async () => {
  const view = renderHook(() => useReadyPushers([rows[0]]), { wrapper });
  await advance(5);
  expect(partitionReady([rows[0]], view.result.current.of, "auto").hidden).toBe(1);
  invoke.mockImplementation(async (_cmd, args) => (args?.rows as PullRequest[]).map(pr => ({ ...pr,
    rules: { state: "read", require_last_push_approval: true, required_review_thread_resolution: false },
    last_pusher: { state: "known", login: "octocat" } })));
  await act(async () => { await qc.invalidateQueries({ queryKey: ["ready-pushers"] }); });
  await advance(5);
  expect(partitionReady([rows[0]], view.result.current.of, "auto").hidden).toBe(0);
  expect(view.result.current.displayOf(rows[0])?.freshness).toBe("retained");
});
