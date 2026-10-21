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
import { useMergeStack, useReviewing, useReviewingCount, useReviewShortfall } from "./hooks";
import { retireSourceOwnership } from "./sourceRefreshHooks";
import { PR_FIXTURES } from "../fixtures/prs";

const pr = (number: number) => ({ ...PR_FIXTURES[0], repo: "synthetic/project", number, title: `PR ${number}` }) as PullRequest;
function setup(retry: number | false = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry, retryDelay: 0 } } });
  client.setQueryData(["viewer"], "fixture");
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return { client, wrapper };
}
function emit(name: string, payload: unknown) {
  const listener = ipc.listeners.get(name);
  expect(listener).toBeDefined();
  for (const handler of listener!) handler({ payload });
}
const calls = (command: string) => ipc.call.mock.calls.filter(([name]) => name === command).length;
const frame = (revision: number, prs: PullRequest[]) => ({
  source: { provider: "github", host: "github.com" }, list: "reviewing", session: "desktop",
  revision, receipt_revision: revision, prs, phase: "ready", error: null, coverage: "complete",
});
beforeEach(() => {
  ipc.call.mockReset();
  ipc.listeners.clear();
});
afterEach(cleanup);

/// #1929: a live rejection that never reached the source receipt left the
/// view with no rows, no error and no loading state -- and `reviewing = []`
/// rendered as "No open pull requests".
describe("a failed review refresh with nothing cached", () => {
  it("is an error, not an empty queue", async () => {
    let answer!: (rows: PullRequest[]) => void;
    ipc.call.mockImplementation((command: string) => {
      if (command === "get_reviewing") return new Promise(resolve => { answer = resolve; });
      if (command === "get_source_snapshot") return Promise.reject(new Error("no saved snapshot"));
      return Promise.resolve(undefined);
    });
    const { client, wrapper } = setup();
    const { result } = renderHook(() => useReviewing(), { wrapper });
    await waitFor(() => expect(calls("get_reviewing")).toBe(1));
    await act(async () => {
      retireSourceOwnership(client);
      answer([]);
      await Promise.resolve();
    });
    await waitFor(() => expect(result.current.isFetching).toBe(false));
    expect(result.current.data).toBeUndefined();
    expect(result.current.isError).toBe(true);
    expect(result.current.status).toBe("error");
    expect(result.current.isLoading).toBe(false);
    expect(String(result.current.error)).toContain("The paired desktop or account changed.");
  });

  it("strips the not-asked marker from the live error", async () => {
    ipc.call.mockImplementation((command: string) => {
      if (command === "get_reviewing") return Promise.reject(new Error("headstate:not-asked GitHub sign-in is required"));
      if (command === "get_source_snapshot") return Promise.reject(new Error("no saved snapshot"));
      return Promise.resolve(undefined);
    });
    const { wrapper } = setup();
    const { result } = renderHook(() => useReviewing(), { wrapper });
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.error?.message).toBe("GitHub sign-in is required");
  });

  it("does not multiply the backend's own bounded attempts", async () => {
    ipc.call.mockImplementation((command: string) => {
      if (command === "get_reviewing") return Promise.reject(new Error("offline"));
      if (command === "get_source_snapshot") return Promise.reject(new Error("no saved snapshot"));
      return Promise.resolve(undefined);
    });
    // A client whose default WOULD retry immediately: the query must opt out.
    const { wrapper } = setup(2);
    const { result } = renderHook(() => useReviewing(), { wrapper });
    await waitFor(() => expect(result.current.isFetching).toBe(false));
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(calls("get_reviewing")).toBe(1);
  });
});

/// #1930: nothing invalidated `["reviewing-count"]`, so the sidebar badge
/// kept its old number beside a list that had already moved.
describe("the review badge follows the review list", () => {
  it("re-counts when the accepted review rows change, and only then", async () => {
    let count = 3;
    ipc.call.mockImplementation((command: string) => command === "count_reviewing"
      ? Promise.resolve(count) : Promise.resolve(undefined));
    const { wrapper } = setup();
    const { result } = renderHook(() => useReviewingCount(), { wrapper });
    await waitFor(() => expect(result.current.data).toBe(3));
    await waitFor(() => expect(ipc.listeners.has("source-poll-status")).toBe(true));
    act(() => emit("source-poll-status", frame(2, [pr(1), pr(2), pr(3)])));
    await waitFor(() => expect(calls("count_reviewing")).toBe(2));
    count = 2;
    act(() => emit("source-poll-status", frame(3, [pr(1), pr(2)])));
    await waitFor(() => expect(result.current.data).toBe(2));
    const settled = calls("count_reviewing");
    // An unchanged poll keeps row identity and costs no count.
    act(() => emit("source-poll-status", frame(4, [pr(1), pr(2)])));
    await act(async () => { await Promise.resolve(); });
    expect(calls("count_reviewing")).toBe(settled);
  });

  it("re-counts wherever the review list is invalidated", async () => {
    let count = 5;
    ipc.call.mockImplementation((command: string) => {
      if (command === "count_reviewing") return Promise.resolve(count);
      if (command === "merge_stack") return Promise.resolve({ outcome: "merged" });
      return Promise.resolve(undefined);
    });
    const { wrapper } = setup();
    const { result } = renderHook(() => ({ count: useReviewingCount(), merge: useMergeStack() }), { wrapper });
    await waitFor(() => expect(result.current.count.data).toBe(5));
    count = 4;
    await act(async () => { await result.current.merge("o/r", 1, "direct_merge", "abc"); });
    await waitFor(() => expect(result.current.count.data).toBe(4));
  });
});

/// #1964: the shortfall started at 0 -- "nothing missing" -- so an unchecked
/// list read as a confirmed census until the first event.
describe("the review shortfall before anything reports", () => {
  it("is unchecked, not complete", async () => {
    ipc.call.mockImplementation(() => Promise.resolve(undefined));
    const { wrapper } = setup();
    const { result } = renderHook(() => useReviewShortfall(), { wrapper });
    await waitFor(() => expect(ipc.listeners.has("reviewing-short")).toBe(true));
    expect(result.current).toBeUndefined();
    act(() => emit("reviewing-short", 0));
    expect(result.current).toBe(0);
  });
});
