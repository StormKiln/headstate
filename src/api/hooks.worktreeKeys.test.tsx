import { describe, expect, it, vi, beforeEach } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";

/// #1957: the cache-key, staleness and double-scan defects behind the
/// Worktrees and Repositories pages,
/// run against the real hooks with the transport mocked.

const invoke = vi.hoisted(() =>
  vi.fn<(cmd: string, args?: unknown) => Promise<unknown>>((cmd) => {
    if (cmd === "assess_worktree") {
      return Promise.resolve({ subjects: [], subjects_elided: 0 });
    }
    return Promise.resolve([]);
  }),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { useAssessment, useWorktreeDirs, useWorktreeSizes } from "./hooks";

const wrapper = (qc: QueryClient) =>
  function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  };

const fresh = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

beforeEach(() => invoke.mockClear());

describe("useAssessment", () => {
  /// After a checkout the same worktree is on another branch, and the old
  /// branch's assessment must not be served from cache for five minutes.
  it("asks again when the worktree's branch changes", async () => {
    const qc = fresh();
    const { rerender } = renderHook(
      ({ branch }) => useAssessment("/code/app", "/code/app-wt", branch),
      { wrapper: wrapper(qc), initialProps: { branch: "feature-a" } },
    );
    await waitFor(() =>
      expect(invoke.mock.calls.filter((c) => c[0] === "assess_worktree")).toHaveLength(1),
    );
    rerender({ branch: "feature-b" });
    await waitFor(() =>
      expect(invoke.mock.calls.filter((c) => c[0] === "assess_worktree")).toHaveLength(2),
    );
    const branches = invoke.mock.calls
      .filter((c) => c[0] === "assess_worktree")
      .map((c) => (c[1] as { branch: string }).branch);
    expect(branches).toEqual(["feature-a", "feature-b"]);
  });
});

describe("useWorktreeDirs on a Save that changes both settings", () => {
  /// Saving exclusions AND scan roots used to walk every root twice, in
  /// sequence: `setExclusions` reset the scan and awaited its refetch,
  /// then `set` refetched it again.
  it("scans once", async () => {
    const qc = fresh();
    let scans = 0;
    invoke.mockImplementation((cmd, args) => {
      if (cmd === "set_discovery_exclusions") return Promise.resolve((args as { paths: string[] }).paths);
      if (cmd === "set_worktree_dirs") return Promise.resolve((args as { dirs: string[] }).dirs);
      return Promise.resolve([]);
    });
    const { result } = renderHook(
      () => {
        // The page's scan, active as it is while Settings is open over it.
        useQuery({
          queryKey: ["worktrees"],
          queryFn: () => {
            scans += 1;
            return Promise.resolve({ repos: [], unreadable: [] });
          },
        });
        return useWorktreeDirs();
      },
      { wrapper: wrapper(qc) },
    );
    await waitFor(() => expect(scans).toBe(1));

    await act(async () => {
      await result.current.setExclusions(["/code/archive"], { rescan: false });
      await result.current.set(["/code", "/work"]);
    });
    expect(scans).toBe(2);
  });
});

describe("useWorktreeSizes", () => {
  /// `Math.min()` of no members is `Infinity`, which made the query's
  /// data timestamp infinite: never stale, never refetched.
  it("gives a member-less query a finite timestamp", async () => {
    const qc = fresh();
    const { result } = renderHook(() => useWorktreeSizes("/code/empty", []), {
      wrapper: wrapper(qc),
    });
    await waitFor(() => expect(result.current.isFetching).toBe(false));
    expect(Number.isFinite(result.current.dataUpdatedAt)).toBe(true);
  });
});
