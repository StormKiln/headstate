import type { Query, QueryClient } from "@tanstack/react-query";
import type { PrDetail, PullRequest, ReadinessField, RowObservation } from "../types/pr";
import type { ProviderReceipt } from "./sourceRefresh";
import { reviewAccountGeneration } from "./reviewOperations";

type Facts = Record<string, string>;
interface Target { revision: number; facts: Facts; probe?: boolean }
interface State { facts: Facts; observed: Facts; initial: boolean; revision: number; required?: Target; reading?: number; account: number; session: number }
type Member = Pick<PullRequest, "repo" | "number">;
interface Source { session?: string; generation: number; retired: Set<string>; account: number; membership: Partial<Record<"authored" | "reviewing", Map<string, Member>>> }
const memberKey = (row: Member) => JSON.stringify([row.repo.toLowerCase(), row.number]);
const states = new WeakMap<Query, State>();
const sources = new WeakMap<QueryClient, Source>();
export const reviewReconciliations = new WeakMap<QueryClient, Map<string, symbol>>();
export const reviewKey = (repo: string, number: number) => JSON.stringify([repo, number]);
// Read lifecycle belongs to the caller's actual cache entry. Normalized source
// identity can name several independently observed aliases of that entry.
const detailQuery = (qc: QueryClient, repo: string, number: number) => qc.getQueryCache().find({ queryKey: ["pr-detail", repo, number], exact: true });
const detailAliases = (qc: QueryClient, repo: string, number: number) => qc.getQueryCache().findAll({
  queryKey: ["pr-detail"],
  predicate: query => query.queryKey.length === 3 && typeof query.queryKey[1] === "string" && query.queryKey[1].toLowerCase() === repo.toLowerCase() && query.queryKey[2] === number,
});
const ordered = (values: unknown[]) => JSON.stringify(values.map(v => JSON.stringify(v)).sort());
function facts(row: PullRequest): Facts {
  const observation = row.observation;
  if (observation?.state === "retained") return {};
  const observed = (field: ReadinessField) => !!observation && !observation.unknown_fields.includes(field) && !observation.retained_fields.includes(field);
  const extra = (field: NonNullable<RowObservation["detail_fields"]>[number]) => observation?.detail_fields?.includes(field) === true;
  const value: Facts = {};
  if (row.head_oid && (!observation || observed("head"))) value.head = row.head_oid;
  if (row.base_ref && extra("base")) value.base = row.base_ref;
  if (observed("draft")) value.draft = String(row.is_draft);
  if (observed("ci")) value.ci = row.ci;
  if (observed("merge")) {
    if (row.merge !== "checking") value.merge = row.merge;
    if (row.merge_status !== "unknown") value.mergeStatus = row.merge_status;
  }
  if (observed("review")) value.review = row.review;
  if (observed("queue")) value.queue = String(row.in_merge_queue);
  if (extra("comments")) value.comments = String(row.comment_count);
  if (extra("threads")) value.threads = JSON.stringify([row.unresolved_threads, row.unresolved_threads_floor]);
  if (extra("reviewers")) value.reviewers = JSON.stringify([ordered(row.requested_reviewers), row.requested_reviewers_total]);
  if (extra("reviews")) value.reviews = JSON.stringify([ordered(row.latest_reviews.map(r => [r.author, r.state, r.id ?? null, r.submitted_at ?? null, r.commit_oid ?? null])), row.latest_reviews_total]);
  return value;
}
function baseline(detail: PrDetail | undefined): Facts {
  return detail ? { mergeStatus: detail.merge_status, head: detail.head_oid, base: detail.base_ref, draft: String(detail.is_draft), review: detail.review,
    queue: String(detail.in_merge_queue) } : {};
}
function source(qc: QueryClient): Source {
  let value = sources.get(qc);
  if (value) return value;
  value = { generation: 0, retired: new Set(), account: reviewAccountGeneration(qc), membership: {} }; sources.set(qc, value);
  // Query-owned targets disappear with their query. A single subscription per
  // client reconciles actual fetch successes, never optimistic/manual patches.
  qc.getQueryCache().subscribe(event => {
    if (event.type !== "updated" || event.action.type !== "success" || event.action.manual) return;
    const query = event.query;
    const state = current(qc, query);
    if (!state) return;
    const data = query.state.data as PrDetail | undefined;
    // Only scalars with identical source/detail semantics can prove readback.
    // Counts, capped lists and CI aggregates are source-to-source signals; a
    // full post-target read acknowledges them without comparing unlike data.
    const comparable = baseline(data);
    const matches = (required: Facts) => ["head", "base", "draft", "review", "queue", "mergeStatus"]
      .every(field => required[field] === undefined || required[field] === comparable[field]);
    // A receipt during the cold read has no full detail to compare yet. Make
    // that first comparison once it arrives; never reopen a consumed target
    // by comparing later reads against an old source snapshot.
    if (state.initial) {
      state.initial = false;
      if (!state.required && !matches(state.observed)) state.required = { revision: state.revision, facts: { ...state.observed } };
    }
    const target = state.required;
    if (!target) return;
    const overtaken = target.revision > (state.reading ?? -1);
    if (!overtaken && data && matches(target.facts)) {
      state.required = undefined;
    } else if (overtaken) {
      // Let TanStack finish the current retryer before asking for the single
      // latest target. Repeated receipts never cancel/restart its transport.
      queueMicrotask(() => { if (current(qc, query)?.required) request(qc, query); });
    }
  });
  return value;
}
function current(qc: QueryClient, query: Query): State | undefined {
  const state = states.get(query);
  return state?.account === reviewAccountGeneration(qc) && state.session === source(qc).generation ? state : undefined;
}
function request(qc: QueryClient, query: Query) {
  if (query.state.fetchStatus !== "idle" || reviewReconciliations.get(qc)?.has(reviewKey(query.queryKey[1] as string, query.queryKey[2] as number))) return;
  void qc.invalidateQueries({ queryKey: query.queryKey, exact: true, refetchType: "active" }, { cancelRefetch: false });
}
/** Only accepted source rows call this. Receipt time and mutation acknowledgments
 * are deliberately absent from the fingerprint; neither is a provider fact. */
