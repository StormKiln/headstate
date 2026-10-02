import { QueryClient } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import type { PrDetail } from "../types/pr";
import { reviewPrAtHead, type BoundReviewRequest, type SubmittedReview } from "./tauri";
import { reconcileReviewDetail, reviewOperation, submitBoundReview, REVIEW_ACK_MS, REVIEW_OPERATION_CAP, releaseCheckedReview } from "./reviewOperations";
vi.mock("./tauri", () => ({ reviewPrAtHead: vi.fn() }));
afterEach(() => { vi.clearAllMocks(); vi.useRealTimers(); });
const request: BoundReviewRequest = { id: "PR-1", repo: "fixture/project", number: 1, verdict: "approve", body: "", expected_head: "head-1", expected_viewer: "fixture" };
const receipt: SubmittedReview = { review_id: "REVIEW-1", pr_id: request.id, repo: request.repo, number: 1, state: "APPROVED", actor: "fixture", commit_oid: "head-1", submitted_at: "2026-10-01T00:00:00Z" };
function setup() { const qc = new QueryClient({ defaultOptions: { queries: { gcTime: Infinity } } }); qc.setQueryData(["viewer"], "fixture"); qc.setQueryData(["pr-detail", request.repo, 1], detail); vi.mocked(reviewPrAtHead).mockResolvedValue({ outcome: "acknowledged", receipt }); return qc; }
const detail = { id: request.id, repo: request.repo, number: 1, head_oid: "head-1", latest_reviews: [] } as unknown as PrDetail;
it("shares pending ownership and preserves acknowledgement through repeated lagging reads", async () => {
 const qc = setup(); let resolve!: (value: Awaited<ReturnType<typeof reviewPrAtHead>>) => void;
 vi.mocked(reviewPrAtHead).mockReturnValueOnce(new Promise(r => { resolve = r; }));
 const first = submitBoundReview(qc, request);
 expect((await submitBoundReview(qc, request)).outcome).toBe("not_dispatched");
 resolve({ outcome: "acknowledged", receipt }); await first;
 for (let i = 0; i < 3; i++) expect(reconcileReviewDetail(qc, detail).latest_reviews[0].state).toBe("APPROVED");
 expect(reviewPrAtHead).toHaveBeenCalledTimes(1);
});
it("retires only identified dismissal or newer review, not aggregate or anonymous readback", async () => {
 const qc = setup(); await submitBoundReview(qc, request);
 expect(reconcileReviewDetail(qc, { ...detail, latest_reviews: [{ author: "fixture", state: "DISMISSED" }] }).latest_reviews[0].state).toBe("APPROVED");
 const dismissed = { ...detail, latest_reviews: [{ author: "fixture", state: "DISMISSED", id: receipt.review_id, commit_oid: "head-1" }] };
 expect(reconcileReviewDetail(qc, dismissed)).toBe(dismissed);
 expect(reviewOperation(qc, request.repo, 1, "head-1")).toBeUndefined();
});
it("qualifies after fifteen minutes without replay and explicit checked release permits another attempt", async () => {
 vi.useFakeTimers(); const qc = setup(); await submitBoundReview(qc, request);
 qc.setQueryData(["pr-detail", request.repo, 1], reconcileReviewDetail(qc, detail));
 await vi.advanceTimersByTimeAsync(REVIEW_ACK_MS);
 expect(reviewOperation(qc, request.repo, 1, "head-1")?.state).toBe("unresolved");
 expect(qc.getQueryData<PrDetail>(["pr-detail", request.repo, 1])?.latest_reviews).toEqual([]);
 expect((await submitBoundReview(qc, request)).outcome).toBe("not_dispatched");
 releaseCheckedReview(qc, request.repo, 1, "head-1"); await submitBoundReview(qc, request);
 expect(reviewPrAtHead).toHaveBeenCalledTimes(2);
});
it("never publishes a late receipt after account retirement", async () => {
 const qc = setup(); let resolve!: (value: Awaited<ReturnType<typeof reviewPrAtHead>>) => void;
 vi.mocked(reviewPrAtHead).mockReturnValueOnce(new Promise(r => { resolve = r; }));
 const result = submitBoundReview(qc, request); qc.setQueryData(["viewer"], "other");
 resolve({ outcome: "acknowledged", receipt }); expect((await result).outcome).toBe("uncertain");
 expect(reconcileReviewDetail(qc, detail)).toBe(detail);
});
it("does not fill capacity with hundreds of converged reviews", async () => {
 const qc = setup();
 for (let number = 1; number <= 600; number++) {
  qc.setQueryData(["pr-detail", request.repo, number], { ...detail, number });
  const r = { ...receipt, number, review_id: `review-${number}` }; vi.mocked(reviewPrAtHead).mockResolvedValue({ outcome: "acknowledged", receipt: r });
  expect((await submitBoundReview(qc, { ...request, number })).outcome).toBe("acknowledged");
  reconcileReviewDetail(qc, { ...detail, number, latest_reviews: [{ author: r.actor, state: r.state, id: r.review_id, commit_oid: r.commit_oid, submitted_at: r.submitted_at! }] });
 }
 for (const number of [1, 600]) expect(reconcileReviewDetail(qc, { ...detail, number, latest_reviews: [] }).latest_reviews[0].state).toBe("APPROVED");
 expect(qc.getQueryCache().getAll()).toHaveLength(601); // viewer + the 600 existing detail queries, no new storage queries
 expect(reviewPrAtHead).toHaveBeenCalledTimes(600);
});
it("refuses capacity before dispatch without silently evicting unresolved operations", async () => {
 const qc = setup(); vi.mocked(reviewPrAtHead).mockResolvedValue({ outcome: "uncertain", message: "Check GitHub" });
 for (let number = 1; number <= REVIEW_OPERATION_CAP; number++) await submitBoundReview(qc, { ...request, number });
 expect((await submitBoundReview(qc, { ...request, number: 1000 })).outcome).toBe("not_dispatched");
 expect(reviewPrAtHead).toHaveBeenCalledTimes(REVIEW_OPERATION_CAP);
 expect(reviewOperation(qc, request.repo, 1, "head-1")?.state).toBe("unresolved");
});
it("unsupported server never falls back and definite refusal permits deliberate retry", async () => {
 const qc = setup(); vi.mocked(reviewPrAtHead).mockRejectedValueOnce(new Error("unknown command review_pr_at_head"));
 expect((await submitBoundReview(qc, request)).outcome).toBe("not_dispatched");
 vi.mocked(reviewPrAtHead).mockResolvedValueOnce({ outcome: "rejected", message: "Cannot approve own pull request" });
 expect((await submitBoundReview(qc, request)).outcome).toBe("rejected");
 expect(reviewOperation(qc, request.repo, 1, "head-1")).toBeUndefined();
});

