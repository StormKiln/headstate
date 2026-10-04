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
