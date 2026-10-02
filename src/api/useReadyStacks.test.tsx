import { act, cleanup, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "@/fixtures/prs";
import type { PrStack } from "@/types/pr";

const invoke = vi.hoisted(() => vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { useReadyStacks } from "./useReadyStacks";

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

it("refreshes mounted unknown and off-list membership without head changes, once per minute in a shared batch", async () => {
  let stack: PrStack = { kind: "unknown" };
  invoke.mockImplementation(async () => rows.map((pr) => ({ repo: pr.repo, number: pr.number, head_oid: pr.head_oid, base_ref: pr.base_ref, valid_for_ms: stack.kind === "unknown" ? 5_000 : 60_000, stack })));
  const view = renderHook(() => useReadyStacks(rows), { wrapper });
  await advance(1);
  expect(view.result.current.of(rows[0])).toEqual({ kind: "unknown" });
  expect(invoke).toHaveBeenCalledTimes(1);
  stack = exact;
  await advance(29_998);
  expect(invoke).toHaveBeenCalledTimes(1);
  await advance(2);
  await advance(2);
  expect(view.result.current.of(rows[0])).toEqual(exact);
  expect(invoke).toHaveBeenCalledTimes(2);
  expect(invoke.mock.calls[1][1]?.rows).toHaveLength(2);
  stack = { ...exact, position: 4, size: 7 };
  await advance(60_000);
  await advance(2);
  expect(view.result.current.of(rows[0])).toEqual(stack);
  expect(invoke).toHaveBeenCalledTimes(3);
  view.unmount();
  await advance(120_000);
  expect(invoke).toHaveBeenCalledTimes(3);
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
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(invoke.mock.calls[0][1]?.rows).toHaveLength(8);
  view.unmount();
  await advance(120_000);
  expect(invoke).toHaveBeenCalledTimes(1);
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
  expect(invoke).toHaveBeenCalledTimes(2);
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
  expect(invoke).toHaveBeenCalledTimes(1);
  Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
  await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
  await advance(120_000);
  expect(invoke).toHaveBeenCalledTimes(1);
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
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(live.result.current.of(rows[0])).toEqual(exact);
});