it("retires account ownership even when an unmounted account switches away and back before the reply", async () => {
 const qc = setup(); let resolve!: (value: Awaited<ReturnType<typeof reviewPrAtHead>>) => void;
 vi.mocked(reviewPrAtHead).mockReturnValueOnce(new Promise(r => { resolve = r; }));
 const result = submitBoundReview(qc, request);
 qc.setQueryData(["viewer"], "other"); qc.setQueryData(["viewer"], "fixture");
 resolve({ outcome: "acknowledged", receipt });
 expect((await result).outcome).toBe("uncertain");
 expect(reviewOperation(qc, request.repo, 1, "head-1")).toBeUndefined();
});

it("treats transported not-asked errors as definite without retaining a replay block", async () => {
 const qc = setup(); vi.mocked(reviewPrAtHead).mockRejectedValueOnce(new Error("headstate:not-asked cooldown"));
 expect(await submitBoundReview(qc, request)).toEqual({ outcome: "not_dispatched", message: "cooldown" });
 expect(reviewOperation(qc, request.repo, 1, "head-1")).toBeUndefined();
});

it("does not treat an unfamiliar read state as proof that a submitted review can be replayed", async () => {
 const qc = setup(); await submitBoundReview(qc, request);
 const read = { ...detail, latest_reviews: [{ author: "fixture", state: "FUTURE_STATE", id: receipt.review_id }] };
 expect(reconcileReviewDetail(qc, read).latest_reviews[0].state).toBe("APPROVED");
 expect(reviewOperation(qc, request.repo, 1, "head-1")?.state).toBe("acknowledged");
});

it("keeps confirmed authority in its existing query without consuming active capacity or blocking a different verdict", async () => {
 const qc = setup(); await submitBoundReview(qc, request);
 const count = qc.getQueryCache().getAll().length;
 reconcileReviewDetail(qc, { ...detail, latest_reviews: [{ author: receipt.actor, state: receipt.state, id: receipt.review_id, commit_oid: receipt.commit_oid, submitted_at: receipt.submitted_at! }] });
 expect(reviewOperation(qc, request.repo, 1, "head-1")).toBeUndefined();
 expect(qc.getQueryCache().getAll()).toHaveLength(count);
 expect((await submitBoundReview(qc, request)).outcome).toBe("not_dispatched");
 expect(reviewPrAtHead).toHaveBeenCalledTimes(1);
 vi.mocked(reviewPrAtHead).mockResolvedValueOnce({ outcome: "acknowledged", receipt: { ...receipt, review_id: "review-2", state: "CHANGES_REQUESTED", submitted_at: "2026-10-01T00:01:00Z" } });
 expect((await submitBoundReview(qc, { ...request, verdict: "request_changes", body: "Synthetic deliberate review" })).outcome).toBe("acknowledged");
 expect(reviewPrAtHead).toHaveBeenCalledTimes(2);
});

