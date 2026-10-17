import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { useClaudeSessions } from "./hooks";

const api = vi.hoisted(() => ({ imported: vi.fn(), sessions: vi.fn(), cancel: vi.fn() }));
vi.mock("./tauri", async (original) => ({ ...await original<object>(),
  claudeImportTranscripts: api.imported, claudeSessions: api.sessions, claudeCancelPopulation: api.cancel,
}));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

it("keeps page mounts automatic and reserves resume intent for the explicit rescan", async () => {
  const paused = { population: { status: "cancelled" } };
  const complete = { population: { status: "complete" } };
  api.imported.mockResolvedValue(paused);
  api.sessions.mockResolvedValue({ sessions: [], reasons: [], unreadable: [] });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  const view = renderHook(() => useClaudeSessions(true), { wrapper });
  await waitFor(() => expect(view.result.current.imported.isSuccess).toBe(true));
  expect(api.imported).toHaveBeenCalledWith(false);
  expect(api.imported).toHaveBeenCalledTimes(1);
  api.imported.mockResolvedValue(complete);
  await act(async () => { await view.result.current.rescan(); });
  expect(api.imported).toHaveBeenLastCalledWith(true);
  expect(api.imported).toHaveBeenCalledTimes(2);
  expect(client.getQueryData(["claude-import"])).toEqual(complete);
  view.unmount(); client.clear();
});


it("exposes explicit rescan pending and failure without losing stored sessions or duplicating requests", async () => {
  api.imported.mockResolvedValue({ population: { status: "cancelled" } });
  api.sessions.mockResolvedValue({ sessions: [], reasons: [], unreadable: [] });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  const view = renderHook(() => useClaudeSessions(true), { wrapper });
  await waitFor(() => expect(view.result.current.imported.isSuccess).toBe(true));
  const stored = view.result.current.list.data;
  let reject!: (error: Error) => void;
  api.imported.mockImplementation(() => new Promise((_, fail) => { reject = fail; }));
  let first!: Promise<void>;
  let second!: Promise<void>;
  act(() => { first = view.result.current.rescan(); second = view.result.current.rescan(); });
  await waitFor(() => expect(view.result.current.imported.isFetching).toBe(true));
  expect(api.imported).toHaveBeenCalledTimes(2);
  await act(async () => { reject(new Error("synthetic scan failure")); await Promise.all([first, second]); });
  await waitFor(() => expect(view.result.current.imported.isError).toBe(true));
  expect(view.result.current.imported.error?.message).toBe("synthetic scan failure");
  expect(view.result.current.imported.isFetching).toBe(false);
  expect(view.result.current.list.data).toBe(stored);
  view.unmount(); client.clear();
});