export function acceptDetailFacts(qc: QueryClient, { rows, session, coverage, list }: ProviderReceipt & { list: "authored" | "reviewing" }) {
  const src = source(qc);
  if (session !== undefined && src.retired.has(session)) return;
  if (session !== undefined && session !== src.session) {
    if (src.session !== undefined) { src.retired.add(src.session); src.generation++; }
    src.session = session;
    src.membership = {};
  }
  const account = reviewAccountGeneration(qc);
  if (src.account !== account) { src.membership = {}; src.account = account; }
  const previous = src.membership[list] ?? new Map<string, Member>();
  const observed = new Map(rows.filter(row => row.observation?.state !== "retained").map(row => [memberKey(row), { repo: row.repo, number: row.number }]));
  const present = new Set(rows.map(memberKey));
  const missing = coverage === "complete" ? [...previous].filter(([id]) => !present.has(id)).map(([, row]) => row) : [];
  src.membership[list] = coverage === "complete" ? new Map([...previous].filter(([id]) => present.has(id)).concat([...observed])) : new Map([...previous, ...observed]);
  for (const row of rows) for (const query of detailAliases(qc, row.repo, row.number)) {
    let state = current(qc, query);
    if (!state) {
      state = { facts: baseline(query.state.data as PrDetail | undefined), observed: {}, initial: query.state.data === undefined, revision: 0, account: reviewAccountGeneration(qc), session: src.generation };
      states.set(query, state);
    }
    const next = facts(row);
    const changed = Object.entries(next).some(([key, value]) => state.facts[key] !== undefined && state.facts[key] !== value);
    // Do not transplant head-dependent requirements across a positively new
    // head when that receipt could not observe the new head's scalar values.
    if (next.head && next.head !== state.facts.head) state.observed = {};
    state.observed = { ...state.observed, ...next };
    state.facts = { ...state.facts, ...next };
    if (!changed) continue;
    state.required = { revision: ++state.revision, facts: { ...state.observed } };
    // Inactive details need a stale marker, but no provider command.
    void qc.invalidateQueries({ queryKey: query.queryKey, exact: true, refetchType: "none" });
    request(qc, query);
  }
  for (const row of missing) for (const query of detailAliases(qc, row.repo, row.number)) {
    let state = current(qc, query);
    if (!state) {
      state = { facts: baseline(query.state.data as PrDetail | undefined), observed: {}, initial: false, revision: 0, account, session: src.generation };
      states.set(query, state);
    }
    // Two inventories can lose the same PR while one probe is pending. Its
    // result observes state without predicting CLOSED or MERGED from absence.
    if (state.required?.probe) continue;
    state.required = { revision: ++state.revision, facts: {}, probe: true };
    void qc.invalidateQueries({ queryKey: query.queryKey, exact: true, refetchType: "none" });
    request(qc, query);
  }
}
export function beginDetailRead(qc: QueryClient, repo: string, number: number) {
  const query = detailQuery(qc, repo, number);
  const state = query && current(qc, query);
  if (state) state.reading = state.revision;
  return source(qc).generation;
}
export function detailReadIsCurrent(qc: QueryClient, generation: number) { return source(qc).generation === generation; }
export function detailNeedsRevalidation(qc: QueryClient, repo: string, number: number) {
  const query = detailQuery(qc, repo, number);
  return !!(query && current(qc, query)?.required);
}
export function resumeDetailRevalidation(qc: QueryClient, repo: string, number: number) {
  const query = detailQuery(qc, repo, number);
  if (query && current(qc, query)?.required) request(qc, query);
}

export function retireDetailOwnership(qc: QueryClient, resetSessions = false) {
  const src = source(qc);
  if (resetSessions) src.retired.clear();
  else if (src.session) src.retired.add(src.session);
  src.session = undefined;
  src.membership = {};
  src.generation++;
  reviewReconciliations.delete(qc);
}
