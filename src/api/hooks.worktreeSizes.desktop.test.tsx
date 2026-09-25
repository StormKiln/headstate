import { describe, expect, it, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

/// The desktop half of #1459's size-call queue: it must not exist.
///
/// The phone queues its size calls because each one dies at the
/// companion's 120s deadline. The desktop's invoke has no deadline and
/// its scan permits already bound the disk, so its fan-out goes out
/// exactly as it always has -- all at once -- and it listens for no
/// connection it does not have.

vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: false, IS_DESKTOP_BUILD: true }));

const seen = vi.hoisted(() => ({ sent: [] as string[], listened: [] as string[] }));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string, args?: { repoPath?: string }) => {
    if (cmd === "size_worktrees") seen.sent.push(args?.repoPath ?? "");
    return new Promise(() => {});
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string) => {
    seen.listened.push(name);
    return Promise.resolve(() => {});
  }),
}));

import { useAllWorktreeSizes } from "./hooks";

describe("worktree sizes on the desktop (#1459)", () => {
  it("sends every repository's size call at once and watches no connection", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );
    const repos = ["/src/a", "/src/b", "/src/c", "/src/d", "/src/e"];
    renderHook(() => useAllWorktreeSizes(repos, true), { wrapper });

    await waitFor(() => expect(seen.sent).toEqual(repos));
    expect(seen.listened).toContain("worktree-size");
    expect(seen.listened).not.toContain("connection-state");
  });
});
