import { commandError } from "../lib/errorKind";
import { useSyncExternalStore } from "react";
import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import type { PrDetail } from "../types/pr";
import { reviewPrAtHead, type BoundReviewRequest, type BoundReviewOutcome, type SubmittedReview } from "./tauri";

export const REVIEW_ACK_MS = 15 * 60_000;
export const REVIEW_OPERATION_CAP = 256;
export interface ReviewOperation {
  request: Omit<BoundReviewRequest, "body">;
  state: "pending" | "acknowledged" | "unresolved";
  receipt?: SubmittedReview;
  message?: string;
  acknowledgedAt?: number;
}
interface Store { revision: number; accountRevision: number; owner?: string; operations: Map<string, ReviewOperation>; listeners: Set<() => void> }
const stores = new WeakMap<QueryClient, Store>();
const key = (repo: string, number: number, head: string) => JSON.stringify([repo.toLowerCase(), number, head]);
function store(qc: QueryClient): Store {
  let value = stores.get(qc);
  if (!value) {
    value = { revision: 0, accountRevision: 0, operations: new Map(), listeners: new Set(), owner: qc.getQueryData<string>(["viewer"])?.toLowerCase() };
    stores.set(qc, value);
    const state = value;
    qc.getQueryCache().subscribe(event => {
      if (event.query.queryKey.length !== 1 || event.query.queryKey[0] !== "viewer") return;
      const before = state.owner;
      account(qc, state);
      if (before !== state.owner) changed(state);
    });
  }
  return value;
}
function changed(s: Store) { s.revision++; for (const listener of s.listeners) listener(); }
function account(qc: QueryClient, s: Store) {
  const owner = qc.getQueryData<string>(["viewer"])?.toLowerCase();
  if (owner !== undefined && s.owner !== owner) { if (s.owner !== undefined) s.accountRevision++; s.owner = owner; s.operations.clear(); }
  return owner;
}
export function reviewOperation(qc: QueryClient, repo: string, number: number, head: string) {
  const s = store(qc); account(qc, s);
  return s.operations.get(key(repo, number, head));
}
export function useReviewOperation(repo: string, number: number, head: string) {
  const qc = useQueryClient(); const s = store(qc);
  return useSyncExternalStore(
    (listener) => { s.listeners.add(listener); return () => { s.listeners.delete(listener); }; },
    () => reviewOperation(qc, repo, number, head),
  );
}
export function useReleaseCheckedReview() {
  const qc = useQueryClient();
  return (repo: string, number: number, head: string) => releaseCheckedReview(qc, repo, number, head);
}
export function releaseCheckedReview(qc: QueryClient, repo: string, number: number, head: string) {
  const s = store(qc); account(qc, s);
  const id = key(repo, number, head);
  if (s.operations.get(id)?.state === "unresolved") { s.operations.delete(id); changed(s); }
}
export function reviewAccountGeneration(qc: QueryClient) { return store(qc).accountRevision; }
export function reviewReadGeneration(qc: QueryClient) { return store(qc).revision; }
// One latest receipt belongs to the existing detail query. An acknowledgement
// is only an ordering floor until read-confirmed; expiry cannot revive older
// affirmative evidence. Query removal removes it, with no second collection.
const AUTHORITY_META = "headstateConfirmedReview";
type ReviewFact = { id: string; author: string; state: string; commit_oid: string; submitted_at?: string };
interface Authority { owner: string | undefined; accountRevision: number; prId: string; head: string; review: ReviewFact; confirmed: boolean }
const detailQuery = (qc: QueryClient, repo: string, number: number) =>
  qc.getQueryCache().find({ queryKey: ["pr-detail", repo, number], exact: true });