it("requires real head and ordered review evidence, and does not resurrect dismissed approval from an older read", async () => {
 const qc = setup(); await submitBoundReview(qc, request);
 const observed = { author: receipt.actor, state: receipt.state, id: receipt.review_id, commit_oid: receipt.commit_oid, submitted_at: receipt.submitted_at! };
 reconcileReviewDetail(qc, { ...detail, latest_reviews: [observed] });
 expect(reconcileReviewDetail(qc, { ...detail, latest_reviews: [{ ...observed, id: "wrong-head-newer", commit_oid: "other", submitted_at: "2026-10-01T00:02:00Z", state: "CHANGES_REQUESTED" }] }).latest_reviews[0].state).toBe("APPROVED");
 reconcileReviewDetail(qc, { ...detail, latest_reviews: [{ ...observed, state: "DISMISSED" }] });
 expect(reconcileReviewDetail(qc, { ...detail, latest_reviews: [observed] }).latest_reviews[0].state).toBe("DISMISSED");
 qc.setQueryData(["viewer"], "other"); qc.setQueryData(["viewer"], "fixture");
 expect(reconcileReviewDetail(qc, detail)).toBe(detail);
});

it.each(["rejected", "not_dispatched", "uncertain"] as const)("preserves confirmed R1 when a different verdict is %s", async outcome => {
 const qc = setup(); await submitBoundReview(qc, request);
 const observed = { author: receipt.actor, state: receipt.state, id: receipt.review_id, commit_oid: receipt.commit_oid, submitted_at: receipt.submitted_at! };
 reconcileReviewDetail(qc, { ...detail, latest_reviews: [observed] });
 vi.mocked(reviewPrAtHead).mockResolvedValueOnce({ outcome, message: "Synthetic refusal or uncertainty" });
 await submitBoundReview(qc, { ...request, verdict: "request_changes" });
 expect(reconcileReviewDetail(qc, detail).latest_reviews).toEqual([observed]);
});

it("retains an acknowledged ordering floor after expiry and checked release until real read convergence", async () => {
 vi.useFakeTimers(); const qc = setup(); await submitBoundReview(qc, request);
 const old = { author: receipt.actor, state: receipt.state, id: receipt.review_id, commit_oid: receipt.commit_oid, submitted_at: receipt.submitted_at! };
 reconcileReviewDetail(qc, { ...detail, latest_reviews: [old] });
 const newer = { ...old, id: "R2", state: "CHANGES_REQUESTED", submitted_at: "2026-10-01T00:02:00Z" };
 vi.mocked(reviewPrAtHead).mockResolvedValueOnce({ outcome: "acknowledged", receipt: { ...receipt, review_id: newer.id, state: newer.state, submitted_at: newer.submitted_at } });
 await submitBoundReview(qc, { ...request, verdict: "request_changes" });
 await vi.advanceTimersByTimeAsync(REVIEW_ACK_MS + 60_000);
 for (const latest_reviews of [[], [old]]) expect(reconcileReviewDetail(qc, { ...detail, latest_reviews }).latest_reviews).toEqual([]);
 releaseCheckedReview(qc, request.repo, 1, "head-1");
 expect(reconcileReviewDetail(qc, { ...detail, latest_reviews: [old] }).latest_reviews).toEqual([]);
 reconcileReviewDetail(qc, { ...detail, latest_reviews: [newer] });
 expect(reconcileReviewDetail(qc, detail).latest_reviews).toEqual([newer]);
 expect((await submitBoundReview(qc, { ...request, verdict: "request_changes" })).outcome).toBe("not_dispatched");
});

it("does not replace a newer confirmed read with a delayed older write receipt", async () => {
 const qc = setup(); await submitBoundReview(qc, request);
 const old = { author: receipt.actor, state: receipt.state, id: receipt.review_id, commit_oid: receipt.commit_oid, submitted_at: receipt.submitted_at! };
 reconcileReviewDetail(qc, { ...detail, latest_reviews: [old] });
 let resolve!: (value: Awaited<ReturnType<typeof reviewPrAtHead>>) => void;
 vi.mocked(reviewPrAtHead).mockReturnValueOnce(new Promise(r => { resolve = r; }));
 const pending = submitBoundReview(qc, { ...request, verdict: "request_changes" });
 const newer = { ...old, id: "R3", state: "COMMENTED", submitted_at: "2026-10-01T00:03:00Z" };
 reconcileReviewDetail(qc, { ...detail, latest_reviews: [newer] });
 resolve({ outcome: "acknowledged", receipt: { ...receipt, review_id: "R2", state: "CHANGES_REQUESTED", submitted_at: "2026-10-01T00:02:00Z" } });
 await pending;
 expect(reconcileReviewDetail(qc, detail).latest_reviews).toEqual([newer]);
 expect(reviewOperation(qc, request.repo, 1, "head-1")).toBeUndefined();
});
