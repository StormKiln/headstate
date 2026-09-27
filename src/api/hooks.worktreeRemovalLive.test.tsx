import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Worktree, WorktreeRemovalFrame } from "@/types/pr";

/// #1544: a bulk removal left the list unchanged until the WHOLE batch
/// returned -- 30 seconds for a hundred rows. Each row must leave as
/// its own removal succeeds, mapped from a path-free progress frame by
/// index onto the list this call sent.
///
/// Mocked at `@tauri-apps/api/event`, beneath `./transport`, the seam
/// the phone swaps -- the same setup `hooks.branchDelete.test.tsx` uses.
const bus = vi.hoisted(() => {
  const listeners = new Map<string, Set<(e: { payload: unknown }) => void>>();
  return {
    listeners,
    emit(name: string, payload: unknown) {
      for (const cb of listeners.get(name) ?? []) cb({ payload });
    },
    reset() {
      listeners.clear();
    },
  };
});

/// The command, held open until the test settles it -- so frames can be
/// delivered while the batch is still running, which is the case at
/// issue.
const call = vi.hoisted(() => ({
  resolve: (() => {}) as (value: unknown) => void,
  reject: (() => {}) as (reason: unknown) => void,
  args: null as null | { runId: number | null; worktreePaths: string[] },
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((name: string, args: { runId: number | null; worktreePaths: string[] }) => {
    if (name !== "remove_worktrees") return new Promise(() => {});
    call.args = args;
    return new Promise((resolve, reject) => {
      call.resolve = resolve;
      call.reject = reject;
    });
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string, cb: (e: { payload: unknown }) => void) => {
    const set = bus.listeners.get(name) ?? new Set();
    set.add(cb);
    bus.listeners.set(name, set);
    return Promise.resolve(() => set.delete(cb));
  }),
}));

import { useRemoveWorktrees } from "./hooks";

const wt = (path: string) => ({ path, branch: "b" }) as unknown as Worktree;
const A = "/code/app/wt-a";
const B = "/code/app/wt-b";
const C = "/code/app/wt-c";

const frame = (f: WorktreeRemovalFrame) =>
  act(() => {
    bus.emit("worktree-removal-progress", f);
  });

const flush = () => act(async () => {});

describe("useRemoveWorktrees, row by row", () => {
  beforeEach(() => {
    bus.reset();
    call.args = null;
  });

  const start = async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData<Worktree[]>(["worktree-safety", "/code/app"], [wt(A), wt(B), wt(C)]);
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => useRemoveWorktrees(), { wrapper });
    const removed: string[] = [];
    let settled: Promise<unknown> = Promise.resolve();
    await act(async () => {
      settled = result.current("/code/app", [A, B, C], (p) => removed.push(p));
      // Swallowed here and asserted by the test that needs it.
      settled.catch(() => {});
    });
    await flush();
    expect(call.args).not.toBeNull();
    const run = call.args?.runId ?? null;
    expect(typeof run).toBe("number");
    const rows = () =>
      qc.getQueryData<Worktree[]>(["worktree-safety", "/code/app"])?.map((w) => w.path);
    return { run: run as number, rows, removed, settled: () => settled };
  };

  /// The issue's own test: the second item is still in flight, and the
  /// first has already left the list while the other two remain.
  it("drops the first row as soon as its removal succeeds, and only that row", async () => {
    const { run, rows, removed } = await start();

    frame({ run, done: 1, total: 3, removed: true });

    expect(rows()).toEqual([B, C]);
    expect(removed).toEqual([A]);
  });

  /// A refusal is still on disk and is the row that needs attention: no
  /// frame and no final outcome may drop it.
  it("never drops a refused row", async () => {
    const { run, rows, removed, settled } = await start();

    frame({ run, done: 1, total: 3, removed: true });
    frame({ run, done: 2, total: 3, removed: false });
    frame({ run, done: 3, total: 3, removed: true });
    expect(rows()).toEqual([B]);

    await act(async () => {
      call.resolve([
        { path: A, error: null },
        { path: B, error: "it went dirty" },
        { path: C, error: null },
      ]);
      await settled();
    });

    expect(rows()).toEqual([B]);
    expect(removed).toEqual([A, C]);
  });

  /// The event is app-global and a phone can run a removal at the same
  /// time. Another run's index names a row in a list this call never
  /// saw, so mapping it here would drop a worktree still on disk.
  it("ignores another run's frames", async () => {
    const { run, rows, removed } = await start();

    frame({ run: run + 1, done: 1, total: 3, removed: true });
    frame({ run: null, done: 2, total: 3, removed: true });

    expect(rows()).toEqual([A, B, C]);
    expect(removed).toEqual([]);
  });

  /// Partial is not nothing: a run that fails midway has already
  /// deleted what it deleted, and the list must not bring it back.
  it("keeps already-removed rows removed when the run fails midway", async () => {
    const { run, rows, settled } = await start();

    frame({ run, done: 1, total: 3, removed: true });
    await act(async () => {
      call.reject("the connection dropped");
      await settled().catch(() => {});
    });

    await expect(settled()).rejects.toBe("the connection dropped");
    expect(rows()).toEqual([B, C]);
  });

  /// A frame lost in transit costs only the live update: the final
  /// outcomes drop every removed row regardless.
  it("drops removed rows from the outcomes when no frame arrived", async () => {
    const { rows, removed, settled } = await start();

    await act(async () => {
      call.resolve([
        { path: A, error: null },
        { path: B, error: null },
        { path: C, error: "locked" },
      ]);
      await settled();
    });

    expect(rows()).toEqual([C]);
    expect(removed).toEqual([A, B]);
  });
});
