import { afterEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { WorktreeScan } from "@/types/pr";

/// The Worktrees page must populate from what has ALREADY been measured
/// (#1924): streamed sizes (#1942), a discovery walk that finished after
/// the page was left (#1944), and the previous scan on a cold start
/// (#1950). Real hooks against a mocked transport, desktop build.

vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: false, IS_DESKTOP_BUILD: true }));

const bridge = vi.hoisted(() => ({
  /// Open calls by command name, each with its resolver.
  open: new Map<string, ((v: unknown) => void)[]>(),
  /// Commands answered at once, with this value.
  immediate: new Map<string, unknown>(),
  handlers: new Map<string, ((e: { payload: unknown }) => void)[]>(),
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
  listen: vi.fn((name: string, cb: (e: { payload: unknown }) => void) => {
    const list = bridge.handlers.get(name) ?? [];
    list.push(cb);
    bridge.handlers.set(name, list);
    return Promise.resolve(() => {
      bridge.handlers.set(name, (bridge.handlers.get(name) ?? []).filter((f) => f !== cb));
    });
  }),
}));

import { useAllWorktreeSizes, useWorktreeSizes, useWorktrees } from "./hooks";

function emit(name: string, payload: unknown) {
  for (const cb of bridge.handlers.get(name) ?? []) cb({ payload });
}

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
  return { qc, wrapper };
}

afterEach(() => {
  bridge.open.clear();
  bridge.immediate.clear();
  bridge.handlers.clear();
});

describe("streamed sizes reach the page before the walk settles (#1942)", () => {
  it("one repository: a streamed size shows while the query is still pending", async () => {
    const { wrapper } = harness();
    // Members passed and NOTHING retained: `warm()` used to return an
    // empty Map, which became `data` as placeholder and hid `partial`
    // behind the page's `data ?? partial`.
    const { result } = renderHook(() => useWorktreeSizes("/a", ["/a", "/a/wt"]), { wrapper });
    await waitFor(() => expect(opened("size_worktrees")).toBe(1));
    await waitFor(() => expect(bridge.handlers.get("worktree-size")?.length).toBeGreaterThan(0));

    act(() => emit("worktree-size", ["/a/wt", 42]));

    await waitFor(() => expect(result.current.sizes.get("/a/wt")).toBe(42));
    expect(result.current.isPlaceholderData).toBe(true);
    // Not measured yet is ABSENT, never zero.
    expect(result.current.sizes.has("/a")).toBe(false);

    // Once settled, the walk's own answer is the authority.
    await act(async () => answer("size_worktrees", [["/a", 100], ["/a/wt", 40]]));
    await waitFor(() => expect(result.current.isPlaceholderData).toBe(false));
    expect([...result.current.sizes]).toEqual([["/a", 100], ["/a/wt", 40]]);
  });

  it("one repository: a streamed value beats a retained one for the same path", async () => {
    const { qc, wrapper } = harness();
    qc.setQueryData(["worktree-sizes", "/a"], new Map([["/a", 10]]));
    const { result } = renderHook(() => useWorktreeSizes("/a", ["/a", "/a/wt"]), { wrapper });
    await waitFor(() => expect(bridge.handlers.get("worktree-size")?.length).toBeGreaterThan(0));
    expect(result.current.sizes.get("/a")).toBe(10);

    act(() => emit("worktree-size", ["/a", 99]));

    await waitFor(() => expect(result.current.sizes.get("/a")).toBe(99));
  });

  it("all repositories: a retained placeholder does not overwrite a fresher streamed size", async () => {
    const { qc, wrapper } = harness();
    qc.setQueryData(["worktree-sizes", "/a"], new Map([["/a", 10]]));
    const repos = [{ path: "/a", worktrees: [{ path: "/a" }, { path: "/a/wt" }] }];
    const { result } = renderHook(() => useAllWorktreeSizes(["/a"], true, repos), { wrapper });
    await waitFor(() => expect(bridge.handlers.get("worktree-size")?.length).toBeGreaterThan(0));
    expect(result.current.sizes.get("/a")).toBe(10);

    act(() => {
      emit("worktree-size", ["/a", 99]);
      emit("worktree-size", ["/a/wt", 7]);
    });

    await waitFor(() => expect(result.current.sizes.get("/a")).toBe(99));
    expect(result.current.sizes.get("/a/wt")).toBe(7);

    // And the settled answer wins once it lands.
    await act(async () => answer("size_worktrees", [["/a", 98], ["/a/wt", 7]]));
    await waitFor(() => expect(result.current.sizes.get("/a")).toBe(98));
  });
});

const scan = (paths: string[]): WorktreeScan => ({
  repos: paths.map((path) => ({ identity: null, name: path.slice(1), path, worktrees: [] })),
  unreadable: [],
  diagnostics: [],
});

describe("a finished discovery walk is kept after the page is left (#1944)", () => {
  it("unmounting mid-scan does not discard the result", async () => {
    bridge.immediate.set("read_cached_scan", null);
    const { qc, wrapper } = harness();
    const { unmount } = renderHook(() => useWorktrees(), { wrapper });
    await waitFor(() => expect(opened("list_worktrees")).toBe(1));

    unmount();
    await act(async () => answer("list_worktrees", scan(["/a"])));

    await waitFor(() => expect(qc.getQueryData(["worktrees"])).toEqual(scan(["/a"])));
  });
});

describe("the previous scan paints a cold start (#1950)", () => {
  it("exposes the cached scan while discovery is pending, and drops it once the live scan lands", async () => {
    bridge.immediate.set("read_cached_scan", {
      payload: JSON.stringify(scan(["/saved"])),
      age_secs: 7200,
      stale: false,
    });
    const { wrapper } = harness();
    const { result } = renderHook(() => useWorktrees(), { wrapper });

    await waitFor(() => expect(result.current.cached?.data.repos[0].path).toBe("/saved"));
    expect(result.current.cached?.ageSecs).toBe(7200);
    // `data` keeps its pending meaning: the cache is NOT the answer.
    expect(result.current.data).toBeUndefined();
    expect(result.current.isLoading).toBe(true);

    await act(async () => answer("list_worktrees", scan(["/live"])));
    await waitFor(() => expect(result.current.data?.[0].path).toBe("/live"));
    expect(result.current.cached).toBeUndefined();
  });
});
