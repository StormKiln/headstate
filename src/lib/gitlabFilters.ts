import { prKey } from "./prIdentity";
import type { MergeRequest } from "../types/gitlab";
import type { Filters } from "./derive";
import { STALE_DAYS } from "./derive";

/** Filter measured GitLab evidence without casting it into a GitHub receipt. */
export function filterGitLab(rows: MergeRequest[], f: Filters, now = Date.now()): MergeRequest[] {
  return rows.filter(row => {
    const q = f.query?.trim().toLowerCase();
    if (f.repo && row.repo !== f.repo) return false;
    if (q && !row.title.toLowerCase().includes(q) && !row.repo.toLowerCase().includes(q) && String(row.number) !== q.replace(/^[#!]/, "")) return false;
    if (f.readyOnly && row.is_draft || f.draftsOnly && !row.is_draft) return false;
    if (f.ci && row.ci !== f.ci || f.review && row.review !== f.review) return false;
    if (f.unresolvedOnly && !(row.unresolved_threads !== null && row.unresolved_threads > 0)) return false;
    if (f.includeLabels?.length && !row.labels.some(l => f.includeLabels!.includes(l.name))) return false;
    if (f.excludeLabels?.length && row.labels.some(l => f.excludeLabels!.includes(l.name))) return false;
    if (f.needsAttentionOnly && row.ci !== "failure" && row.detailed_merge_status !== "conflict") return false;
    if (f.staleOnly && now - Date.parse(row.updated_at) < STALE_DAYS * 86_400_000) return false;
    if (f.awaitingReviewOnly && (row.is_draft || row.review !== "review_required")) return false;
    // These filters require evidence not present in older desktop receipts.
    // Unknown cannot be promoted to an affirmative match.
    if (f.inMergeQueueOnly && row.in_merge_queue !== true) return false;
    if (f.readyToQueueOnly && (row.is_draft || row.can_enqueue_train !== true)) return false;
    if (f.needsMyReviewOnly && row.needs_my_review !== true) return false;
    return true;
  });
}

/** Structural branch relationships, scoped to the complete provider identity. */
export function gitlabParents(rows: MergeRequest[]): Map<string, number> {
  const heads = new Map<string, MergeRequest[]>();
  const key = (row: MergeRequest, branch: string) => JSON.stringify([row.source.provider, row.source.host, row.repo, branch]);
  for (const row of rows) if (row.head_ref) {
    const k = key(row, row.head_ref);
    heads.set(k, [...(heads.get(k) ?? []), row]);
  }
  const parents = new Map<string, number>();
  for (const row of rows) {
    const candidates = heads.get(key(row, row.base_ref));
    if (candidates?.length === 1 && prKey(candidates[0]) !== prKey(row)) parents.set(prKey(row), candidates[0].number);
  }
  return parents;
}
