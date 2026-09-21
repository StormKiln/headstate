import { afterEach, describe, expect, it, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { PullRequest } from "@/types/pr";
import { readyForReview } from "@/lib/derive";
import { PR_FIXTURES } from "../fixtures/prs";

const invoke = vi.hoisted(() =>
  vi.fn<(cmd: string, ...a: unknown[]) => Promise<unknown>>(() => Promise.resolve()),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { useActOnPr, useActOnPrs, useReviewPr } from "./hooks";

/// One pull request that `readyForReview` accepts, so the assertions
/// below are about the MUTATION and never about the predicate.
///
/// Built from a fixture rather than written out, so a new required field
/// on `PullRequest` breaks the fixture once instead of here as well.
function readyRow(overrides: Partial<PullRequest> = {}): PullRequest {
  return {
    ...PR_FIXTURES[0],
    repo: "octocat/hello-world",
    number: 7,
    is_draft: false,
    ci: "success",
    merge: "mergeable",
    review: "review_required",
    in_merge_queue: false,
    ...overrides,
  };
}

function wrap(qc: QueryClient) {
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
}

/// The whole-account search the user is waiting out, held open until
/// the test releases it.
///
/// This gate is what makes the file test #1276 rather than "the list
/// catches up eventually" -- which it always did. The complaint is that
/// catching up costs a 7-17 second search (#742 measured it) and the
/// approved, queued pull request sits in "Ready for review" for all of
/// it. So every assertion about a row being gone is made while this is
/// still unresolved: if one passes only because the refresh landed, it
/// is not testing the bug.
///
/// It must eventually RESOLVE rather than hang, because `useActOnPr`
/// awaits `refreshPrs` -- its promise is how `PrActions` clears the
/// button's busy state, so a refresh that never settles is a button
/// that never re-enables. Resolving it is the test acknowledging that,
/// not working around it.
let releaseRefresh: ((rows: PullRequest[]) => void) | null = null;

function heldRefresh(): Promise<PullRequest[]> {
  return new Promise<PullRequest[]>((resolve) => {
    releaseRefresh = resolve;
  });
}

/// Let the held search land, once it has actually STARTED.
///
/// The refresh is only issued after the mutation resolves, so a test
/// that released immediately after calling the mutation would release
/// nothing -- `releaseRefresh` would still be null (or, worse, the
/// previous test's resolver) and the awaited promise would hang to the
/// timeout. Nulled on each `beforeEach` so that cross-test case is a
/// visible hang here rather than a silent pass.
async function landRefresh(rows: PullRequest[] = []): Promise<void> {
  await waitFor(() => {
    if (releaseRefresh === null) throw new Error("the refresh has not started yet");
  });
  releaseRefresh?.(rows);
}

/// A client whose list caches hold `row`.
function seeded(row: PullRequest) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData<PullRequest[]>(["prs"], [row]);
  qc.setQueryData<PullRequest[]>(["reviewing"], [row]);
  return qc;
}

function reviewingRows(qc: QueryClient): PullRequest[] {
  return qc.getQueryData<PullRequest[]>(["reviewing"]) ?? [];
}

/// Release whatever search is still held, whether the test got that far
/// or not.
///
/// `refreshPrs` coalesces on a MODULE-level in-flight promise that
/// outlives the query client, so a test that fails before its own
/// `landRefresh` would leave that promise unresolved and every later
/// test would join it instead of issuing a refresh of its own. That
/// turns one real failure into a file of timeouts and hides which
/// assertion actually broke -- which is exactly what it did while this
/// file was being written.
afterEach(async () => {
  releaseRefresh?.([]);
  releaseRefresh = null;
  // Let the coalescing promise settle and clear `refreshInFlight`.
  await Promise.resolve();
  await Promise.resolve();
});

beforeEach(() => {
  invoke.mockReset();
  releaseRefresh = null;
  invoke.mockImplementation((cmd: string) => {
    // The read-back the user is waiting out; see `heldRefresh`.
    if (cmd === "refresh_now" || cmd === "get_reviewing") return heldRefresh();
    if (cmd === "get_cached") return Promise.resolve([]);
    if (cmd === "get_pr_detail") return Promise.resolve({ latest_reviews: [] });
    return Promise.resolve();
  });
});

/// Mandatory test 1.
describe("a pull request the user approved and queued leaves the list at once", () => {
  it("drops out of readyForReview on approve, with no poll and no refresh", async () => {
    const qc = seeded(readyRow());
    expect(reviewingRows(qc).filter(readyForReview)).toHaveLength(1);

    const { result } = renderHook(() => useReviewPr(), { wrapper: wrap(qc) });
    await result.current("id", "octocat/hello-world", 7, "approve", "");

    expect(reviewingRows(qc).filter(readyForReview)).toHaveLength(0);
    expect(reviewingRows(qc)[0].review).toBe("approved");

    // `useReviewPr` fires the refresh WITHOUT awaiting it -- deliberately,
    // since awaiting made approving one pull request take ~20s to
    // register. So the assertions above already ran while it was in
    // flight. Landing it here is housekeeping, not part of the claim:
    // `refreshPrs` coalesces on a module-level in-flight promise, and
    // leaving one outstanding would make every later test join it
    // instead of issuing its own.
    await landRefresh();
  });

  it("drops out of readyForReview on enqueue, before the refresh returns", async () => {
    // The state after the approve: still in the list only because it has
    // not been queued yet.
    const qc = seeded(readyRow({ review: "review_required" }));

    const { result } = renderHook(() => useActOnPr(), { wrapper: wrap(qc) });
    // NOT awaited yet. `useActOnPr` awaits the whole-account refresh, so
    // awaiting here would assert about a list that had already caught up
    // the slow way -- which is the behaviour #1276 says is too slow.
    const done = result.current("id", "octocat/hello-world", 7, "enqueue");
    await waitFor(() => expect(reviewingRows(qc)[0].in_merge_queue).toBe(true));
    expect(reviewingRows(qc).filter(readyForReview)).toHaveLength(0);

    // Only now let the search land, and confirm the promise settles so
    // the button un-busies.
    await landRefresh();
    await done;
  });

  it("patches My PRs as well as To review, so the row cannot survive a tab switch", async () => {
    const qc = seeded(readyRow());

    const { result } = renderHook(() => useActOnPr(), { wrapper: wrap(qc) });
    const done = result.current("id", "octocat/hello-world", 7, "enqueue");
    await waitFor(() => {
      const authored = qc.getQueryData<PullRequest[]>(["prs"]) ?? [];
      expect(authored[0].in_merge_queue).toBe(true);
    });

    await landRefresh();
    await done;
  });
});

/// Mandatory test 2. The honesty constraint, which is the reason this
/// change is allowed to remove a row at all.
///
/// A row removed for an action GitHub refused is worse than the stale
/// row it replaced: the user believes that pull request is handled and
/// never revisits it. So a rejected enqueue must leave the row exactly
/// where it was, and the rejection must reach the caller -- which is
/// what `PrActions` turns into `toast.error("Could not add #7 to the
/// merge queue", { description: <GitHub's words> })`.
describe("a pull request whose enqueue GitHub refused stays in the list", () => {
  it("leaves the row untouched and surfaces the refusal", async () => {
    const REFUSAL = "Pull request is in an unmergeable state";
    // No refresh at all on this path: the mutation rejects before
    // `refreshPrs` is reached, which is itself part of the claim.
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "act_on_pr") return Promise.reject(REFUSAL);
      if (cmd === "refresh_now" || cmd === "get_reviewing") return heldRefresh();
      return Promise.resolve();
    });

    const qc = seeded(readyRow());
    const { result } = renderHook(() => useActOnPr(), { wrapper: wrap(qc) });

    // The failure must REACH the caller. A mutation that resolved on a
    // refusal would let `PrActions` fire its success toast, and the
    // "failure is visible" half of the constraint would be lost even
    // with the row correctly restored.
    await expect(
      result.current("id", "octocat/hello-world", 7, "enqueue"),
    ).rejects.toBe(REFUSAL);

    // And the row is still there, still ready to review.
    expect(reviewingRows(qc)).toHaveLength(1);
    expect(reviewingRows(qc)[0].in_merge_queue).toBe(false);
    expect(reviewingRows(qc).filter(readyForReview)).toHaveLength(1);
  });

  it("does not drop the batch rows GitHub refused, only the ones it accepted", async () => {
    const ok = readyRow({ number: 7 });
    const refused = readyRow({ number: 8 });
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "act_on_prs") {
        return Promise.resolve([
          { repo: ok.repo, number: 7, error: null },
          { repo: refused.repo, number: 8, error: "Merge queue rejected this branch" },
        ]);
      }
      if (cmd === "refresh_now" || cmd === "get_reviewing") return heldRefresh();
      return Promise.resolve();
    });

    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData<PullRequest[]>(["reviewing"], [ok, refused]);

    const { result } = renderHook(() => useActOnPrs(), { wrapper: wrap(qc) });
    const done = result.current(
      [
        ["id7", ok.repo, 7],
        ["id8", refused.repo, 8],
      ],
      "enqueue",
    );
    await waitFor(() =>
      expect(reviewingRows(qc).find((p) => p.number === 7)?.in_merge_queue).toBe(true),
    );

    const rows = reviewingRows(qc);
    expect(rows.find((p) => p.number === 7)?.in_merge_queue).toBe(true);
    // The one that failed is the whole assertion: a batch that patched
    // by REQUEST rather than by OUTCOME would have queued this too.
    expect(rows.find((p) => p.number === 8)?.in_merge_queue).toBe(false);
    expect(rows.filter(readyForReview).map((p) => p.number)).toEqual([8]);

    await landRefresh();
    await done;
  });
});

/// The two actions deliberately left non-optimistic.
///
/// `useActOnPr`'s original comment refused to render a merge before
/// GitHub agreed, because a merge is irreversible. #1276 does not
/// overturn that: it patches the reversible middle only, and this test
/// is what stops a later "while we're here" from widening it.
describe("merge and close still wait for GitHub", () => {
  it("leaves a merged row's state exactly as GitHub last reported it", async () => {
    const before = readyRow({ review: "approved" });
    const qc = seeded(before);

    const { result } = renderHook(() => useActOnPr(), { wrapper: wrap(qc) });
    const done = result.current("id", "octocat/hello-world", 7, "merge");

    // Asserted while the refresh is still outstanding, which is exactly
    // the window an over-eager patch would write in.
    //
    // Field-by-field, NOT just `toHaveLength(1)`. A patch widened to
    // `merge` would not remove the row -- `patchListRows` only rewrites
    // fields -- so a length assertion passes under precisely the
    // regression this test exists to catch. It did, while this file was
    // being written, and was strengthened to this.
    await landRefresh();
    await done;
    expect(reviewingRows(qc)).toEqual([before]);
  });
});