function readAuthority(qc: QueryClient, repo: string, number: number): Authority | undefined {
  const authority = detailQuery(qc, repo, number)?.meta?.[AUTHORITY_META] as Authority | undefined;
  const s = store(qc);
  return authority?.owner === s.owner && authority?.accountRevision === s.accountRevision ? authority : undefined;
}
function writeAuthority(qc: QueryClient, fresh: Pick<PrDetail, "repo" | "number" | "id" | "head_oid">, review?: ReviewFact, confirmed = true) {
  const query = detailQuery(qc, fresh.repo, fresh.number);
  if (!query) return;
  const s = store(qc);
  // Preserve the shared metadata object referenced by every observer's options.
  // Replacing it lets an already-created observer restore an older empty object.
  const meta = query.meta ?? {};
  meta[AUTHORITY_META] = review
    ? { owner: s.owner, accountRevision: s.accountRevision, prId: fresh.id, head: fresh.head_oid, review, confirmed } satisfies Authority
    : undefined;
  if (!query.meta) query.setOptions({ ...query.options, meta });
}
function usableReview(review: PrDetail["latest_reviews"][number] | undefined, owner: string | undefined, head: string): review is ReviewFact {
  return !!review?.id && !!head && review.commit_oid === head && review.author.toLowerCase() === owner
    && ["APPROVED", "CHANGES_REQUESTED", "COMMENTED", "DISMISSED"].includes(review.state);
}
function newerReview(read: ReviewFact, before: { id: string; state: string; submitted_at?: string | null }) {
  if (read.id === before.id) return read.state === before.state || read.state === "DISMISSED";
  return !!read.submitted_at && !!before.submitted_at && Date.parse(read.submitted_at) > Date.parse(before.submitted_at);
}
function withReview(fresh: PrDetail, owner: string, review: PrDetail["latest_reviews"][number]): PrDetail {
  return { ...fresh, latest_reviews: [...fresh.latest_reviews.filter(r => r.author.toLowerCase() !== owner), review] };
}
export function reconcileReviewDetail(qc: QueryClient, fresh: PrDetail, generation?: number): PrDetail {
  const s = store(qc); const owner = account(qc, s);
  const currentRead = generation === undefined || generation === s.revision;
  const observed = fresh.latest_reviews.find(r => r.author.toLowerCase() === owner);
  for (const [id, op] of s.operations) {
    if (op.request.repo.toLowerCase() !== fresh.repo.toLowerCase() || op.request.number !== fresh.number) continue;
    if (currentRead && fresh.head_oid && fresh.head_oid !== op.request.expected_head) {
      s.operations.delete(id); changed(s); continue;
    }
    const receipt = op.receipt;
    if (!receipt || owner !== receipt.actor.toLowerCase()) continue;
    if (currentRead && usableReview(observed, owner, fresh.head_oid)
      && newerReview(observed, { id: receipt.review_id, state: receipt.state, submitted_at: receipt.submitted_at })) {
      writeAuthority(qc, fresh, { ...observed });
      s.operations.delete(id); changed(s);
      continue;
    }
    if (op.state === "acknowledged" && fresh.head_oid === receipt.commit_oid) {
      return withReview(fresh, owner, { author: receipt.actor, state: receipt.state, id: receipt.review_id,
        commit_oid: receipt.commit_oid, ...(receipt.submitted_at ? { submitted_at: receipt.submitted_at } : {}) });
    }
  }
  const authority = readAuthority(qc, fresh.repo, fresh.number);
  if (!authority || !owner || authority.prId !== fresh.id) return fresh;
  if (fresh.head_oid && fresh.head_oid !== authority.head) {
    if (currentRead) writeAuthority(qc, fresh);
    return fresh;
  }
  if (fresh.head_oid !== authority.head) return fresh;
  if (currentRead && usableReview(observed, owner, fresh.head_oid) && newerReview(observed, authority.review)) {
    writeAuthority(qc, fresh, { ...observed });
    return fresh;
  }
  return authority.confirmed ? withReview(fresh, owner, authority.review)
    : { ...fresh, latest_reviews: fresh.latest_reviews.filter(r => r.author.toLowerCase() !== owner) };
}
export async function submitBoundReview(qc: QueryClient, request: BoundReviewRequest): Promise<BoundReviewOutcome> {
  const s = store(qc); const owner = account(qc, s); const id = key(request.repo, request.number, request.expected_head);
  if (!owner || owner !== request.expected_viewer.toLowerCase() || !request.expected_head)
    return { outcome: "not_dispatched", message: "A known account and viewed head are required before reviewing." };
  const authority = readAuthority(qc, request.repo, request.number);
  const requestedState = request.verdict === "approve" ? "APPROVED" : request.verdict === "request_changes" ? "CHANGES_REQUESTED" : "COMMENTED";
  if (authority?.confirmed && authority.prId === request.id && authority.head === request.expected_head && authority.review.state === requestedState)
    return { outcome: "not_dispatched", message: "Your review is already confirmed for this head. Check GitHub before repeating it." };
  if (s.operations.has(id)) return { outcome: "not_dispatched", message: "This review is already recorded or unresolved. Check its state on GitHub before another submission." };
  if (s.operations.size >= REVIEW_OPERATION_CAP) return { outcome: "not_dispatched", message: "There are too many unresolved reviews. Check their state on GitHub before submitting another review." };
  const op: ReviewOperation = { request: {
    id: request.id, repo: request.repo, number: request.number, verdict: request.verdict,
    expected_head: request.expected_head, expected_viewer: request.expected_viewer,
  }, state: "pending" }; s.operations.set(id, op); changed(s);
  let outcome: BoundReviewOutcome;
  try { outcome = await reviewPrAtHead(request); }
  catch (error) {
    const failure = commandError(error);
    const message = failure.message;
    // Unknown-command errors prove that the older desktop never dispatched this command.
    outcome = failure.kind === "not-asked" ? { outcome: "not_dispatched", message }
      : /(?:unknown|not found|not supported|unrecognized).*command|command.*(?:not found|unknown|not supported)/i.test(message)
      ? { outcome: "not_dispatched", message: `Update the desktop to review the viewed head. ${message}` }
      : { outcome: "uncertain", message };
  }
  if (!outcome || !["acknowledged", "uncertain", "not_dispatched", "rejected"].includes(outcome.outcome))
    outcome = { outcome: "uncertain", message: "The desktop did not return a review receipt. Check GitHub before trying again." };
  if (account(qc, s) !== owner || s.operations.get(id) !== op)
    return { outcome: "uncertain", message: "The account or viewed head changed. Check the review on GitHub." };
  if (outcome.outcome === "not_dispatched" || outcome.outcome === "rejected") { s.operations.delete(id); changed(s); return outcome; }
  const next: ReviewOperation = outcome.outcome === "acknowledged"
    ? { ...op, state: "acknowledged", receipt: outcome.receipt, acknowledgedAt: Date.now() }
    : { ...op, state: "unresolved", message: outcome.message };
  if (outcome.outcome === "acknowledged") {
    const receipt = outcome.receipt;
    const fact: ReviewFact = { id: receipt.review_id, author: receipt.actor, state: receipt.state,
      commit_oid: receipt.commit_oid, ...(receipt.submitted_at ? { submitted_at: receipt.submitted_at } : {}) };
    const current = readAuthority(qc, request.repo, request.number);
    if (current && current !== authority && current.confirmed && current.prId === request.id
      && current.head === request.expected_head && newerReview(current.review, fact)) {
      s.operations.delete(id); changed(s);
      return outcome;
    }
    // A semantic write supersedes the fact present when it started. If a newer
    // read arrived meanwhile, require ordering evidence before replacing it.
    if (receipt.pr_id === request.id && receipt.repo.toLowerCase() === request.repo.toLowerCase()
      && receipt.number === request.number && usableReview(fact, owner, request.expected_head)
      && (!current || current === authority || newerReview(fact, current.review))) {
      writeAuthority(qc, { ...request, head_oid: request.expected_head }, fact, false);
    }
  }
  s.operations.set(id, next); changed(s);
  if (next.state === "acknowledged") setTimeout(() => {
    if (s.operations.get(id) !== next) return;
    s.operations.set(id, { ...next, state: "unresolved", message: "GitHub confirmed the review, but current readback has not converged. Check its state before another submission." });
    qc.setQueryData<PrDetail>(["pr-detail", next.request.repo, next.request.number], detail => detail ? {
      ...detail, latest_reviews: detail.latest_reviews.filter(review => review.id !== next.receipt?.review_id),
    } : detail);
    changed(s);
    void qc.invalidateQueries({ queryKey: ["pr-detail", next.request.repo, next.request.number], exact: true });
  }, REVIEW_ACK_MS);
  return outcome;
}

export function retireReviewOwnership(qc: QueryClient) {
  const value = store(qc);
  value.accountRevision++;
  value.owner = undefined;
  value.operations.clear();
  changed(value);
}
