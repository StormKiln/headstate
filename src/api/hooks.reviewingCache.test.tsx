import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { PullRequest } from "../types/pr";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { useReviewing } from "./hooks";

function wrapper(qc: QueryClient) {
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
}

const client = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

const pr = (n: number) => ({ number: n, title: `pr ${n}` }) as unknown as PullRequest;

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

beforeEach(() => invoke.mockReset());

/// The reported complaint: To review "takes more than a minute to load,
/// with no indication that it is blocked", showing an empty panel the
/// whole time.
///
/// The cache still earns its place now that #328 has made the live query
/// fast: painting from disk beats any network round-trip.
///
/// The claim this comment used to carry -- "a bare 25-item search already
/// costs 6.2s", so the query could not be improved -- was a wrong
/// measurement. A bare search is ~0.7s; the cost was `mergeStateStatus`
/// at ~154ms per pull request.
describe("useReviewing paints from cache while the live query runs", () => {
  it("shows the cached list without waiting for GitHub", async () => {
    const live = deferred<PullRequest[]>();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_cached_reviewing")
        return Promise.resolve({ prs: [pr(1), pr(2)], stale_secs: null });
      if (cmd === "get_reviewing") return live.promise;
      return Promise.resolve(undefined);
    });

    const { result } = renderHook(() => useReviewing(true), { wrapper: wrapper(client()) });

    // The cache resolves in milliseconds; the live query is still out.
    await waitFor(() => expect(result.current.data).toHaveLength(2));
    expect(result.current.isLoading).toBe(false);
    expect(result.current.isFromCache).toBe(true);
    expect(result.current.isRefreshing).toBe(true);

    live.resolve([pr(1), pr(2), pr(3)]);
  });

  /// Painting from cache must not mean showing stale data forever.
  it("replaces the cache with live data when it arrives", async () => {
    const live = deferred<PullRequest[]>();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_cached_reviewing")
        return Promise.resolve({ prs: [pr(1)], stale_secs: null });
      if (cmd === "get_reviewing") return live.promise;
      return Promise.resolve(undefined);
    });

    const { result } = renderHook(() => useReviewing(true), { wrapper: wrapper(client()) });
    await waitFor(() => expect(result.current.data).toHaveLength(1));

    live.resolve([pr(1), pr(2), pr(3)]);
    await waitFor(() => expect(result.current.data).toHaveLength(3));
    expect(result.current.isFromCache).toBe(false);
    expect(result.current.isRefreshing).toBe(false);
  });

  /// A cold cache is the first-run case and must still report loading,
  /// or the panel would claim "no open pull requests" before asking.
  it("still reports loading when there is nothing cached", async () => {
    const live = deferred<PullRequest[]>();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_cached_reviewing")
        return Promise.resolve({ prs: [], stale_secs: null });
      if (cmd === "get_reviewing") return live.promise;
      return Promise.resolve(undefined);
    });

    const { result } = renderHook(() => useReviewing(true), { wrapper: wrapper(client()) });
    // An EMPTY cached array is data, so the hook must not present it as
    // a finished answer while the live query is still running.
    await waitFor(() => expect(result.current.isRefreshing).toBe(true));
    expect(result.current.data ?? []).toHaveLength(0);

    live.resolve([pr(7)]);
    await waitFor(() => expect(result.current.data).toHaveLength(1));
  });

  /// #742: the bug the user reported.
  ///
  /// A snapshot past the freshness window used to be discarded by the
  /// Rust side, arriving here as `[]`. Since `[] ?? x` is `[]`, the
  /// empty array beat the loading state and the view rendered a
  /// confident "nothing awaits your review" for as long as the live
  /// fetch took -- seventeen seconds on the account that reported it.
  it("keeps showing a stale snapshot, marked, rather than emptying the list", async () => {
    const live = deferred<PullRequest[]>();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_cached_reviewing")
        // Two hours old: past the hour-long window, so the backend now
        // returns the rows AND says how old they are.
        return Promise.resolve({ prs: [pr(1), pr(2)], stale_secs: 7200 });
      if (cmd === "get_reviewing") return live.promise;
      return Promise.resolve(undefined);
    });

    const { result } = renderHook(() => useReviewing(true), { wrapper: wrapper(client()) });

    // The rows are on screen, not replaced by an empty list.
    await waitFor(() => expect(result.current.data).toHaveLength(2));
    // And the view is told they are old, so it can say so.
    expect(result.current.staleSecs).toBe(7200);
    expect(result.current.isFromCache).toBe(true);

    // Once live data lands the marker clears: it is current by
    // definition, whatever the disk said a moment ago.
    live.resolve([pr(9)]);
    await waitFor(() => expect(result.current.data).toHaveLength(1));
    expect(result.current.staleSecs).toBeNull();
  });

  /// The other direction: a fresh snapshot must NOT be marked, or every
  /// normal paint would carry a warning and the marker would stop
  /// meaning anything.
  it("does not mark a snapshot inside the freshness window", async () => {
    const live = deferred<PullRequest[]>();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_cached_reviewing")
        return Promise.resolve({ prs: [pr(1)], stale_secs: null });
      if (cmd === "get_reviewing") return live.promise;
      return Promise.resolve(undefined);
    });

    const { result } = renderHook(() => useReviewing(true), { wrapper: wrapper(client()) });
    await waitFor(() => expect(result.current.data).toHaveLength(1));
    expect(result.current.staleSecs).toBeNull();
    live.resolve([pr(1)]);
  });

  it("retains the SQLite rows and stale marker when a correlated failure has no receipt", async () => {
    const live = deferred<unknown>();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_cached_reviewing") return Promise.resolve({ prs: [pr(1)], stale_secs: 7200 });
      if (cmd === "get_reviewing") return live.promise;
      return Promise.resolve(undefined);
    });
    const qc = client();
    const { result } = renderHook(() => useReviewing(), { wrapper: wrapper(qc) });
    await waitFor(() => expect(result.current.data?.[0].number).toBe(1));
    live.resolve({ request_id: "request", update: {
      source: { provider: "github", host: "github.com" }, list: "reviewing", session: "desktop", revision: 2,
      receipt_revision: null, prs: null, phase: "failed", error: "GitHub unavailable",
    } });
    await waitFor(() => expect(result.current.isRefreshing).toBe(false));
    expect(result.current.isError).toBe(true);
    expect(String(result.current.error)).toContain("GitHub unavailable");
    expect(result.current.data?.[0].number).toBe(1);
    expect(result.current.isFromCache).toBe(true);
    expect(result.current.staleSecs).toBe(7200);
    expect(qc.getQueryData(["reviewing"])).toBeUndefined();
  });

  it("reads nothing at all while the view is disabled", async () => {
    invoke.mockImplementation(() => Promise.resolve({ prs: [], stale_secs: null }));
    renderHook(() => useReviewing(false), { wrapper: wrapper(client()) });
    expect(invoke).not.toHaveBeenCalled();
  });
});
