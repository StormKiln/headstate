import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { PrDetail } from "@/types/pr";

/// The detail response GitHub returns during its read-side lag: the
/// approval succeeded, and `latestReviews` does not show it yet.
const STALE_DETAIL = { id: "id", repo: "o/r", number: 7, head_oid: "head", latest_reviews: [] } as unknown as PrDetail;

const ACK = { outcome: "acknowledged", receipt: { review_id: "review", state: "APPROVED", actor: "me", commit_oid: "head", submitted_at: "2026-10-01T00:00:00Z", pr_id: "id", repo: "o/r", number: 7 } };
const invoke = vi.hoisted(() =>
  vi.fn<(cmd: string, ...a: unknown[]) => Promise<unknown>>(() => Promise.resolve()),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { usePrDetail, useReviewPr } from "./hooks";

/// #440: after approving, the button stayed on "Approve" until you left
/// the PR and came back.
///
/// `useReviewPr` deliberately seeds the cache with the verdict it knows
/// landed, because `latestReviews` lags `addPullRequestReview` by a
/// second or two. Then it immediately awaited a refetch of THE SAME KEY
/// -- replacing the correct answer with GitHub's stale one, inside the
/// exact window the seed exists to cover.
describe("useReviewPr", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "review_pr_at_head") return Promise.resolve({ outcome: "acknowledged", receipt: { review_id: "review", state: "APPROVED", actor: "me", commit_oid: "head", submitted_at: "2026-10-01T00:00:00Z", pr_id: "id", repo: "o/r", number: 7 } });
      if (cmd === "get_pr_detail") return Promise.resolve(STALE_DETAIL);
      return Promise.resolve();
    });
  });

  it("keeps the approval visible while GitHub's read side lags", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["viewer"], "me");
    qc.setQueryData<PrDetail>(["pr-detail", "o/r", 7], STALE_DETAIL);

    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );
    // A MOUNTED observer of the detail query, which is what makes
    // `refetchQueries` actually refetch: it only refreshes ACTIVE
    // queries, so without a component watching the key the refetch is a
    // no-op and the bug cannot reproduce.
    const { result } = renderHook(
      () => ({ review: useReviewPr(), detail: usePrDetail("o/r", 7) }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.detail.data).toBeDefined());
    await result.current.review("id", "o/r", 7, "approve", "");

    const after = qc.getQueryData<PrDetail>(["pr-detail", "o/r", 7]);
    expect(after?.latest_reviews).toContainEqual(expect.objectContaining({ author: "me", state: "APPROVED" }));
  });

  /// The seed must not become a lie that outlives the truth: once
  /// GitHub reports the review, the normal fetch path replaces it.
  it("lets a later fetch replace the seeded verdict", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["viewer"], "me");
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );

    const { result } = renderHook(
      () => ({ review: useReviewPr(), detail: usePrDetail("o/r", 7) }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.detail.data).toBeDefined());
    await result.current.review("id", "o/r", 7, "approve", "");

    // GitHub has caught up and reports a DIFFERENT state.
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "review_pr_at_head") return Promise.resolve({ outcome: "acknowledged", receipt: { review_id: "review", state: "APPROVED", actor: "me", commit_oid: "head", submitted_at: "2026-10-01T00:00:00Z", pr_id: "id", repo: "o/r", number: 7 } });
      if (cmd === "get_pr_detail") {
        return Promise.resolve({
          ...STALE_DETAIL,
          latest_reviews: [{ author: "me", state: "CHANGES_REQUESTED", id: "newer", commit_oid: "head", submitted_at: "2026-10-01T00:01:00Z" }],
        } as unknown as PrDetail);
      }
      return Promise.resolve();
    });
    await qc.refetchQueries({ queryKey: ["pr-detail", "o/r", 7] });

    const after = qc.getQueryData<PrDetail>(["pr-detail", "o/r", 7]);
    expect(after?.latest_reviews).toEqual([expect.objectContaining({ author: "me", state: "CHANGES_REQUESTED" })]);
  });
});

