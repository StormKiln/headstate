import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Artifact } from "@/types/pr";

const bridge = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: bridge.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
import { useArtifacts, useArtifactSizes, useAllWorktreeSizes, useWorktreeDirs, useArtifactScanFailures, useWorktrees } from "./hooks";

function harness() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return { qc, wrapper: ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider> };
}
const artifact = (path: string): Artifact => ({ path, repo_path: "/repo", kind: "cargo_target", size_bytes: null });
const calls = (cmd: string) => bridge.invoke.mock.calls.filter(([name]) => name === cmd);

beforeEach(() => {
  bridge.invoke.mockReset();
  bridge.invoke.mockImplementation((cmd: string, args: { paths?: string[]; dirs?: string[] } = {}) => {
    if (cmd === "size_artifacts_report") return Promise.resolve(args.paths!.map((p) => [p, 10, 60]));
    if (cmd === "set_worktree_dirs") return Promise.resolve(args.dirs);
    return Promise.resolve([]);
  });
});

describe("disk measurement membership", () => {
  it("sizes only added artifacts, retains warm rows, and does no walk for removal", async () => {
    const { wrapper } = harness();
    const { result, rerender } = renderHook(({ paths }) => useArtifactSizes(paths.map(artifact), true), { wrapper, initialProps: { paths: ["/repo/target"] } });
    await waitFor(() => expect(result.current.sizes.get("/repo/target")).toBe(10));
    rerender({ paths: ["/repo/target", "/repo/new/target"] });
    expect(result.current.sizes.get("/repo/target")).toBe(10);
    await waitFor(() => expect(result.current.sizes.size).toBe(2));
    expect(calls("size_artifacts_report").map(([, args]) => args.paths)).toEqual([["/repo/target"], ["/repo/new/target"]]);
    rerender({ paths: ["/repo/new/target"] });
    await waitFor(() => expect(result.current.sizes.size).toBe(1));
    expect(calls("size_artifacts_report")).toHaveLength(2);
  });

  it("explicit discovery refresh remeasures a completed build", async () => {
    let bytes = 10;
    bridge.invoke.mockImplementation((cmd, args) => Promise.resolve(cmd === "size_artifacts_report" ? args.paths.map((p: string) => [p, bytes, 60]) : [artifact("/repo/target")]));
    const { wrapper } = harness();
    const { result } = renderHook(() => {
      const discovery = useArtifacts(true);
      return { discovery, measured: useArtifactSizes(discovery.data ?? [], true) };
    }, { wrapper });
    await waitFor(() => expect(result.current.measured.sizes.get("/repo/target")).toBe(10));
    bytes = 90;
    await act(() => result.current.discovery.refetch());
    await waitFor(() => expect(result.current.measured.sizes.get("/repo/target")).toBe(90));
    expect(calls("size_artifacts_report")).toHaveLength(2);
  });

  it("ignores an obsolete response after scope membership changes", async () => {
    let finishOld!: (v: unknown) => void;
    bridge.invoke.mockImplementation((cmd, args) => {
      if (cmd !== "size_artifacts_report") return Promise.resolve([]);
      if (args.paths[0] === "/repo/old") return new Promise((resolve) => { finishOld = resolve; });
      return Promise.resolve([["/repo/new", 80, 60]]);
    });
    const { wrapper } = harness();
    const { result, rerender } = renderHook(({ path }) => useArtifactSizes([artifact(path)], true), { wrapper, initialProps: { path: "/repo/old" } });
    await waitFor(() => expect(calls("size_artifacts_report")).toHaveLength(1));
    rerender({ path: "/repo/new" });
    await waitFor(() => expect(result.current.sizes.get("/repo/new")).toBe(80));
    await act(async () => finishOld([["/repo/old", 999, 60]]));
    expect([...result.current.sizes]).toEqual([["/repo/new", 80]]);
  });

  it("worktree additions remeasure only the changed repository and removal keeps sizes", async () => {
    const { qc, wrapper } = harness();
    qc.setQueryData(["worktree-sizes", "/a"], new Map([["/a/main", 10]]));
    qc.setQueryData(["worktree-sizes", "/b"], new Map([["/b/main", 20]]));
    bridge.invoke.mockImplementation((cmd) => Promise.resolve(cmd === "size_worktrees" ? [["/a/main", 10], ["/a/new", 30]] : []));
    const repos = (paths: string[]) => [{ path: "/a", worktrees: paths.map((path) => ({ path })) }, { path: "/b", worktrees: [{ path: "/b/main" }] }];
    const { result, rerender } = renderHook(({ paths }) => useAllWorktreeSizes(["/a", "/b"], true, repos(paths)), { wrapper, initialProps: { paths: ["/a/main"] } });
    expect(result.current.sizes.get("/b/main")).toBe(20);
    rerender({ paths: ["/a/main", "/a/new"] });
    expect(result.current.sizes.get("/a/main")).toBe(10);
    await waitFor(() => expect(result.current.sizes.get("/a/new")).toBe(30));
    expect(calls("size_worktrees").map(([, args]) => args.repoPath)).toEqual(["/a"]);
    rerender({ paths: ["/a/new"] });
    await waitFor(() => expect(result.current.sizes.has("/a/main")).toBe(false));
    expect(calls("size_worktrees")).toHaveLength(1);
  });

  it("root changes invalidate warm discovery and measurements", async () => {
    const { qc, wrapper } = harness();
    qc.setQueryData(["artifact-sizes", "/repo"], [["/repo/target", 10, 60]]);
    qc.setQueryData(["worktree-sizes", "/repo"], new Map([["/repo", 10]]));
    qc.setQueryData(["artifacts"], [artifact("/repo/target")]);
    // The saved scans are keyed by kind alone on this side, so a changed
    // root set must mark them stale or the old roots' scan keeps painting
    // (#1962's frontend half).
    qc.setQueryData(["cached-scan", "artifacts"], { payload: "[]", age_secs: 60, stale: false });
    qc.setQueryData(["cached-scan", "worktrees"], { payload: "{}", age_secs: 60, stale: false });
    const { result } = renderHook(() => useWorktreeDirs(), { wrapper });
    await act(() => result.current.set(["/new/root"]));
    expect(qc.getQueryState(["cached-scan", "artifacts"])?.isInvalidated).toBe(true);
    expect(qc.getQueryState(["cached-scan", "worktrees"])?.isInvalidated).toBe(true);
    expect(qc.getQueryState(["artifact-sizes", "/repo"])?.isInvalidated).toBe(true);
    expect(qc.getQueryState(["worktree-sizes", "/repo"])?.isInvalidated).toBe(true);
    expect(qc.getQueryState(["artifacts"])?.isInvalidated).toBe(true);
    expect(qc.getQueryData(["artifact-sizes", "/repo"])).toEqual([["/repo/target", 10, 60]]);
  });
  it("keeps a trustworthy size when a refresh cannot measure, and distinguishes an empty directory", async () => {
    let bytes: number | null = 40;
    bridge.invoke.mockImplementation((cmd, args) => Promise.resolve(cmd === "size_artifacts_report" ? args.paths.map((p: string) => [p, bytes, bytes === null ? null : 60]) : [artifact("/repo/target")]));
    const { wrapper } = harness();
    const { result } = renderHook(() => {
      const discovery = useArtifacts(true);
      return { discovery, measured: useArtifactSizes(discovery.data ?? [], true) };
    }, { wrapper });
    await waitFor(() => expect(result.current.measured.sizes.get("/repo/target")).toBe(40));
    bytes = null;
    await act(() => result.current.discovery.refetch());
    await waitFor(() => expect(result.current.measured.failed).toBe(1));
    expect(result.current.measured.sizes.get("/repo/target")).toBe(40);
    expect(result.current.measured.ages.has("/repo/target")).toBe(false);
    bytes = 0;
    await act(() => result.current.discovery.refetch());
    await waitFor(() => expect(result.current.measured.sizes.get("/repo/target")).toBe(0));
    expect(result.current.measured.failed).toBe(0);
  });

  it("binds omissions to the accepted scan rather than an obsolete scope response", async () => {
    const scans: ((v: unknown) => void)[] = [];
    bridge.invoke.mockImplementation((cmd) => cmd === "scan_artifacts_report" ? new Promise((resolve) => scans.push(resolve)) : Promise.resolve([]));
    const { wrapper } = harness();
    const { result } = renderHook(() => ({ discovery: useArtifacts(true), failures: useArtifactScanFailures(true) }), { wrapper });
    await waitFor(() => expect(scans).toHaveLength(1));
    let refreshed!: Promise<unknown>;
    act(() => { refreshed = result.current.discovery.refetch(); });
    await waitFor(() => expect(scans).toHaveLength(2));
    await act(async () => { scans[1]({ artifacts: [artifact("/repo/new")], failed_roots: [] }); await refreshed; });
    await act(async () => scans[0]({ artifacts: [], failed_roots: [{ root: "/old", why: "permission denied" }] }));
    await waitFor(() => expect(result.current.discovery.data?.[0]?.path).toBe("/repo/new"));
    expect(result.current.failures).toEqual([]);
  });

  it("membership churn does not renew the age of retained artifact measurements", async () => {
    const clock = vi.spyOn(Date, "now");
    let now = Date.now();
    clock.mockImplementation(() => now);
    try {
      const { wrapper } = harness();
      const { result, rerender } = renderHook(({ paths }) => useArtifactSizes(paths.map(artifact), true), { wrapper, initialProps: { paths: ["/repo/old"] } });
      await waitFor(() => expect(result.current.sizes.size).toBe(1));
      now += 4 * 60 * 1000;
      rerender({ paths: ["/repo/old", "/repo/new"] });
      await waitFor(() => expect(result.current.sizes.size).toBe(2));
      expect(calls("size_artifacts_report")[1][1].paths).toEqual(["/repo/new"]);
      now += 2 * 60 * 1000;
      rerender({ paths: ["/repo/old", "/repo/new", "/repo/latest"] });
      await waitFor(() => expect(result.current.sizes.size).toBe(3));
      expect(calls("size_artifacts_report")[2][1].paths).toEqual(["/repo/latest", "/repo/old"]);
    } finally { clock.mockRestore(); }
  });

  it("coalesces repeated explicit refreshes instead of restarting disk walks", async () => {
    let release!: (v: unknown) => void;
    let scans = 0;
    bridge.invoke.mockImplementation((cmd, args) => {
      if (cmd === "scan_artifacts_report") {
        scans++;
        return scans === 1 ? Promise.resolve([artifact("/repo/target")]) : new Promise((resolve) => { release = resolve; });
      }
      return Promise.resolve(cmd === "size_artifacts_report" ? args.paths.map((p: string) => [p, 10, 60]) : []);
    });
    const { wrapper } = harness();
    const { result } = renderHook(() => {
      const discovery = useArtifacts(true);
      return { discovery, measured: useArtifactSizes(discovery.data ?? [], true) };
    }, { wrapper });
    await waitFor(() => expect(result.current.measured.sizes.size).toBe(1));
    let a!: Promise<unknown>, b!: Promise<unknown>;
    act(() => { a = result.current.discovery.refetch(); b = result.current.discovery.refetch(); });
    await waitFor(() => expect(scans).toBe(2));
    await act(async () => { release([artifact("/repo/target")]); await Promise.all([a, b]); });
    expect(scans).toBe(2);
    expect(calls("size_artifacts_report")).toHaveLength(2);
  });

});

