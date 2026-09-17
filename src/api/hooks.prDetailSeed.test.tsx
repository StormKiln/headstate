import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { PrDetail, PullRequest } from "@/types/pr";

const invoke = vi.hoisted(() =>
  vi.fn<(cmd: string, ...a: unknown[]) => Promise<unknown>>(() => Promise.resolve()),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { usePrDetail } from "./hooks";

/// The clicked list row: every field the detail view can show before a
/// single request has been made.
const ROW: PullRequest = {
  id: "PR_row",
  number: 7,
  title: "Cap the check pagination",
  url: "https://github.com/o/r/pull/7",
  repo: "o/r",
  author: "octocat",
  is_draft: false,
  head_ref: "perf/pr-detail-load",
  head_oid: "deadbeef",
  head_ref_id: "REF_1",
  base_ref: "main",
  created_at: "2026-09-01T00:00:00Z",
  updated_at: "2026-09-01T00:00:00Z",
  ci: "failure",
  merge: "mergeable",
  merge_status: "blocked",
  review: "changes_requested",
  in_merge_queue: false,
  labels: [],
  comment_count: 4,
  unresolved_threads: 2,
  requested_reviewers: [],
  assignees: [],
  latest_reviews: [{ author: "hubot", state: "CHANGES_REQUESTED" }],
  requested_reviewers_total: 0,
  assignees_total: 0,
  latest_reviews_total: 1,
  labels_total: 0,
};

/// What the command eventually answers with.
const DETAIL = {
  id: "PR_row",
  number: 7,
  title: "Cap the check pagination",
  body: "## Why\n\n21 serial POSTs.",
  additions: 120,
  deletions: 9,
  changed_files: 4,
  checks: [{ name: "build", state: "success", url: "", run_id: null }],
  checks_total: 1,
} as unknown as PrDetail;

/// A never-resolving command, so the PLACEHOLDER window can be asserted
/// on at all. The bug in #790 is entirely about what is on screen while
/// this promise is outstanding; a fetch that resolves immediately closes
/// the window the test exists to inspect.
function hanging(): Promise<never> {
  return new Promise<never>(() => {});
}

function wrap(qc: QueryClient) {
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
}

/// #790: clicking a pull request showed nothing but "Loading pull
/// request…" for up to 30 seconds, even though the row just clicked
/// already held the title, number, author, branch pair and review state.
describe("usePrDetail seeding", () => {
  beforeEach(() => {
    invoke.mockReset();
  });

  it("renders the clicked row's facts before the fetch resolves", async () => {
    invoke.mockImplementation(() => hanging());
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData<PullRequest[]>(["prs"], [ROW]);

    const { result } = renderHook(() => usePrDetail("o/r", 7), { wrapper: wrap(qc) });

    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(result.current.isPlaceholderData).toBe(true);
    // The view reads exactly these to paint its header.
    expect(result.current.data?.title).toBe("Cap the check pagination");
    expect(result.current.data?.author).toBe("octocat");
    expect(result.current.data?.head_ref).toBe("perf/pr-detail-load");
    expect(result.current.data?.base_ref).toBe("main");
    expect(result.current.data?.merge_status).toBe("blocked");
    expect(result.current.data?.latest_reviews).toEqual([
      { author: "hubot", state: "CHANGES_REQUESTED" },
    ]);
    // `isLoading` false is what makes the real view render instead of
    // the spinner. If this ever goes back to true the seed is inert.
    expect(result.current.isLoading).toBe(false);
  });

  /// The seed must not INVENT the parts the row does not carry. A body,
  /// a comment list or a check list filled in from nothing would be
  /// indistinguishable from a pull request that genuinely has none.
  it("leaves what the row cannot know empty rather than guessing", async () => {
    invoke.mockImplementation(() => hanging());
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData<PullRequest[]>(["prs"], [ROW]);

    const { result } = renderHook(() => usePrDetail("o/r", 7), { wrapper: wrap(qc) });
    await waitFor(() => expect(result.current.data).toBeDefined());

    expect(result.current.data?.body).toBe("");
    expect(result.current.data?.comments).toEqual([]);
    expect(result.current.data?.review_threads).toEqual([]);
    expect(result.current.data?.checks).toEqual([]);
    // Zero against an empty list, so the Checks panel cannot claim
    // "showing 0 of N" on data nobody fetched.
    expect(result.current.data?.checks_total).toBe(0);
    // Same reasoning one section up (#802). The row DOES carry
    // `unresolved_threads`, which makes a non-zero value here tempting --
    // but that counts open threads, not the connection total, so it would
    // annotate an empty list with a shortfall it cannot support.
    expect(result.current.data?.review_threads_total).toBe(0);
    // The list query does not select the diff size, so this is genuinely
    // unknown. The view suppresses the line at zero rather than printing
    // "+0 −0 across 0 files".
    expect(result.current.data?.changed_files).toBe(0);
  });

  /// To review is the other way into this view, and it is a different
  /// cache key. Seeding only `["prs"]` would leave every review-queue
  /// click on the old spinner.
  it("seeds from the review queue as well as My PRs", async () => {
    invoke.mockImplementation(() => hanging());
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData<PullRequest[]>(["reviewing"], [ROW]);

    const { result } = renderHook(() => usePrDetail("o/r", 7), { wrapper: wrap(qc) });
    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(result.current.data?.title).toBe("Cap the check pagination");
  });

  /// The path with nothing to seed from -- a cold launch straight into a
  /// detail view, or a pull request in neither list. It must still work,
  /// and it must still show the spinner rather than a blank page.
  it("falls back to the plain loading state with no cached row", async () => {
    invoke.mockImplementation(() => hanging());
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });

    const { result } = renderHook(() => usePrDetail("o/r", 7), { wrapper: wrap(qc) });
    await waitFor(() => expect(result.current.isLoading).toBe(true));
    expect(result.current.data).toBeUndefined();
  });

  /// `placeholderData` and not `initialData`: the seed must never enter
  /// the cache, because `staleTime: 30_000` would then suppress the real
  /// fetch and the body would never arrive.
  it("does not write the seed into the cache, and still fetches", async () => {
    invoke.mockImplementation(() => hanging());
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData<PullRequest[]>(["prs"], [ROW]);

    const { result } = renderHook(() => usePrDetail("o/r", 7), { wrapper: wrap(qc) });
    await waitFor(() => expect(result.current.data).toBeDefined());

    expect(qc.getQueryData(["pr-detail", "o/r", 7])).toBeUndefined();
    expect(invoke).toHaveBeenCalledWith("get_pr_detail", { repo: "o/r", number: 7 });
  });

  /// And the real answer replaces the seed, rather than the seed
  /// sticking as a permanently body-less page.
  it("replaces the seed with the fetched detail", async () => {
    invoke.mockImplementation((cmd: string) =>
      cmd === "get_pr_detail" ? Promise.resolve(DETAIL) : Promise.resolve(),
    );
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData<PullRequest[]>(["prs"], [ROW]);

    const { result } = renderHook(() => usePrDetail("o/r", 7), { wrapper: wrap(qc) });
    await waitFor(() => expect(result.current.isPlaceholderData).toBe(false));
    expect(result.current.data?.body).toContain("21 serial POSTs");
    expect(result.current.data?.changed_files).toBe(4);
  });
});