/// #699: approving is exactly what makes a pull request mergeable, or
/// makes auto-merge enqueue it -- so the merge fields are stale the
/// instant the review lands, and nothing was re-reading them.
///
/// `usePrDetail` polls while `merge_status` is `unknown`, but that
/// could never fix this: the stale value is the PRE-approval verdict
/// (`blocked`), which looks like a settled answer rather than a
/// transient one.
describe("useReviewPr and the merge buttons", () => {
  /// GitHub after the approval: mergeable, and auto-merge has queued it.
  const FRESH = {
    ...STALE_DETAIL,
    latest_reviews: [],
    merge_status: "clean",
    merge_queue_enabled: true,
    in_merge_queue: true,
  } as unknown as PrDetail;

  /// The cache before it: the pre-approval verdict.
  const BEFORE = {
    ...STALE_DETAIL,
    latest_reviews: [],
    merge_status: "blocked",
    merge_queue_enabled: true,
    in_merge_queue: false,
  } as unknown as PrDetail;

  beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "review_pr_at_head") return Promise.resolve({ outcome: "acknowledged", receipt: { review_id: "review", state: "APPROVED", actor: "me", commit_oid: "head", submitted_at: "2026-10-01T00:00:00Z", pr_id: "id", repo: "o/r", number: 7 } });
      if (cmd === "get_pr_detail") return Promise.resolve(FRESH);
      return Promise.resolve();
    });
  });

  const wrap = (qc: QueryClient) =>
    ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );

  /// The reported symptom: "Add to merge queue" offered for a pull
  /// request GitHub had already queued, and clicking it errored with
  /// "already in the merge queue".
  it("re-reads whether the pull request is now queued", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["viewer"], "me");
    qc.setQueryData<PrDetail>(["pr-detail", "o/r", 7], BEFORE);

    const { result } = renderHook(() => useReviewPr(), { wrapper: wrap(qc) });
    await result.current("id", "o/r", 7, "approve", "");

    await waitFor(() => expect(qc.getQueryData<PrDetail>(["pr-detail", "o/r", 7])?.in_merge_queue).toBe(true));
  });

  /// The other reported symptom, on a repository with no merge queue:
  /// the Merge button never activated, and the work had to be finished
  /// on github.com.
  it("re-reads mergeability so the merge button can activate", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["viewer"], "me");
    qc.setQueryData<PrDetail>(["pr-detail", "o/r", 7], BEFORE);

    const { result } = renderHook(() => useReviewPr(), { wrapper: wrap(qc) });
    await result.current("id", "o/r", 7, "approve", "");

    await waitFor(() => expect(qc.getQueryData<PrDetail>(["pr-detail", "o/r", 7])?.merge_status).toBe("clean"));
  });

  /// The trap this fix had to avoid: the merge fields and the review
  /// verdict lag in OPPOSITE directions. `latestReviews` lags behind
  /// the approval, so re-reading it reverts the button; mergeability
  /// lags ahead of the cache, so NOT re-reading it strands the button.
  /// Reconciliation must preserve the verified receipt in the full response.
  it("does not let the re-read undo the seeded verdict", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["viewer"], "me");
    qc.setQueryData<PrDetail>(["pr-detail", "o/r", 7], BEFORE);

    const { result } = renderHook(() => useReviewPr(), { wrapper: wrap(qc) });
    await result.current("id", "o/r", 7, "approve", "");

    const after = qc.getQueryData<PrDetail>(["pr-detail", "o/r", 7]);
    expect(
      after?.latest_reviews,
      "the merge re-read overwrote the verdict, which is #440 all over again",
    ).toContainEqual(expect.objectContaining({ author: "me", state: "APPROVED" }));
  });

  it("does not let an older detail query erase a verified approval", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["viewer"], "me");
    qc.setQueryData<PrDetail>(["pr-detail", "o/r", 7], BEFORE);
    let finishOld!: (value: PrDetail) => void;
    let finishNew!: (value: PrDetail) => void;
    const oldRead = new Promise<PrDetail>(resolve => { finishOld = resolve; });
    const newRead = new Promise<PrDetail>(resolve => { finishNew = resolve; });
    let reads = 0;
    invoke.mockImplementation((cmd: string) => cmd === "get_pr_detail"
      ? (++reads === 1 ? oldRead : newRead) : Promise.resolve(ACK));
    const { result, unmount } = renderHook(() => ({ detail: usePrDetail("o/r", 7), review: useReviewPr() }), { wrapper: wrap(qc) });
    const older = result.current.detail.refetch();
    await waitFor(() => expect(reads).toBe(1));
    await result.current.review("id", "o/r", 7, "approve", "");
    finishOld(BEFORE);
    await older;
    try {
      expect(qc.getQueryData<PrDetail>(["pr-detail", "o/r", 7])).toMatchObject({
        latest_reviews: [{ author: "me", state: "APPROVED" }], merge_status: "unknown",
      });
    } finally { finishNew(FRESH); unmount(); qc.clear(); }
  });

  it("finishes a verified approval before the merge-field read returns", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["viewer"], "me");
    qc.setQueryData<PrDetail>(["pr-detail", "o/r", 7], BEFORE);
    let finish!: (value: PrDetail) => void;
    const pending = new Promise<PrDetail>((resolve) => { finish = resolve; });
    invoke.mockImplementation((cmd: string) => cmd === "get_pr_detail" ? pending : Promise.resolve(ACK));
    const { result, unmount } = renderHook(() => useReviewPr(), { wrapper: wrap(qc) });
    let completed = false;
    const submitted = result.current("id", "o/r", 7, "approve", "").then(() => { completed = true; });
    try {
      await waitFor(() => expect(completed).toBe(true));
      expect(qc.getQueryData<PrDetail>(["pr-detail", "o/r", 7])).toMatchObject({
        latest_reviews: [{ author: "me", state: "APPROVED" }], merge_status: "unknown",
      });
    } finally {
      finish(FRESH);
      await submitted;
      unmount();
    }
    await waitFor(() => expect(qc.getQueryData<PrDetail>(["pr-detail", "o/r", 7])?.merge_status).toBe("clean"));
    qc.clear();
  });

  /// The review already succeeded. A failed follow-up read must not
  /// report it as failed -- the poll loop catches up regardless.
  it("does not fail the review when the follow-up read fails", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["viewer"], "me");
    qc.setQueryData<PrDetail>(["pr-detail", "o/r", 7], BEFORE);
    invoke.mockImplementation((cmd: string) =>
      cmd === "get_pr_detail" ? Promise.reject(new Error("offline")) : Promise.resolve(ACK),
    );

    const { result } = renderHook(() => useReviewPr(), { wrapper: wrap(qc) });
    await expect(result.current("id", "o/r", 7, "approve", "")).resolves.toMatchObject({ state: "APPROVED" });
    // The seed still landed, so the approval is still visible.
    expect(
      qc.getQueryData<PrDetail>(["pr-detail", "o/r", 7])?.latest_reviews,
    ).toContainEqual(expect.objectContaining({ author: "me", state: "APPROVED" }));
  });
});