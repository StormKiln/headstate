import { act, cleanup, render, renderHook, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "@/fixtures/prs";
import { partitionReady, partitionSummary } from "@/lib/readyPusher";
import type { PusherAsk } from "@/types/pr";
const invoke = vi.hoisted(() => vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>());
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => cmd === "remote_call"
    ? invoke(args?.command as string, args?.args as Record<string, unknown>)
    : invoke(cmd, args),
}));
import { useReadyPushers } from "./useReadyPushers";
import { advisoryDispatch } from "./advisoryDispatch";
let qc: QueryClient;
const pr = { ...PR_FIXTURES[0], repo: "synthetic/repo", head_repo: "synthetic/repo", head_oid: "head", head_ref: "feature" };
function wrapper({ children }: { children: ReactNode }) { return <QueryClientProvider client={qc}>{children}</QueryClientProvider>; }
async function advance(ms = 5) { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); }
function success(ask: PusherAsk) { return { ...ask, pusher_valid_for_ms: 60_000, rules_valid_for_ms: 600_000, last_pusher: { state: "known", login: "octocat" }, rules: { state: "read", require_last_push_approval: true, required_review_thread_resolution: false } }; }
beforeEach(() => {
  vi.useFakeTimers();
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(["viewer"], "octocat");
  invoke.mockReset();
  invoke.mockImplementation(async (cmd, args) => cmd === "get_viewer" ? "octocat" : (args?.rows as PusherAsk[]).map(success));
});
afterEach(() => { cleanup(); qc.clear(); vi.useRealTimers(); });

// The production hook must not convert a settled failed read back to absence.
it.each(["rejection", "missing", "mismatch"])("settles %s as unreadable and renders an undecided summary", async failure => {
  let finish!: () => void;
  invoke.mockImplementationOnce((_cmd, args) => new Promise((resolve, reject) => {
    finish = () => failure === "rejection" ? reject(new Error("connection lost")) : resolve(failure === "missing" ? [] : [{ ...success((args?.rows as PusherAsk[])[0]), head_oid: "other-head" }]);
  }));
  function Summary() {
    const result = useReadyPushers([pr]);
    return <output>{partitionSummary(partitionReady([pr], result.of, "auto"), "auto")}</output>;
  }
  const view = renderHook(() => useReadyPushers([pr]), { wrapper });
  render(<Summary />, { wrapper });
  await advance();
  expect(view.result.current.isPending).toBe(true);
  expect(screen.getByText("1 not checked yet")).toBeTruthy();
  await act(async () => { finish(); });
  await advance();
  expect(view.result.current.isPending).toBe(false);
  expect(view.result.current.of(pr)).toEqual({ pusher: { state: "unknown" }, rule: "unread" });
  expect(screen.getByText("1 could not be decided")).toBeTruthy();
  expect(view.result.current.displayOf(pr)).toBeUndefined();
});

it("retains original display age after rejected refresh without filter authority, then recovers", async () => {
  const view = renderHook(() => useReadyPushers([pr]), { wrapper });
  await advance();
  const observedAt = view.result.current.displayOf(pr)?.observedAt;
  expect(partitionReady([pr], view.result.current.of, "auto").hidden).toBe(1);
  invoke.mockRejectedValueOnce(new Error("connection lost"));
  await act(async () => { await qc.invalidateQueries({ queryKey: ["ready-pushers"] }); });
  await advance();
  expect(view.result.current.of(pr)).toEqual({ pusher: { state: "unknown" }, rule: "unread" });
  expect(view.result.current.displayOf(pr)).toMatchObject({ observedAt, freshness: "retained", value: { pusher: { state: "viewer" } } });
  expect(partitionReady([pr], view.result.current.of, "auto")).toMatchObject({ hidden: 0, unknown: 1, notChecked: 0 });
  const calls = invoke.mock.calls.length;
  await advance(4_000);
  expect(invoke.mock.calls).toHaveLength(calls);
  await advance(26_000);
  await advance();
  expect(view.result.current.displayOf(pr)?.freshness).toBe("fresh");
  expect(partitionReady([pr], view.result.current.of, "auto").hidden).toBe(1);
});

it.each(["declined", "not-asked"])("keeps explicit %s as not checked", async kind => {
  invoke.mockImplementationOnce(async (_cmd, args) => {
    if (kind === "not-asked") throw "headstate:not-asked Sign in first";
    return (args?.rows as PusherAsk[]).map(ask => ({ ...ask, last_pusher: { state: "declined", reason: "budget" }, rules: { state: "declined", reason: "budget" } }));
  });
  const view = renderHook(() => useReadyPushers([pr]), { wrapper });
  await advance();
  expect(view.result.current.isPending).toBe(false);
  expect(partitionReady([pr], view.result.current.of, "auto")).toMatchObject({ hidden: 0, notChecked: 1, unknown: 0 });
});

