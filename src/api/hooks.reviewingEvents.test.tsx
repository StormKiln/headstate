import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PullRequest } from "../types/pr";

const ipc = vi.hoisted(() => ({
  call: vi.fn(),
  listeners: new Map<string, Set<(event: { payload: unknown }) => void>>(),
}));
vi.mock("./transport", () => ({
  call: ipc.call,
  listen: (name: string, handler: (event: { payload: unknown }) => void) => {
    const handlers = ipc.listeners.get(name) ?? new Set();
    handlers.add(handler);
    ipc.listeners.set(name, handlers);
    return Promise.resolve(() => {
      handlers.delete(handler);
      if (handlers.size === 0) ipc.listeners.delete(name);
    });
  },
}));
import { useActOnPr, usePollError, usePullRequests, useRefreshRequested, useReviewing, useReviewPr, useReviewShortfall, useTruncation } from "./hooks";
import { PR_FIXTURES } from "../fixtures/prs";

const pr = (number: number) => ({ number, title: `PR ${number}` }) as PullRequest;
function wrapper() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}
function emit(name: string, payload: unknown) {
  const listener = ipc.listeners.get(name);
  expect(listener).toBeDefined();
  for (const handler of listener!) handler({ payload });
}
beforeEach(() => {
  ipc.call.mockReset();
  ipc.listeners.clear();
});
afterEach(cleanup);

