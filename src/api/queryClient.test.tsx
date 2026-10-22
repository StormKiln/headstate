import { describe, expect, it, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

const invoke = vi.hoisted(() => vi.fn<(cmd: string, args?: unknown) => Promise<unknown>>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { createQueryClient } from "./queryClient";
import { useWorktrees } from "./hooks";

/// A bare `new QueryClient()` gave every query TanStack's `retry: 3`
/// (#1955), so a refused discovery walk ran four times, with about seven
/// seconds of backoff, before the error panel showed. #846 had already
/// decided walks must not retry; the default quietly undid that for every
/// hook that did not opt out.

/// Every production `.ts`/`.tsx` under `src/`, by path. Tests are skipped:
/// they build their own clients, often with deliberate seeds, and this
/// file's own prose names the pattern it forbids.
const sources = import.meta.glob(["../**/*.ts", "../**/*.tsx"], {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

describe("the app's query client (#1955)", () => {
  it("does not retry a query unless the query asks to", () => {
    expect(createQueryClient().getDefaultOptions().queries?.retry).toBe(false);
  });

  it("is the client main.tsx mounts, and no production file builds a bare one", () => {
    const main = sources["../main.tsx"];
    expect(main).toBeDefined();
    expect(main).toMatch(/createQueryClient\(\)/);
    const bare = Object.entries(sources)
      .filter(([path]) => !/\.test\.tsx?$/.test(path) && path !== "./queryClient.ts")
      .filter(([, text]) => /new QueryClient\(\s*\)/.test(text))
      .map(([path]) => path);
    expect(bare).toEqual([]);
  });

  it("surfaces a refused discovery walk after one attempt, not four", async () => {
    invoke.mockImplementation((cmd) => {
      if (cmd === "list_worktrees") return Promise.reject(new Error("could not read the discovery exclusions"));
      if (cmd === "read_cached_scan") return Promise.resolve(null);
      return Promise.resolve([]);
    });
    const qc = createQueryClient();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => useWorktrees(), { wrapper });
    // Well inside TanStack's first backoff (1 s): with retries on, the
    // query is still loading here and the error has not been shown.
    await waitFor(() => expect(result.current.isError).toBe(true), { timeout: 500 });
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "list_worktrees")).toHaveLength(1);
    qc.clear();
  });
});