it("coalesces observers and reuses fresh cached evidence without another command", async () => {
  const first = renderHook(() => useReadyPushers([pr]), { wrapper });
  const second = renderHook(() => useReadyPushers([pr]), { wrapper });
  await advance();
  expect(first.result.current.displayOf(pr)?.freshness).toBe("fresh");
  expect(second.result.current.displayOf(pr)?.freshness).toBe("fresh");
  first.unmount(); second.unmount();
  const next = renderHook(() => useReadyPushers([pr]), { wrapper });
  await advance();
  expect(next.result.current.of(pr).pusher.state).toBe("viewer");
  expect(invoke.mock.calls.filter(([cmd]) => cmd === "get_ready_pushers")).toHaveLength(1);
});

it.each(["head_oid", "base_ref"] as const)("cannot carry retained success onto a changed %s", async field => {
  const view = renderHook(({ row }) => useReadyPushers([row]), { wrapper, initialProps: { row: pr } });
  await advance();
  invoke.mockRejectedValue(new Error("connection lost"));
  const changed = { ...pr, [field]: "changed" };
  view.rerender({ row: changed });
  await advance(30_000); await advance();
  expect(view.result.current.displayOf(changed)).toBeUndefined();
  expect(view.result.current.of(changed).pusher.state).toBe("unknown");
});

it("discards a rejected in-flight read when its session retires", async () => {
  let reject!: (error: Error) => void;
  invoke.mockImplementationOnce(() => new Promise((_resolve, fail) => { reject = fail; }));
  const view = renderHook(() => useReadyPushers([pr]), { wrapper });
  await advance();
  await act(async () => { qc.setQueryData(["viewer"], "new-viewer"); });
  invoke.mockImplementation(() => new Promise(() => {}));
  await act(async () => { reject(new Error("old connection failed")); });
  await advance();
  expect(view.result.current.of(pr).pusher.state).toBe("pending");
  expect(view.result.current.displayOf(pr)).toBeUndefined();
  expect(qc.getQueryCache().findAll({ queryKey: ["ready-pushers", "octocat"] })).toHaveLength(0);
});

it("cancels queued work without publishing unknown or dispatching it", async () => {
  let finish!: (value: unknown) => void;
  invoke.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const first = renderHook(() => useReadyPushers([pr]), { wrapper });
  const queuedPr = { ...pr, number: pr.number + 1 };
  const queued = renderHook(() => useReadyPushers([queuedPr]), { wrapper });
  await advance();
  expect(queued.result.current.of(queuedPr).pusher.state).toBe("pending");
  queued.unmount();
  await act(async () => { finish([]); });
  await advance();
  expect(invoke.mock.calls.filter(([cmd]) => cmd === "get_ready_pushers")).toHaveLength(1);
  expect(qc.getQueryCache().findAll({ queryKey: ["ready-pushers"] }).filter(query => query.state.data !== undefined)).toHaveLength(1);
  first.unmount();
});


it("keeps queue-capacity refusal not checked without dispatching a command", async () => {
  const controller = new AbortController();
  let finish!: () => void;
  const active = advisoryDispatch(qc, "active", "detail", controller.signal, () => new Promise<void>(resolve => { finish = resolve; }));
  await advance();
  const waiting = Array.from({ length: 64 }, (_, i) => advisoryDispatch(qc, `waiting-${i}`, "detail", controller.signal, async () => {}).catch(() => {}));
  const view = renderHook(() => useReadyPushers([pr]), { wrapper });
  await advance();
  expect(view.result.current.isPending).toBe(false);
  expect(partitionReady([pr], view.result.current.of, "auto")).toMatchObject({ hidden: 0, notChecked: 1, unknown: 0 });
  expect(invoke).not.toHaveBeenCalled();
  await act(async () => { controller.abort(); finish(); await Promise.all([active, ...waiting]); });
});

