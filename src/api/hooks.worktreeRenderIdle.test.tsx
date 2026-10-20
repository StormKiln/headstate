import { it, expect, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: false, IS_DESKTOP_BUILD: true }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => new Promise(() => {})) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
import { useAllWorktreeSizes } from "./hooks";

it.each([false, true])("disabled repository observers settle after parent renders (partial cache=%s)", async (partial) => {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) =>
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const repos = Array.from({ length: 50 }, (_, i) => ({
    path: `/fixture/repo-${i}`,
    worktrees: [{ path: `/fixture/repo-${i}` }, { path: `/fixture/repo-${i}/linked` }],
  }));
  if (partial) {
    for (const repo of repos) {
      qc.setQueryData(["worktree-sizes", repo.path, [repo.path]], new Map([[repo.path, null]]));
    }
  }
  let renders = 0;
  const hook = renderHook(() => {
    renders++;
    return useAllWorktreeSizes(repos.map(r => r.path), false, repos);
  }, { wrapper });
  try {
    await act(async () => { await new Promise(r => setTimeout(r, 150)); });
    const baseline = renders;
    // Parent navigation renders must not restart placeholder notifications.
    hook.rerender();
    for (let i = 0; i < 10; i++) {
      await act(async () => { await new Promise(r => setTimeout(r, 10)); });
    }
    if (partial) expect(hook.result.current.sizes.get(repos[0].path)).toBeNull();
    expect(renders - baseline).toBeLessThanOrEqual(1);
  } finally {
    hook.unmount();
    qc.clear();
  }
});
