import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
// The companion's own deadline, read from its source so the backstop
// below cannot drift under it. `?raw` for the reason
// `mirroredConstants.test.ts` gives: no `@types/node` in this project.
import clientRs from "../../src-mobile/src/client.rs?raw";

/// #1459: on the phone, the Worktrees size column stayed on skeletons
/// while the desktop resolved the same sizes. These run the real size
/// hooks against a mocked transport, as the PHONE build, and pin the
/// behaviours that keep a row from waiting on a promise nobody keeps.

vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: true, IS_DESKTOP_BUILD: false }));

type Pairs = [string, number | null][];
interface Pending {
  repoPath: string;
  resolve: (v: Pairs) => void;
  reject: (e: unknown) => void;
}

const bridge = vi.hoisted(() => ({
  /// Every `size_worktrees` the hooks have SENT, in order, still open.
  pending: [] as Pending[],
  /// Everything sent, settled or not.
  sent: [] as string[],
  handlers: new Map<string, ((e: { payload: unknown }) => void)[]>(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string, args?: { repoPath?: string }) => {
    if (cmd !== "size_worktrees") return Promise.resolve(undefined);
    const repoPath = args?.repoPath ?? "";
    bridge.sent.push(repoPath);
    return new Promise<Pairs>((resolve, reject) => {
      bridge.pending.push({ repoPath, resolve, reject });
    });
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string, cb: (e: { payload: unknown }) => void) => {
    const list = bridge.handlers.get(name) ?? [];
    list.push(cb);
    bridge.handlers.set(name, list);
    return Promise.resolve(() => {
      const now = bridge.handlers.get(name) ?? [];
      bridge.handlers.set(
        name,
        now.filter((f) => f !== cb),
      );
    });
  }),
}));

import { PHONE_SIZE_DEADLINE_MS, useAllWorktreeSizes, useWorktreeSizes } from "./hooks";

function emit(name: string, payload: unknown) {
  for (const cb of bridge.handlers.get(name) ?? []) cb({ payload });
}

/// Answer the open call for `repoPath`.
function answer(repoPath: string, pairs: Pairs) {
  const i = bridge.pending.findIndex((p) => p.repoPath === repoPath);
  expect(i, `no open size_worktrees call for ${repoPath}`).toBeGreaterThanOrEqual(0);
  const [call] = bridge.pending.splice(i, 1);
  call.resolve(pairs);
}

function fail(repoPath: string, why: unknown) {
  const i = bridge.pending.findIndex((p) => p.repoPath === repoPath);
  expect(i, `no open size_worktrees call for ${repoPath}`).toBeGreaterThanOrEqual(0);
  const [call] = bridge.pending.splice(i, 1);
  call.reject(why);
}

const open = () => bridge.pending.map((p) => p.repoPath);

function harness() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
  return { qc, wrapper };
}

/// The limiter is module state shared by every test here, so a test
/// must not leave a call holding a slot for the next one.
async function drain() {
  await act(async () => {
    while (bridge.pending.length > 0) {
      for (const p of bridge.pending.splice(0)) p.resolve([]);
      await new Promise((r) => setTimeout(r, 0));
    }
  });
}

beforeEach(() => {
  bridge.pending.length = 0;
  bridge.sent.length = 0;
  bridge.handlers.clear();
});
afterEach(drain);

const REPOS = ["/src/a", "/src/b", "/src/c", "/src/d", "/src/e"];

