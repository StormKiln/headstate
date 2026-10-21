import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

const invoke = vi.hoisted(() => vi.fn(() => Promise.resolve("Already up to date.")));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { useFetchRefs, usePullCheckout, useUpdateAllRepositories, useUpdateAllState } from "./hooks";

/// #346: "Update to latest" appeared to do nothing — the row stayed
/// yellow saying "N commits behind" for ~10 seconds after a SUCCESSFUL
/// pull, so the button looked broken.
///
/// The pull invalidated `["worktrees"]`, the base listing. But the
/// upstream line is rendered from `["worktree-safety"]`, which nothing
/// invalidated — it only refreshed when something else happened to.
/// The query keys one call invalidated, in the order it invalidated them.
///
/// Shared by every case below rather than rebuilt in each: the property
/// under test is the same for all three hooks, and three copies of this
/// scaffolding is how one of them ends up checking a key the others do
/// not.
const invalidationsOf = async (call: (qc: QueryClient) => Promise<unknown>) => {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidated: unknown[] = [];
  const original = qc.invalidateQueries.bind(qc);
  vi.spyOn(qc, "invalidateQueries").mockImplementation((filters) => {
    invalidated.push((filters as { queryKey?: unknown[] })?.queryKey?.[0]);
    return original(filters);
  });
  await call(qc);
  return invalidated;
};

const wrapperFor = (qc: QueryClient) =>
  ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );

describe("usePullCheckout", () => {
  beforeEach(() => invoke.mockClear());

  it("refreshes the classification that renders the upstream line", async () => {
    const invalidated = await invalidationsOf(async (qc) => {
      const { result } = renderHook(() => usePullCheckout(), { wrapper: wrapperFor(qc) });
      await result.current("/code/proj");
    });

    await waitFor(() => expect(invalidated).toContain("worktrees"));
    // The one that was missing. Without it a successful pull leaves the
    // row saying the checkout is still behind.
    expect(invalidated).toContain("worktree-safety");
  });
});

/// #1042 added a THIRD cache holding the ahead/behind fact: the All
/// Repositories overview's verdicts, under `["repo-upstream", path]`,
/// with a five-minute `staleTime`.
///
/// #346 is the report these guard against, and it is the same report
/// three times over: a successful action that leaves the screen saying
/// the work was not done. The two keys above were each added after
/// exactly that complaint; this one is added with them rather than after
/// a fourth round of it.
describe("the overview's verdicts are refreshed by everything that moves a ref", () => {
  beforeEach(() => invoke.mockClear());

  it("is invalidated by a pull", async () => {
    const invalidated = await invalidationsOf(async (qc) => {
      const { result } = renderHook(() => usePullCheckout(), { wrapper: wrapperFor(qc) });
      await result.current("/code/proj");
    });
    await waitFor(() => expect(invalidated).toContain("repo-upstream"));
  });

  /// Sharpest of the three: this feature exists to make a stale verdict
  /// fresh, and the overview is the surface that most loudly qualifies
  /// its verdicts by ref age. Refreshing the age note without the verdict
  /// beside it is a worse lie than the stale one being fixed.
  it("is invalidated by a fetch", async () => {
    const invalidated = await invalidationsOf(async (qc) => {
      const { result } = renderHook(() => useFetchRefs(), { wrapper: wrapperFor(qc) });
      await result.current("/code/proj");
    });
    await waitFor(() => expect(invalidated).toContain("repo-upstream"));
    // Alongside the two it already refreshed, not instead of either.
    expect(invalidated).toContain("worktrees");
    expect(invalidated).toContain("worktree-safety");
  });

  /// Update All's button lives ON the overview table, so its own rows are
  /// the ones that would otherwise sit stale -- a run reporting that it
  /// moved 30 repositories, above a table still saying they are behind.
  it("is invalidated by Update All", async () => {
    const invalidated = await invalidationsOf(async (qc) => {
      const { result } = renderHook(() => useUpdateAllRepositories(), {
        wrapper: wrapperFor(qc),
      });
      await result.current();
    });
    await waitFor(() => expect(invalidated).toContain("repo-upstream"));
  });

  /// #1947, #1954: a rejected run may still have moved repositories (one
  /// that died part-way), and the phone's call gives up while the desktop
  /// carries on -- so the views are refreshed on a rejection too.
  it("is invalidated by an Update All that rejected", async () => {
    invoke.mockImplementationOnce(() => Promise.reject(new Error("the update run failed to finish")));
    const invalidated = await invalidationsOf(async (qc) => {
      const { result } = renderHook(() => useUpdateAllRepositories(), {
        wrapper: wrapperFor(qc),
      });
      await result.current().catch(() => {});
    });
    await waitFor(() => expect(invalidated).toContain("repo-upstream"));
    expect(invalidated).toContain("worktrees");
    expect(invalidated).toContain("update-all-state");
  });
});

/// #1946: the run outlives whoever started it, so the registry is read --
/// polled while the run is going -- and when a run it watched ends, the
/// repository views are refreshed, since the caller that would have done
/// it may be long gone.
describe("useUpdateAllState", () => {
  beforeEach(() => invoke.mockReset());

  it("reads the registry, and refreshes the repository views when a watched run ends", async () => {
    const states: unknown[] = [
      { state: "running", run: 3, done: 1, total: 2 },
      { state: "done", run: 3, report: { outcomes: [], cancelled: false, timedOut: false, unreadable: [] } },
    ];
    invoke.mockImplementation((() => Promise.resolve(states.length > 1 ? states.shift() : states[0])) as never);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidated: unknown[] = [];
    const original = qc.invalidateQueries.bind(qc);
    vi.spyOn(qc, "invalidateQueries").mockImplementation((filters) => {
      invalidated.push((filters as { queryKey?: unknown[] })?.queryKey?.[0]);
      return original(filters);
    });
    const { result } = renderHook(() => useUpdateAllState(), { wrapper: wrapperFor(qc) });
    await waitFor(() => expect(result.current.data).toMatchObject({ state: "running", done: 1 }));
    expect((invoke.mock.calls as unknown[][])[0]?.[0]).toBe("update_all_state");
    expect(invalidated).not.toContain("worktrees");
    // The poll, without waiting out its interval.
    await result.current.refetch();
    await waitFor(() => expect(result.current.data).toMatchObject({ state: "done", run: 3 }));
    await waitFor(() => expect(invalidated).toContain("worktrees"));
    expect(invalidated).toContain("repo-upstream");
  });
});
