import { QueryClient, QueryClientProvider, focusManager } from "@tanstack/react-query";
import { act, cleanup, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { useClaudeUsageProfile } from "./hooks";

const read = vi.hoisted(() => vi.fn());
const events = vi.hoisted(() => ({ handler: undefined as (() => void) | undefined, stop: vi.fn(), listen: vi.fn() }));
vi.mock("./transport", async (original) => ({ ...await original<object>(), listen: (name: string, handler: () => void) => {
  events.listen(name); events.handler = handler; return Promise.resolve(events.stop);
} }));
vi.mock("./tauri", async (original) => ({ ...await original<object>(), claudeUsageProfile: read }));
afterEach(() => { cleanup(); vi.useRealTimers(); focusManager.setFocused(undefined); read.mockReset(); events.listen.mockClear(); events.stop.mockClear(); events.handler = undefined; });

it("shares unfinished polling, stops when complete, and does not poll while hidden", async () => {
  vi.useFakeTimers();
  focusManager.setFocused(true);
  read.mockResolvedValue({ population: { status: "deferred" } });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  const view = renderHook(() => [useClaudeUsageProfile(), useClaudeUsageProfile()], { wrapper });
  await act(async () => { await vi.advanceTimersByTimeAsync(1); });
  expect(read).toHaveBeenCalledTimes(1);
  focusManager.setFocused(false);
  await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
  expect(read).toHaveBeenCalledTimes(1);
  read.mockResolvedValue({ population: { status: "complete" } });
  await act(async () => { focusManager.setFocused(true); await vi.advanceTimersByTimeAsync(1); });
  expect(read).toHaveBeenCalledTimes(2);
  await act(async () => { await vi.advanceTimersByTimeAsync(90_000); });
  expect(read).toHaveBeenCalledTimes(2);
  view.unmount();
  client.clear();
});

it("invalidates complete summaries on a shared receipt and polls running progress", async () => {
  vi.useFakeTimers();
  focusManager.setFocused(true);
  read.mockResolvedValue({ population: { status: "complete" } });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  const view = renderHook(() => [useClaudeUsageProfile(), useClaudeUsageProfile()], { wrapper });
  await act(async () => { await vi.advanceTimersByTimeAsync(1); });
  expect(events.listen).toHaveBeenCalledTimes(1);
  expect(events.listen).toHaveBeenCalledWith("claude-population-updated");
  read.mockResolvedValue({ population: { phase: "usage", status: "running", completed: 2, total: 10 } });
  await act(async () => { events.handler?.(); await vi.advanceTimersByTimeAsync(1); });
  expect(read).toHaveBeenCalledTimes(2);
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  expect(read).toHaveBeenCalledTimes(3);
  focusManager.setFocused(false);
  await act(async () => { events.handler?.(); await vi.advanceTimersByTimeAsync(1_000); });
  expect(read).toHaveBeenCalledTimes(3);
  view.unmount();
  expect(events.stop).toHaveBeenCalledOnce();
  client.clear();
});