describe("background reviewing publications", () => {
  it.each(["success", "failure"])("keeps published rows while exposing a foreground %s", async (outcome) => {
    let resolve!: (rows: PullRequest[]) => void;
    let reject!: (error: Error) => void;
    const pending = new Promise<PullRequest[]>((yes, no) => { resolve = yes; reject = no; });
    ipc.call.mockImplementation((command: string) => {
      if (command === "get_reviewing") return pending;
      if (command === "get_cached_reviewing") return Promise.resolve({ prs: [], stale_secs: null });
      return Promise.resolve(undefined);
    });
    const { result } = renderHook(() => ({ queue: useReviewing(), short: useReviewShortfall() }), { wrapper: wrapper() });
    await waitFor(() => expect(ipc.call).toHaveBeenCalledWith("get_reviewing", { requestId: expect.any(String) }));
    act(() => {
      emit("reviewing-short", 10);
      emit("reviewing-updated", [pr(2)]);
    });
    await waitFor(() => expect(result.current.queue.data?.[0].number).toBe(2));
    expect(result.current.short).toBe(10);
    await act(async () => {
      if (outcome === "success") resolve([pr(1)]);
      else reject(new Error("latest foreground request failed"));
      await Promise.resolve();
    });
    await waitFor(() => expect(result.current.queue.isFetching).toBe(false));
    expect(result.current.queue.data?.[0].number).toBe(2);
    expect(result.current.queue.isError).toBe(outcome === "failure");
    if (outcome === "failure") expect(String(result.current.queue.error)).toContain("latest foreground request failed");
    expect(result.current.short).toBe(10);
  });

  it.each(["before", "after"])("keeps the latest reviewing failure when older rows arrive %s rejection", async (order) => {
    let reject!: (error: Error) => void;
    const pending = new Promise<PullRequest[]>((_, no) => { reject = no; });
    ipc.call.mockImplementation((command: string) => command === "get_reviewing"
      ? pending : Promise.resolve({ prs: [], stale_secs: null }));
    const { result } = renderHook(() => useReviewing(), { wrapper: wrapper() });
    await waitFor(() => expect(ipc.call).toHaveBeenCalledWith("get_reviewing", { requestId: expect.any(String) }));
    const failed = { source: { provider: "github", host: "github.com" }, list: "reviewing", phase: "failed", error: "latest foreground failure" };
    if (order === "before") act(() => emit("reviewing-updated", [pr(1)]));
    await act(async () => {
      emit("source-poll-status", failed);
      reject(new Error("latest foreground failure"));
    });
    await waitFor(() => expect(result.current.isFetching).toBe(false));
    if (order === "after") act(() => {
      emit("reviewing-updated", [pr(1)]);
      emit("source-poll-status", failed);
    });
    await waitFor(() => expect(result.current.data?.[0].number).toBe(1));
    expect(result.current.isError).toBe(true);
    expect(result.current.status).toBe("error");
    expect(result.current.isSuccess).toBe(false);
    expect(String(result.current.error)).toContain("latest foreground failure");
    act(() => emit("source-poll-status", { ...failed, list: "authored", phase: "ready", error: null }));
    expect(result.current.isError).toBe(true);
    act(() => {
      emit("reviewing-updated", [pr(2)]);
      emit("source-poll-status", { ...failed, phase: "ready", error: null });
    });
    await waitFor(() => expect(result.current.isError).toBe(false));
    expect(result.current.data?.[0].number).toBe(2);
  });

  it("keeps a newer ready outcome when an older command rejection arrives late", async () => {
    let reject!: (error: Error) => void;
    const pending = new Promise<PullRequest[]>((_, no) => { reject = no; });
    ipc.call.mockImplementation((command: string) => command === "get_reviewing"
      ? pending : Promise.resolve({ prs: [], stale_secs: null }));
    const { result } = renderHook(() => useReviewing(), { wrapper: wrapper() });
    await waitFor(() => expect(ipc.call).toHaveBeenCalledWith("get_reviewing", { requestId: expect.any(String) }));
    const status = { source: { provider: "github", host: "github.com" }, list: "reviewing", phase: "failed", error: "older foreground failure" };
    act(() => emit("source-poll-status", status));
    expect(result.current.isError).toBe(true);
    act(() => {
      emit("reviewing-updated", [pr(2)]);
      emit("source-poll-status", { ...status, phase: "ready", error: null });
    });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    await act(async () => { reject(new Error("older foreground failure")); });
    await waitFor(() => expect(result.current.isFetching).toBe(false));
    expect(result.current.data?.[0].number).toBe(2);
    expect(result.current.error).toBeNull();
    expect(result.current.isError).toBe(false);
    expect(result.current.isSuccess).toBe(true);
    expect(result.current.isPending).toBe(false);
    expect(result.current.status).toBe("success");
  });

  it("observes background recovery while disabled without refetching a fresh query on return", async () => {
    ipc.call.mockImplementation((command: string) => Promise.resolve(
      command === "get_reviewing" ? [pr(1)] : { prs: [], stale_secs: null },
    ));
    const { result, rerender } = renderHook(({ enabled }) => useReviewing(enabled), {
      wrapper: wrapper(), initialProps: { enabled: true },
    });
    await waitFor(() => expect(result.current.data?.[0].number).toBe(1));
    const status = { source: { provider: "github", host: "github.com" }, list: "reviewing", phase: "failed", error: "foreground failure" };
    act(() => emit("source-poll-status", status));
    expect(result.current.isError).toBe(true);
    rerender({ enabled: false });
    act(() => {
      emit("reviewing-updated", [pr(2)]);
      emit("source-poll-status", { ...status, phase: "ready", error: null });
    });
    await waitFor(() => expect(result.current.data?.[0].number).toBe(2));
    expect(result.current.error).toBeNull();
    rerender({ enabled: true });
    expect(result.current.error).toBeNull();
    expect(result.current.status).toBe("success");
    expect(result.current.data?.[0].number).toBe(2);
    expect(ipc.call.mock.calls.filter(([name]) => name === "get_reviewing")).toHaveLength(1);
  });

  it("delivers a later background success without a cache reread and removes listeners on unmount", async () => {
    ipc.call.mockImplementation((command: string) => Promise.resolve(
      command === "get_reviewing" ? [pr(1)] : { prs: [], stale_secs: null },
    ));
    const { result, unmount } = renderHook(() => useReviewing(), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.[0].number).toBe(1));
    act(() => emit("reviewing-updated", [pr(2)]));
    await waitFor(() => expect(result.current.data?.[0].number).toBe(2));
    expect(ipc.call.mock.calls.filter(([name]) => name === "get_cached_reviewing")).toHaveLength(1);
    unmount();
    expect(ipc.listeners.has("reviewing-updated")).toBe(false);
  });
  it("keeps a newer authored failure when an older manual success returns", async () => {
    let resolve!: (rows: PullRequest[]) => void;
    const pending = new Promise<PullRequest[]>((yes) => { resolve = yes; });
    ipc.call.mockImplementation((command: string) => command === "refresh_now" ? pending : Promise.resolve(undefined));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrap = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
    const { result } = renderHook(() => {
      useRefreshRequested();
      return usePollError();
    }, { wrapper: wrap });
    await waitFor(() => expect(ipc.listeners.has("refresh-requested")).toBe(true));
    act(() => emit("refresh-requested", null));
    await waitFor(() => expect(ipc.call).toHaveBeenCalledWith("refresh_now", { requestId: expect.any(String) }));
    const status = { source: { provider: "github", host: "github.com" }, list: "authored", phase: "failed", error: "newer background failure" };
    act(() => emit("source-poll-status", status));
    await waitFor(() => expect(result.current).toBe("newer background failure"));
    act(() => emit("source-poll-status", { ...status, source: { provider: "gitlab", host: "gitlab.com" }, phase: "ready", error: null }));
    expect(result.current).toBe("newer background failure");
    await act(async () => { resolve([pr(1)]); });
    await waitFor(() => expect(client.getQueryData<PullRequest[]>(["prs"])?.[0].number).toBe(1));
    expect(result.current).toBe("newer background failure");
    act(() => emit("source-poll-status", { ...status, phase: "ready", error: null }));
    await waitFor(() => expect(result.current).toBeNull());
  });

  it.each(["success", "failure"])("does not regress authored publications after a delayed correlated %s", async (outcome) => {
    let resolve!: (value: unknown) => void;
    let reject!: (error: Error) => void;
    const pending = new Promise((yes, no) => { resolve = yes; reject = no; });
    ipc.call.mockImplementation((command: string) => command === "refresh_now" ? pending : Promise.resolve(undefined));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrap = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
    const { result } = renderHook(() => { useRefreshRequested(); return usePollError(); }, { wrapper: wrap });
    act(() => emit("refresh-requested", null));
    await waitFor(() => expect(ipc.call).toHaveBeenCalledWith("refresh_now", { requestId: expect.any(String) }));
    const requestId = ipc.call.mock.calls.find(([command]) => command === "refresh_now")![1].requestId;
    const older = { source: { provider: "github", host: "github.com" }, list: "authored", session: "desktop", revision: 2,
      receipt_revision: outcome === "success" ? 2 : null, prs: outcome === "success" ? [pr(1)] : null,
      phase: outcome === "success" ? "ready" : "failed", error: outcome === "success" ? null : "older failure", completed_request: requestId };
    const latestError = outcome === "success" ? "newer provider failure" : null;
    act(() => {
      emit("source-poll-status", older);
      emit("source-poll-status", { ...older, revision: 4, receipt_revision: 3, prs: [pr(3)],
        phase: latestError ? "failed" : "ready", error: latestError, completed_request: "background" });
    });
    await act(async () => {
      if (outcome === "success") resolve({ request_id: requestId, update: older });
      else reject(new Error("older failure"));
    });
    expect(client.getQueryData<PullRequest[]>(["prs"])?.[0].number).toBe(3);
    expect(result.current).toBe(latestError);
  });

  it.each(["before", "after"])("shows phone transport failure when unrelated reviewing recovery arrives %s rejection", async (order) => {
    let reject!: (error: Error) => void;
    const pending = new Promise<PullRequest[]>((_, no) => { reject = no; });
    ipc.call.mockImplementation((command: string) => command === "get_reviewing"
      ? pending : Promise.resolve({ prs: [], stale_secs: null }));
    const { result } = renderHook(() => useReviewing(), { wrapper: wrapper() });
    await waitFor(() => expect(ipc.call).toHaveBeenCalledWith("get_reviewing", { requestId: expect.any(String) }));
    const background = { source: { provider: "github", host: "github.com" }, list: "reviewing", session: "desktop", revision: 2,
      receipt_revision: 2, prs: [pr(2)], phase: "ready", error: null, completed_request: null };
    if (order === "before") act(() => emit("source-poll-status", background));
    await act(async () => { reject(new Error("desktop unreachable")); });
    await waitFor(() => expect(result.current.isFetching).toBe(false));
    if (order === "after") act(() => emit("source-poll-status", background));
    await waitFor(() => expect(result.current.data?.[0].number).toBe(2));
    expect(result.current.isError).toBe(true);
    expect(String(result.current.error)).toContain("desktop unreachable");
  });

  it.each(["enqueue", "draft", "ready", "approve"] as const)("shows confirmed %s in authoritative rows while readback is pending and after it fails", async (action) => {
    const row = { ...PR_FIXTURES[0], repo: "o/r", number: 1, is_draft: action === "ready", in_merge_queue: false, review: "review_required" as const };
    let reject!: (error: Error) => void;
    const pending = new Promise<PullRequest[]>((_, no) => { reject = no; });
    ipc.call.mockImplementation((command: string) => {
      if (command === "get_cached") return Promise.resolve([row]);
      if (command === "refresh_now") return pending;
      return Promise.resolve(undefined);
    });
    const { result } = renderHook(() => ({
      authored: usePullRequests(), reviewing: useReviewing(false), action: useActOnPr(), review: useReviewPr(),
    }), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.authored.data?.[0].number).toBe(1));
    act(() => {
      for (const list of ["authored", "reviewing"]) emit("source-poll-status", {
        source: { provider: "github", host: "github.com" }, list, session: "desktop", revision: 2,
        receipt_revision: 2, prs: [row], phase: "ready", error: null,
      });
    });
    let mutation!: Promise<void>;
    act(() => {
      mutation = action === "approve"
        ? result.current.review("id", "o/r", 1, "approve", "")
        : result.current.action("id", "o/r", 1, action);
    });
    const patch = action === "approve" ? { review: "approved" }
      : action === "enqueue" ? { in_merge_queue: true } : { is_draft: action === "draft" };
    await waitFor(() => expect(result.current.authored.data?.[0]).toMatchObject(patch));
    expect(result.current.reviewing.data?.[0]).toMatchObject(patch);
    await waitFor(() => expect(ipc.call).toHaveBeenCalledWith("refresh_now", { requestId: expect.any(String) }));
    await act(async () => { reject(new Error("readback unavailable")); await mutation; });
    expect(result.current.authored.data?.[0]).toMatchObject(patch);
    expect(result.current.reviewing.data?.[0]).toMatchObject(patch);
  });

  it("adopts the versioned reconnect snapshot without a further poll or command", async () => {
    ipc.call.mockImplementation(() => Promise.resolve([pr(1)]));
    const { result } = renderHook(() => ({ rows: usePullRequests(), error: usePollError() }), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.rows.data?.[0].number).toBe(1));
    const frame = { source: { provider: "github", host: "github.com" }, list: "authored", session: "desktop", revision: 2,
      receipt_revision: 2, prs: [pr(2)], phase: "failed", error: "old failure" };
    act(() => emit("source-poll-status", frame));
    expect(result.current.rows.data?.[0].number).toBe(2);
    act(() => {
      // The SSE reconnect preserves the legacy first frame and then replays
      // the latest versioned source frame, including missed recovery.
      emit("prs-updated", [pr(3)]);
      emit("source-poll-status", { ...frame, revision: 4, receipt_revision: 4, prs: [pr(3)], phase: "ready", error: null });
    });
    expect(result.current.rows.data?.[0].number).toBe(3);
    expect(result.current.error).toBeNull();
    expect(ipc.call.mock.calls.map(([command]) => command)).toEqual(["get_cached"]);
  });

  it.each([
    { name: "partial with a count", coverage: { partial: { total: 8 } }, authored: 8, reviewing: 6 },
    { name: "partial without a count", coverage: { partial: { total: null } }, authored: null, reviewing: null },
    { name: "unknown", coverage: "unknown", authored: null, reviewing: null },
  ])("restores $name completeness on reconnect and clears it only with a newer complete receipt", async ({ coverage, authored, reviewing }) => {
    const { result } = renderHook(() => ({
      authored: useTruncation(), reviewing: useReviewShortfall(), queue: useReviewing(false),
    }), { wrapper: wrapper() });
    act(() => {
      emit("prs-truncated", 99);
      emit("reviewing-short", 99);
    });
    expect(result.current.authored).toBe(99);
    expect(result.current.reviewing).toBe(99);
    const frame = { source: { provider: "github", host: "github.com" }, session: "desktop", revision: 2,
      receipt_revision: 2, prs: [pr(1), pr(2)], phase: "partial", error: null, coverage };
    act(() => {
      // Reconnect replays just source frames; the advisory events were missed.
      for (const list of ["authored", "reviewing"]) emit("source-poll-status", { ...frame, list });
    });
    expect(result.current.queue.data).toHaveLength(2);
    expect(result.current.authored).toBe(authored);
    expect(result.current.reviewing).toBe(reviewing);
    act(() => {
      // Separate legacy advisories and older queued frames cannot unqualify
      // the versioned receipt that is currently on screen.
      emit("prs-truncated", 0);
      emit("reviewing-short", 0);
      for (const list of ["authored", "reviewing"]) emit("source-poll-status", {
        ...frame, list, revision: 1, receipt_revision: 1, coverage: "complete",
      });
    });
    expect(result.current.authored).toBe(authored);
    expect(result.current.reviewing).toBe(reviewing);
    act(() => {
      for (const list of ["authored", "reviewing"]) emit("source-poll-status", {
        ...frame, list, revision: 4, receipt_revision: 4, phase: "ready", coverage: "complete",
      });
    });
    expect(result.current.authored).toBe(0);
    expect(result.current.reviewing).toBe(0);
    act(() => {
      for (const list of ["authored", "reviewing"]) emit("source-poll-status", { ...frame, list });
    });
    expect(result.current.authored).toBe(0);
    expect(result.current.reviewing).toBe(0);
    expect(ipc.call).not.toHaveBeenCalled();
  });

  it("replaces earlier numeric advice with unknown for each new list", async () => {
    const { result } = renderHook(() => ({ authored: useTruncation(), reviewing: useReviewShortfall() }), { wrapper: wrapper() });
    await waitFor(() => expect(ipc.listeners.has("reviewing-short")).toBe(true));
    act(() => {
      emit("prs-truncated", 20);
      emit("reviewing-short", 10);
    });
    await waitFor(() => expect(result.current).toEqual({ authored: 20, reviewing: 10 }));
    act(() => {
      emit("prs-truncated", null);
      emit("reviewing-short", null);
    });
    await waitFor(() => expect(result.current).toEqual({ authored: null, reviewing: null }));
  });

});
