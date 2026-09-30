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
  invoke.mockImplementation(async () => rows.map((pr) => ({ repo: pr.repo, number: pr.number, stack })));
  const view = renderHook(() => useReadyStacks(rows), { wrapper });
  await advance(1);
  expect(view.result.current.of(rows[0])).toEqual({ kind: "unknown" });
  expect(invoke).toHaveBeenCalledTimes(1);
  stack = exact;
  await advance(59_998);
  expect(invoke).toHaveBeenCalledTimes(1);
  await advance(2);
  expect(view.result.current.of(rows[0])).toEqual(exact);
  expect(invoke).toHaveBeenCalledTimes(2);
  expect(invoke.mock.calls[1][1]?.rows).toHaveLength(2);
  stack = { ...exact, position: 4, size: 7 };
  await advance(60_000);
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
