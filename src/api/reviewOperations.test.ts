import { QueryClient } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import type { PrDetail } from "../types/pr";
import { reviewPrAtHead, type BoundReviewRequest, type SubmittedReview } from "./tauri";
import { reconcileReviewDetail, reviewOperation, submitBoundReview, REVIEW_ACK_MS, REVIEW_OPERATION_CAP, releaseCheckedReview } from "./reviewOperations";
vi.mock("./tauri", () => ({ reviewPrAtHead: vi.fn() }));
afterEach(() => { vi.clearAllMocks(); vi.useRealTimers(); });
const request: BoundReviewRequest = { id: "PR-1", repo: "fixture/project", number: 1, verdict: "approve", body: "", expected_head: "head-1", expected_viewer: "fixture" };
const receipt: SubmittedReview = { review_id: "REVIEW-1", pr_id: request.id, repo: request.repo, number: 1, state: "APPROVED", actor: "fixture", commit_oid: "head-1", submitted_at: "2026-10-01T00:00:00Z" };
function setup() { const qc = new QueryClient({ defaultOptions: { queries: { gcTime: Infinity } } }); qc.setQueryData(["viewer"], "fixture"); vi.mocked(reviewPrAtHead).mockResolvedValue({ outcome: "acknowledged", receipt }); return qc; }
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
 const dismissed = { ...detail, latest_reviews: [{ author: "fixture", state: "DISMISSED", id: receipt.review_id }] };
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
  const r = { ...receipt, number, review_id: `review-${number}` }; vi.mocked(reviewPrAtHead).mockResolvedValue({ outcome: "acknowledged", receipt: r });
  expect((await submitBoundReview(qc, { ...request, number })).outcome).toBe("acknowledged");
  reconcileReviewDetail(qc, { ...detail, number, latest_reviews: [{ author: r.actor, state: r.state, id: r.review_id }] });
 }
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
