import { QueryClient, QueryClientProvider, focusManager } from "@tanstack/react-query";
import { act, cleanup, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "../fixtures/prs";

const target = vi.hoisted(() => ({ mobile: true }));
vi.mock("../lib/target", () => ({ get IS_MOBILE_BUILD() { return target.mobile; } }));
vi.mock("./transport", () => ({ listen: vi.fn(async () => () => {}), call: vi.fn(async () => undefined) }));
vi.mock("./tauri", async (original) => ({
  ...await original<Record<string, unknown>>(),
  getCached: vi.fn(async () => PR_FIXTURES),
  getCachedReviewing: vi.fn(async () => ({ prs: PR_FIXTURES, stale_secs: null })),
  refreshSource: vi.fn(async () => PR_FIXTURES.slice(1)),
}));
import { refreshSource } from "./tauri";
import { usePullRequests, useReviewing } from "./hooks";
import { usePhoneGitHubRefresh, useSourceRefresh } from "./sourceRefreshHooks";

let client: QueryClient;
beforeEach(() => {
  vi.useFakeTimers();
  target.mobile = true;
  vi.mocked(refreshSource).mockReset().mockResolvedValue(PR_FIXTURES.slice(1));
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(["prs"], PR_FIXTURES);
});
afterEach(() => {
  cleanup(); client.clear(); focusManager.setFocused(undefined); vi.useRealTimers();
});
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
const settle = () => act(async () => { await vi.advanceTimersByTimeAsync(1); });

describe("phone source refresh", () => {
  it("refreshes a warm authored cache on selection, cadence and resume without changing desktop preferences", async () => {
    const hook = renderHook(({ enabled }) => {
      usePhoneGitHubRefresh(enabled);
      return usePullRequests(enabled);
    }, { wrapper, initialProps: { enabled: false } });
    await settle();
    expect(refreshSource).not.toHaveBeenCalled();
    hook.rerender({ enabled: true });
    await settle();
    expect(refreshSource).toHaveBeenCalledTimes(1);
    expect(refreshSource).toHaveBeenLastCalledWith("authored", expect.any(String));
    expect(hook.result.current.data).toEqual(PR_FIXTURES.slice(1));
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(refreshSource).toHaveBeenCalledTimes(2);
    act(() => focusManager.setFocused(false));
    act(() => focusManager.setFocused(true));
    await settle();
    expect(refreshSource).toHaveBeenCalledTimes(3);
    hook.rerender({ enabled: false });
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(refreshSource).toHaveBeenCalledTimes(3);
  });

  it("refreshes the active reviewing queue and stops when hidden or GitHub is deselected", async () => {
    const hook = renderHook(({ github, view }) => useReviewing(github && view === "to-review"), {
      wrapper, initialProps: { github: true, view: "my-prs" },
    });
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(refreshSource).not.toHaveBeenCalled();
    hook.rerender({ github: true, view: "to-review" });
    await settle();
    expect(refreshSource).toHaveBeenCalledTimes(1);
    expect(refreshSource).toHaveBeenLastCalledWith("reviewing", expect.any(String));
    expect(hook.result.current.data).toEqual(PR_FIXTURES.slice(1));
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(refreshSource).toHaveBeenCalledTimes(2);
    act(() => focusManager.setFocused(false));
    act(() => focusManager.setFocused(true));
    await settle();
    expect(refreshSource).toHaveBeenCalledTimes(3);
    hook.rerender({ github: true, view: "my-prs" });
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(refreshSource).toHaveBeenCalledTimes(3);
    hook.rerender({ github: false, view: "to-review" });
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(refreshSource).toHaveBeenCalledTimes(3);
  });

  it("retains cached rows and reports a failed request", async () => {
    vi.mocked(refreshSource).mockRejectedValue(new Error("GitHub unavailable"));
    const hook = renderHook(() => {
      usePhoneGitHubRefresh(true);
      return { rows: usePullRequests().data, status: useSourceRefresh("authored") };
    }, { wrapper });
    await settle();
    expect(hook.result.current.rows).toEqual(PR_FIXTURES);
    expect(hook.result.current.status.error).toContain("GitHub unavailable");
  });

  it("leaves the desktop poll loop responsible for desktop refreshes", async () => {
    target.mobile = false;
    renderHook(() => usePhoneGitHubRefresh(true), { wrapper });
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(refreshSource).not.toHaveBeenCalled();
  });
});
