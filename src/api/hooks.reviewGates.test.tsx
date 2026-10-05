import { act, cleanup, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "@/fixtures/prs";
import type { PrDetail } from "@/types/pr";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
import { useReviewGates } from "./hooks";
import { retireSourceOwnership } from "./sourceRefreshHooks";
let qc: QueryClient;
const pr = { ...PR_FIXTURES[0], head_repo: "synthetic/repo", head_oid: "head", head_ref: "feature" } as unknown as PrDetail;
const good = () => ({ rules: { state: "read", require_last_push_approval: true, required_review_thread_resolution: true }, last_pusher: { state: "known", login: "octocat" }, rules_valid_for_ms: 600_000, pusher_valid_for_ms: 60_000 });
function wrapper({ children }: { children: ReactNode }) { return <QueryClientProvider client={qc}>{children}</QueryClientProvider>; }
async function advance(ms = 5) { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); }
beforeEach(() => { vi.useFakeTimers(); qc = new QueryClient({ defaultOptions: { queries: { retry: false } } }); qc.setQueryData(["viewer"], "octocat"); invoke.mockReset(); invoke.mockResolvedValue(good()); });
afterEach(() => { cleanup(); qc.clear(); vi.useRealTimers(); });
it("recovers settled failures while stationary and coalesces matching observers", async () => {
  invoke.mockResolvedValueOnce({ rules: { state: "unreadable", reason: "offline" }, last_pusher: { state: "not_needed" }, rules_valid_for_ms: 5000 });
  const first = renderHook(() => useReviewGates(pr, false), { wrapper });
  const second = renderHook(() => useReviewGates(pr, false), { wrapper });
  await advance(); expect(first.result.current.data?.rules.state).toBe("unreadable");
  expect(invoke).toHaveBeenCalledTimes(1);
  await advance(30_000); await advance();
  expect(first.result.current.data?.rules.state).toBe("read");
  expect(second.result.current.data?.rules.state).toBe("read");
  expect(invoke).toHaveBeenCalledTimes(2);
});
it("expires pusher and policy independently while their replacement is slow", async () => {
  const view = renderHook(() => useReviewGates(pr, false), { wrapper });
  await advance(); expect(view.result.current.data?.last_pusher.state).toBe("known");
  invoke.mockImplementation(() => new Promise(() => {}));
  await advance(60_001);
  expect(view.result.current.data?.rules.state).toBe("read");
  expect(view.result.current.data?.last_pusher.state).not.toBe("known");
  await advance(540_001);
  expect(view.result.current.data?.rules.state).not.toBe("read");
});
it("does not certify old-peer replies or renew original lifetimes after slow transport", async () => {
  const { rules, last_pusher } = good(); invoke.mockResolvedValueOnce({ rules, last_pusher });
  const view = renderHook(() => useReviewGates(pr, false), { wrapper });
  await advance(); expect(view.result.current.data?.rules.state).not.toBe("read");
  let finish!: (value: unknown) => void;
  invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  act(() => { void view.result.current.refetch({ cancelRefetch: false }); });
  await advance(2000);
  await act(async () => finish({ ...good(), rules_valid_for_ms: 1000, pusher_valid_for_ms: 1000 }));
  await advance();
  expect(view.result.current.data?.rules.state).not.toBe("read");
  expect(view.result.current.data?.last_pusher.state).not.toBe("known");
});
it("suppresses retired source replies even without removing queries", async () => {
  let finish!: (value: unknown) => void;
  invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const view = renderHook(() => useReviewGates(pr, false), { wrapper });
  await advance(); const oldFinish = finish; act(() => retireSourceOwnership(qc));
  await act(async () => oldFinish(good())); await advance();
  expect(view.result.current.data?.rules.state).not.toBe("read");
});

it("observes a same-head policy change after the original ten-minute rules lifetime", async () => {
  invoke.mockResolvedValueOnce({ ...good(), last_pusher: { state: "not_needed" } });
  const view = renderHook(() => useReviewGates(pr, false), { wrapper });
  await advance();
  invoke.mockResolvedValue({ ...good(), rules: { state: "read", require_last_push_approval: false, required_review_thread_resolution: false }, last_pusher: { state: "not_needed" } });
  await advance(599_000); expect(invoke).toHaveBeenCalledTimes(1);
  await advance(1005);
  expect(view.result.current.data?.rules).toMatchObject({ required_review_thread_resolution: false });
  expect(invoke).toHaveBeenCalledTimes(2);
});
it("reuses mounted native remaining policy lifetime independently of partial pusher failure", async () => {
  invoke.mockResolvedValue({ ...good(), rules_valid_for_ms: 45_000, last_pusher: { state: "unknown", reason: "timeout" } });
  const view = renderHook(() => useReviewGates(pr, false), { wrapper });
  await advance(); expect(view.result.current.data?.rules.state).toBe("read");
  invoke.mockImplementation(() => new Promise(() => {}));
  await advance(30_005); expect(invoke).toHaveBeenCalledTimes(2);
  expect(view.result.current.data?.rules.state).toBe("read");
  await advance(15_005); expect(view.result.current.data?.rules.state).not.toBe("read");
});
it("stops background reads while hidden and expires guidance regardless of visibility", async () => {
  const view = renderHook(() => useReviewGates(pr, false), { wrapper });
  await advance();
  act(() => { Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" }); document.dispatchEvent(new Event("visibilitychange")); });
  await advance(600_005);
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(view.result.current.data?.rules.state).not.toBe("read");
  act(() => { Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" }); document.dispatchEvent(new Event("visibilitychange")); });
  await advance(); expect(invoke).toHaveBeenCalledTimes(2);
});
it("retiring an account cannot reacquire authority from its old reply", async () => {
  let finish!: (value: unknown) => void;
  invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const view = renderHook(() => useReviewGates(pr, false), { wrapper });
  await advance(); const oldFinish = finish;
  act(() => { qc.setQueryData(["viewer"], "other-account"); });
  await act(async () => oldFinish(good())); await advance();
  expect(view.result.current.data?.rules.state).not.toBe("read");
});