describe("worktree sizes on the phone (#1459)", () => {
  /// The recovery every other fix leans on: a settled command fills the
  /// rows even if not ONE `worktree-size` frame reached the phone --
  /// which is what a stream that reconnected, or lagged, delivers.
  it("fills every size from the settled result with no event at all", async () => {
    const { wrapper } = harness();
    const { result } = renderHook(() => useWorktreeSizes("/src/a"), { wrapper });
    await waitFor(() => expect(open()).toEqual(["/src/a"]));
    expect(result.current.isFetching).toBe(true);

    act(() =>
      answer("/src/a", [
        ["/src/a", 4096],
        ["/src/a/.worktrees/one", null],
      ]),
    );

    await waitFor(() => expect(result.current.isFetching).toBe(false));
    expect(result.current.partial.size).toBe(0);
    expect(result.current.data?.get("/src/a")).toBe(4096);
    // Null stays null: "could not measure", never zero.
    expect(result.current.data?.has("/src/a/.worktrees/one")).toBe(true);
    expect(result.current.data?.get("/src/a/.worktrees/one")).toBeNull();
  });

  /// The fan-out is queued on the phone, so no call spends its 120s
  /// waiting behind the others on the desktop -- and every repository
  /// still counts as pending, because a number is still coming.
  it("sends at most two size calls at once, and counts the queued ones as pending", async () => {
    const { wrapper } = harness();
    const { result } = renderHook(() => useAllWorktreeSizes(REPOS, true), { wrapper });

    await waitFor(() => expect(open()).toEqual(["/src/a", "/src/b"]));
    expect(result.current.pending).toBe(5);

    act(() => answer("/src/a", [["/src/a", 1]]));
    await waitFor(() => expect(open()).toEqual(["/src/b", "/src/c"]));
    expect(result.current.pending).toBe(4);
    expect(result.current.sizes.get("/src/a")).toBe(1);
  });

  /// Opening a repository from All repositories puts ITS call first.
  it("sends the repository on screen next, ahead of the rest of the fan-out", async () => {
    const { wrapper } = harness();
    const { rerender } = renderHook(
      ({ selected }: { selected: string | undefined }) => {
        useAllWorktreeSizes(REPOS, true);
        return useWorktreeSizes(selected);
      },
      { wrapper, initialProps: { selected: undefined as string | undefined } },
    );
    await waitFor(() => expect(open()).toEqual(["/src/a", "/src/b"]));

    rerender({ selected: "/src/e" });
    act(() => answer("/src/a", []));

    await waitFor(() => expect(open()).toEqual(["/src/b", "/src/e"]));
  });

  /// A call that times out on the companion settles the query in ERROR:
  /// not fetching, so the row leaves Pending and says it was not
  /// measured (#1042's distinction).
  it("leaves Pending when the companion reports a timeout", async () => {
    const { wrapper } = harness();
    const { result } = renderHook(() => useWorktreeSizes("/src/a"), { wrapper });
    await waitFor(() => expect(open()).toEqual(["/src/a"]));

    act(() => fail("/src/a", "Desktop is unreachable: desktop unreachable: timed out"));

    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.isFetching).toBe(false);
    expect(result.current.error).toBe("Desktop is unreachable: desktop unreachable: timed out");
  });

  /// The backstop: a call whose answer never arrives -- for whatever
  /// reason -- still ends, as a failure, rather than fetching forever.
  it("fails a call that never settles once the phone's deadline passes", async () => {
    // No `waitFor` under fake timers: it advances them itself, which
    // would spend the very deadline this test is measuring.
    vi.useFakeTimers();
    try {
      const { wrapper } = harness();
      const { result } = renderHook(() => useWorktreeSizes("/src/a"), { wrapper });
      await act(() => vi.advanceTimersByTimeAsync(0));
      expect(open()).toEqual(["/src/a"]);

      await act(() => vi.advanceTimersByTimeAsync(PHONE_SIZE_DEADLINE_MS - 1));
      expect(result.current.isFetching).toBe(true);
      await act(() => vi.advanceTimersByTimeAsync(1));
      // TanStack and React deliver the settlement on their own short
      // timers, which are fake here too.
      await act(() => vi.advanceTimersByTimeAsync(10));

      expect(result.current.isError).toBe(true);
      expect(result.current.isFetching).toBe(false);
      expect((result.current.error as Error).message).toMatch(/did not answer in time/);
    } finally {
      vi.useRealTimers();
      // The stuck call is abandoned, not answered; drop it so `drain`
      // does not resolve a promise no one is waiting on.
      bridge.pending.length = 0;
    }
  });

  /// The backstop must never beat the companion's own deadline: that
  /// one's message says what happened, this one's can only guess.
  it("gives the companion's CALL_TIMEOUT room to answer first", () => {
    const m = clientRs.match(/const CALL_TIMEOUT: Duration = Duration::from_secs\((\d+)\);/);
    expect(m, "src-mobile/src/client.rs must define CALL_TIMEOUT in seconds").toBeTruthy();
    expect(PHONE_SIZE_DEADLINE_MS).toBeGreaterThan(Number(m![1]) * 1000);
  });

  /// A reconnect retries the calls that FAILED -- the desktop is back,
  /// so the failure may not hold -- and leaves an in-flight one alone:
  /// its settled answer carries whatever frames the reconnect dropped,
  /// and re-sending it would start a second walk of the same tree.
  it("retries failed size calls when the phone reconnects, and only those", async () => {
    const { wrapper } = harness();
    const { result } = renderHook(() => useAllWorktreeSizes(["/src/a", "/src/b"], true), {
      wrapper,
    });
    await waitFor(() => expect(open()).toEqual(["/src/a", "/src/b"]));
    act(() => fail("/src/a", "Desktop is unreachable: timed out"));
    await waitFor(() => expect(result.current.pending).toBe(1));
    expect(bridge.sent).toEqual(["/src/a", "/src/b"]);

    act(() => emit("connection-state", { state: "connecting" }));
    act(() => emit("connection-state", { state: "connected" }));

    await waitFor(() => expect(bridge.sent).toEqual(["/src/a", "/src/b", "/src/a"]));
    expect(open()).toEqual(["/src/b", "/src/a"]);
    // Both hooks' listeners heard it; the second joined the first retry.
    act(() => emit("connection-state", { state: "connected" }));
    await new Promise((r) => setTimeout(r, 0));
    expect(bridge.sent).toEqual(["/src/a", "/src/b", "/src/a"]);

    act(() => answer("/src/a", [["/src/a", 10]]));
    await waitFor(() => expect(result.current.sizes.get("/src/a")).toBe(10));
  });
});