it("keeps offscreen initial pusher and rules demand until usable publication instead of retained presence", async () => {
  const population = Array.from({ length: 12 }, (_, i) => ({ ...pr, number: 100 + i }));
  const target = population[6];
  let calls = 0;
  const useful: boolean[] = [];
  const unsubscribe = qc.getQueryCache().subscribe(event => {
    if (event.type !== "updated" || event.action.type !== "success" || event.query.queryKey[0] !== "ready-pushers") return;
    if (JSON.parse(JSON.parse(String(event.query.queryKey[3]))[0])[3] !== target.number) return;
    const value = event.query.state.data as { row: { last_pusher: { state: string }; rules: { state: string } }; pusher?: { expiresAt: number }; rules?: { expiresAt: number } };
    useful.push(value.row.last_pusher.state === "known" && value.row.rules.state === "read" && (value.pusher?.expiresAt ?? 0) > performance.now() && (value.rules?.expiresAt ?? 0) > performance.now());
  });
  invoke.mockImplementation(async (_command, args) => {
    const ask = (args?.rows as PusherAsk[])[0];
    const count = ask.number === target.number ? ++calls : 4;
    if (count === 1) await new Promise(resolve => setTimeout(resolve, 7));
    const answer = { ...success(ask), rules: { state: "read", require_last_push_approval: false, required_review_thread_resolution: false } };
    return [{ ...answer,
      ...(count === 2 || count === 3 ? { last_pusher: { state: "declined", reason: "budget" }, rules: { state: "declined", reason: "budget" } } : {}),
      pusher_valid_for_ms: count === 1 ? 1 : 60_000, rules_valid_for_ms: count === 1 ? 1 : 600_000,
      last_known_pusher: { value: answer.last_pusher, age_ms: 60_000 }, last_known_rules: { value: answer.rules, age_ms: 600_000 },
      advisory_progress: { outcome: count === 2 ? "partial" : count === 3 ? "deferred" : "offered", admitted: count !== 3 } }];
  });
  const view = renderHook(() => useReadyPushers(population, new Set(population.slice(0, 6).map(row => JSON.stringify(["github", "github.com", row.repo, row.number])))), { wrapper });
  await advance(20);
  expect(useful).toEqual([false]);
  expect(view.result.current.displayOf(target)?.freshness).toBe("retained");
  for (let cycle = 0; cycle < 6; cycle++) { await advance(30_000); await advance(10); }
  expect(useful).toContain(true);
  expect(calls).toBe(4);
  await advance(120_000);
  expect(calls).toBe(4);
  unsubscribe();
});

it("retires served cached-partial offscreen rows so never-served families get their turns", async () => {
  const population = Array.from({ length: 12 }, (_, i) => ({ ...pr, number: 100 + i }));
  const identity = (row: typeof pr) => JSON.stringify(["github", "github.com", row.repo, row.number]);
  const calls = new Map<number, number>();
  const usable = new Set<number>();
  const unsubscribe = qc.getQueryCache().subscribe(event => {
    if (event.type !== "updated" || event.action.type !== "success" || event.query.queryKey[0] !== "ready-pushers") return;
    const value = event.query.state.data as { row: { number: number; last_pusher: { state: string } }; pusher?: { expiresAt: number } };
    if (value.row.last_pusher.state === "known" && (value.pusher?.expiresAt ?? 0) > performance.now()) usable.add(value.row.number);
  });
  invoke.mockImplementation(async (_command, args) => (args?.rows as PusherAsk[]).map(ask => {
    const count = (calls.get(ask.number) ?? 0) + 1;
    calls.set(ask.number, count);
    const served = ask.number === 106 || ask.number === 107;
    const partial = served ? count > 1 : count === 1;
    return { ...success(ask),
      pusher_valid_for_ms: served ? 1_000 : 60_000,
      ...(partial ? { last_pusher: { state: "declined", reason: "advisory consumer share is spent for this cycle" } } : {}),
      advisory_progress: { outcome: partial ? "partial" : "offered", admitted: !served || count === 1 } };
  }));
  const initialPriority = new Set([...population.slice(0, 4), ...population.slice(6, 8)].map(identity));
  const view = renderHook(({ priority }) => useReadyPushers(population, priority), { wrapper, initialProps: { priority: initialPriority } });
  await advance();
  expect(usable.has(106) && usable.has(107)).toBe(true);
  await advance(30_000); await advance();
  expect(calls.get(106)).toBe(2);
  expect(calls.get(107)).toBe(2);
  view.rerender({ priority: new Set(population.slice(0, 6).map(identity)) });
  for (let cycle = 0; cycle < 12; cycle++) { await advance(30_000); await advance(); }
  for (const row of population.slice(8)) {
    expect(usable.has(row.number), `initial family debt for ${row.number}`).toBe(true);
    expect(calls.get(row.number)).toBe(2);
  }
  expect(calls.get(106)).toBe(2);
  expect(calls.get(107)).toBe(2);
  expect(calls.get(100)).toBeGreaterThan(3); // Preferred rows continue refresh.
  expect(view.result.current.displayOf(population[6])?.freshness).toBe("retained");
  unsubscribe();
});