it("exclusion changes discard an old discovery response and cached scope without rescanning artifacts", async () => {
  let finishOld!: (value: unknown) => void;
  let scans = 0;
  bridge.invoke.mockImplementation((cmd, args) => {
    if (cmd === "set_discovery_exclusions") return Promise.resolve(args.paths);
    // `useWorktrees` now reads the saved scan while discovery is pending
    // (#1950). The backend's answer for a scope with no saved scan is
    // `null`; the generic `[]` below is not a shape that command returns.
    if (cmd === "read_cached_scan") return Promise.resolve(null);
    if (cmd === "list_worktrees") {
      scans++;
      if (scans === 1) return new Promise(resolve => { finishOld = resolve; });
      return Promise.resolve({ repos: [], unreadable: [], diagnostics: [] });
    }
    return Promise.resolve([]);
  });
  const { qc, wrapper } = harness();
  qc.setQueryData(["cached-scan", "worktrees"], { payload: "old scope" });
  qc.setQueryData(["artifacts"], [artifact("/repo/target")]);
  const { result } = renderHook(() => ({ dirs: useWorktreeDirs(), scan: useWorktrees() }), { wrapper });
  await waitFor(() => expect(scans).toBe(1));
  await act(() => result.current.dirs.setExclusions(["/repo/backup"]));
  await waitFor(() => expect(result.current.scan.data).toEqual([]));
  await act(async () => finishOld({ repos: [{ path: "/old" }], unreadable: ["old warning"] }));
  expect(result.current.scan.data).toEqual([]);
  expect(qc.getQueryData(["cached-scan", "worktrees"])).toBeNull();
  expect(qc.getQueryState(["cached-scan", "worktrees"])?.isInvalidated).toBe(true);
  expect(qc.getQueryData(["artifacts"])).toEqual([artifact("/repo/target")]);
  expect(calls("list_artifacts")).toHaveLength(0);
});
