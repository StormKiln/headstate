import { act, cleanup, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "@/fixtures/prs";
import { prKey } from "@/lib/prIdentity";
import type { PullRequest } from "@/types/pr";
const invoke = vi.hoisted(() => vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { useReadyPushers } from "./useReadyPushers";
import { useReadyStacks } from "./useReadyStacks";

let qc: QueryClient;
let finish: () => void;
const rows = Array.from({ length: 12 }, (_, i) => ({ ...PR_FIXTURES[0], number: 100 + i }));
const priority = (numbers: number[]) => new Set(rows.filter(pr => numbers.includes(pr.number)).map(prKey));
const original = priority([100, 101, 102, 103, 104, 105]);
const scrolled = priority([100, 101, 102, 103, 104, 106]);
function wrapper({ children }: { children: ReactNode }) { return <QueryClientProvider client={qc}>{children}</QueryClientProvider>; }
function useBoth(preferred: Set<string>) {
  useReadyPushers(rows, preferred);
  useReadyStacks(rows, preferred);
}
async function advance(ms: number) { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); }
const called = () => invoke.mock.calls.map(([cmd, args]) => [cmd, (args?.rows as PullRequest[])[0].number]);
beforeEach(() => {
  vi.useFakeTimers();
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(["viewer"], "octocat");
  invoke.mockReset();
  let first = true;
  invoke.mockImplementation(async (cmd, args) => {
    if (first) { first = false; await new Promise<void>(resolve => { finish = resolve; }); }
    return (args?.rows as PullRequest[]).map(pr => ({ ...pr,
      advisory_progress: { outcome: "offered", admitted: true },
      ...(cmd === "get_ready_stacks" ? { stack: { kind: "none" }, valid_for_ms: 60_000 }
        : { last_pusher: { state: "known", login: "octocat" }, rules: { state: "read", require_last_push_approval: false, required_review_thread_resolution: false }, pusher_valid_for_ms: 60_000, rules_valid_for_ms: 600_000 }),
    }));
  });
});
afterEach(() => { cleanup(); qc.clear(); vi.useRealTimers(); });

it.each([false, true])("keeps pending classification until an ordinary window (tick=%s)", async tick => {
  const view = renderHook(({ preferred }) => useBoth(preferred), { wrapper, initialProps: { preferred: original } });
  await advance(5);
  expect(called()).toEqual([["get_ready_pushers", 100]]);
  view.rerender({ preferred: scrolled });
  await advance(5);
  expect(invoke).toHaveBeenCalledTimes(1);
  if (tick) {
    await advance(30_000);
    // A second scroll before another boundary must not undo that snapshot.
    view.rerender({ preferred: original });
    await advance(5);
  }
  expect(invoke).toHaveBeenCalledTimes(1);
  await act(async () => finish());
  await advance(5);
  expect.soft(called()[1]).toEqual(["get_ready_stacks", tick ? 105 : 106]);
  expect.soft(called()[3]).toEqual(["get_ready_pushers", tick ? 105 : 106]);
  expect(called()).toHaveLength(16);
  expect(new Set(called().map(call => JSON.stringify(call))).size).toBe(16);
});

it.each([false, true])("aggregates coalesced visible claims and cleans up unmount (unmount=%s)", async unmount => {
  renderHook(() => useBoth(original), { wrapper });
  await advance(5);
  const other = renderHook(() => useBoth(scrolled), { wrapper });
  await advance(5);
  if (unmount) other.unmount();
  expect(invoke).toHaveBeenCalledTimes(1);
  await act(async () => finish());
  await advance(5);
  expect(called()[1]).toEqual(["get_ready_stacks", unmount ? 106 : 107]);
  expect(called()[3]).toEqual(["get_ready_pushers", unmount ? 106 : 107]);
  expect(called()).toHaveLength(16);
  expect(new Set(called().map(call => JSON.stringify(call))).size).toBe(16);
});
