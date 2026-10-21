import { afterEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { Artifact, Venv } from "@/types/pr";

/// A finished disk scan or measurement is kept after the page is left
/// (#1974, #1944's defect in the hooks #1967 left alone), and the Venvs
/// cold start reads its saved scan (#1975). Real hooks against a mocked
/// transport, desktop build.

vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: false, IS_DESKTOP_BUILD: true }));

const bridge = vi.hoisted(() => ({
  /// Open calls by command name, each with its resolver.
  open: new Map<string, ((v: unknown) => void)[]>(),
  /// Commands answered at once, with this value.
  immediate: new Map<string, unknown>(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string) => {
    if (bridge.immediate.has(cmd)) return Promise.resolve(bridge.immediate.get(cmd));
    return new Promise((resolve) => {
      const list = bridge.open.get(cmd) ?? [];
      list.push(resolve);
      bridge.open.set(cmd, list);
    });
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

import { useArtifacts, useArtifactSizes, useCachedScan, useVenvs } from "./hooks";

/// Settle the oldest open call to `cmd`.
function answer(cmd: string, value: unknown) {
  const list = bridge.open.get(cmd) ?? [];
  const resolve = list.shift();
  expect(resolve, `no open ${cmd} call`).toBeDefined();
  resolve!(value);
}

const opened = (cmd: string) => bridge.open.get(cmd)?.length ?? 0;

function harness() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
  // The scope a saved-scan read may wait on, answered at once so these
  // tests hold whether or not the cache key carries the roots.
  bridge.immediate.set("get_worktree_dirs", ["/code"]);
  bridge.immediate.set("get_discovery_exclusions", []);
  return { qc, wrapper };
}

/// Every answered cached-scan entry for `kind`, whatever else its key
/// carries (a key that names its scope also has a disabled, scope-less
/// entry from before the scope was known).
const savedScans = (qc: QueryClient, kind: string) =>
  qc
    .getQueriesData({ queryKey: ["cached-scan", kind] })
    .map(([, data]) => data)
    .filter((data) => data !== undefined);

afterEach(() => {
  bridge.open.clear();
  bridge.immediate.clear();
});

const artifact: Artifact = { path: "/code/a/target", kind: "cargo_target", repo_path: "/code/a", size_bytes: null };

describe("a finished disk scan is kept after the page is left (#1974)", () => {
  it("useArtifacts: unmounting mid-scan does not discard the result", async () => {
    const { qc, wrapper } = harness();
    const { unmount } = renderHook(() => useArtifacts(true), { wrapper });
    await waitFor(() => expect(opened("scan_artifacts_report")).toBe(1));

    unmount();
    await act(async () => answer("scan_artifacts_report", { artifacts: [artifact], failed_roots: [] }));

    await waitFor(() =>
      expect(qc.getQueryData(["artifacts"])).toEqual({ artifacts: [artifact], failed_roots: [] }),
    );
  });

  it("useCachedScan: unmounting mid-read does not discard the saved scan", async () => {
    const { qc, wrapper } = harness();
    const saved = { payload: JSON.stringify([artifact]), age_secs: 60, stale: false };
    const { unmount } = renderHook(() => useCachedScan<Artifact[]>("artifacts", true), { wrapper });
    await waitFor(() => expect(opened("read_cached_scan")).toBe(1));

    unmount();
    await act(async () => answer("read_cached_scan", saved));

    await waitFor(() => expect(savedScans(qc, "artifacts")).toEqual([saved]));
  });

  it("useArtifactSizes: unmounting mid-measurement keeps the measured sizes", async () => {
    const { qc, wrapper } = harness();
    const { unmount } = renderHook(() => useArtifactSizes([artifact], true), { wrapper });
    await waitFor(() => expect(opened("size_artifacts_report")).toBe(1));

    unmount();
    await act(async () => answer("size_artifacts_report", [[artifact.path, 4096, 30]]));

    await waitFor(() => {
      const rows = qc.getQueryData<unknown[][]>(["artifact-sizes", artifact.repo_path, [artifact.path]]);
      expect(rows?.map((r) => r.slice(0, 3))).toEqual([[artifact.path, 4096, 30]]);
    });
  });

  // The explicit path that DOES mean "throw it away" -- a refresh or a
  // root change calls `cancelQueries` -- must still work without the
  // queryFn reading its signal.
  it("an explicit cancelQueries still drops an in-flight scan", async () => {
    const { qc, wrapper } = harness();
    renderHook(() => useArtifacts(true), { wrapper });
    await waitFor(() => expect(opened("scan_artifacts_report")).toBe(1));

    await act(() => qc.cancelQueries({ queryKey: ["artifacts"] }));
    await act(async () => answer("scan_artifacts_report", { artifacts: [artifact], failed_roots: [] }));

    expect(qc.getQueryData(["artifacts"])).toBeUndefined();
    expect(qc.getQueryState(["artifacts"])?.fetchStatus).toBe("idle");
  });
});

const venv = (path: string): Venv => ({
  path,
  project: path.split("/").pop()!,
  state: "live",
  source: "/code/p",
  size_bytes: null,
  idle_secs: null,
});

describe("the previous venv scan paints a cold start (#1975)", () => {
  it("exposes the saved scan while the live scan is pending, and drops it once it lands", async () => {
    const { wrapper } = harness();
    bridge.immediate.set("read_cached_scan", {
      payload: JSON.stringify([venv("/venvs/saved")]),
      age_secs: 3600,
      stale: false,
    });
    const { result } = renderHook(() => useVenvs(true), { wrapper });

    await waitFor(() => expect(result.current.cached?.data[0].path).toBe("/venvs/saved"));
    expect(result.current.cached?.ageSecs).toBe(3600);
    // `data` keeps its pending meaning: the saved scan is NOT the answer.
    expect(result.current.data).toBeUndefined();
    expect(result.current.isLoading).toBe(true);

    await act(async () => answer("scan_venvs", [venv("/venvs/live")]));
    await waitFor(() => expect(result.current.data?.[0].path).toBe("/venvs/live"));
    expect(result.current.cached).toBeUndefined();
  });
});
