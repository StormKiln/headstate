import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderHook, waitFor, act } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

/// The backend, reduced to what `read_cached_scan` does: it answers for
/// the CURRENT root set (and, for worktrees, the current exclusions),
/// because that is what its row key is built from.
const backend = vi.hoisted(() => ({ dirs: ["/a"], exclusions: [] as string[], reads: 0 }));
const invoke = vi.hoisted(() =>
  vi.fn((cmd: string): Promise<unknown> => {
    if (cmd === "get_worktree_dirs") return Promise.resolve([...backend.dirs]);
    if (cmd === "get_discovery_exclusions") return Promise.resolve([...backend.exclusions]);
    if (cmd === "read_cached_scan") {
      backend.reads += 1;
      const scope = [...backend.dirs, ...backend.exclusions.map((e) => `-${e}`)].join(",");
      return Promise.resolve({ payload: JSON.stringify(scope), age_secs: 60, stale: false });
    }
    return Promise.resolve(null);
  }),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { createQueryClient } from "./queryClient";
import { useCachedScan } from "./hooks";

const harness = () => {
  const qc = createQueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
  return { qc, wrapper };
};

beforeEach(() => {
  backend.dirs = ["/a"];
  backend.exclusions = [];
  backend.reads = 0;
  invoke.mockClear();
});

/// #1962: the frontend keyed the saved scan by kind alone, with
/// `staleTime: Infinity`, while the backend keys it by the root set. So
/// once read, the previous folders' scan was what Artifacts painted for
/// the rest of the session. #1967 added invalidation on the settings
/// path; these change the scope WITHOUT that path, so they pass only if
/// the key itself carries the scope.
describe("useCachedScan is keyed by the scope it was read for (#1962)", () => {
  it("re-reads under a new root set instead of painting the old roots' scan", async () => {
    const { qc, wrapper } = harness();
    const { result } = renderHook(() => useCachedScan<string>("artifacts", true), { wrapper });
    await waitFor(() => expect(result.current?.data).toBe("/a"));

    backend.dirs = ["/b"];
    await act(async () => { qc.setQueryData(["worktree-dirs"], ["/b"]); });

    await waitFor(() => expect(result.current?.data).toBe("/b"));
    expect(backend.reads).toBe(2);
    // The old roots' answer is still held, but only under the old roots:
    // no entry pairs it with the new set.
    const entries = qc.getQueriesData<{ payload: string }>({ queryKey: ["cached-scan", "artifacts"] });
    expect(entries.map(([key, data]) => [key[2], data?.payload])).toEqual(
      expect.arrayContaining([
        [{ roots: ["/a"], exclusions: [] }, JSON.stringify("/a")],
        [{ roots: ["/b"], exclusions: [] }, JSON.stringify("/b")],
      ]),
    );
  });

  it("treats a reordered root set as the same scope, as the backend's sorted key does", async () => {
    backend.dirs = ["/a", "/b"];
    const { qc, wrapper } = harness();
    const { result } = renderHook(() => useCachedScan<string>("artifacts", true), { wrapper });
    await waitFor(() => expect(result.current?.data).toBe("/a,/b"));
    await act(async () => { qc.setQueryData(["worktree-dirs"], ["/b", "/a"]); });
    // Long enough for the re-render and any read it would have started.
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(result.current?.data).toBe("/a,/b");
    expect(backend.reads).toBe(1);
  });

  it("re-reads the worktree scan when the discovery exclusions change", async () => {
    const { qc, wrapper } = harness();
    const { result } = renderHook(() => useCachedScan<string>("worktrees", true), { wrapper });
    await waitFor(() => expect(result.current?.data).toBe("/a"));

    backend.exclusions = ["/a/backup"];
    await act(async () => { qc.setQueryData(["discovery-exclusions"], ["/a/backup"]); });

    await waitFor(() => expect(result.current?.data).toBe("/a,-/a/backup"));
  });

  it("does not read the cache before it knows which scope it is reading for", async () => {
    let releaseDirs!: (dirs: string[]) => void;
    invoke.mockImplementationOnce(() => new Promise((resolve) => { releaseDirs = resolve; }));
    const { wrapper } = harness();
    const { result } = renderHook(() => useCachedScan<string>("artifacts", true), { wrapper });
    await waitFor(() => expect(invoke.mock.calls.some(([cmd]) => cmd === "get_worktree_dirs")).toBe(true));
    expect(backend.reads).toBe(0);
    expect(result.current).toBeUndefined();
    await act(async () => releaseDirs(["/a"]));
    await waitFor(() => expect(result.current?.data).toBe("/a"));
  });
});
