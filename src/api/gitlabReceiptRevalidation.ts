import { focusManager, type Query, type QueryClient } from "@tanstack/react-query";
import type { MergeRequest } from "../types/gitlab";
import type { GitLabCapabilities, GitLabDetail } from "../types/gitlabActions";
import type { SourcePollUpdate } from "./tauri";
import { GitLabQueueState } from "./gitlabQueueState";
import { prKey } from "../lib/prIdentity";

type Facts = Record<string, string>;
type Target = { facts: Facts; absent: Set<string> };
type State = { receipts: Map<string, GitLabQueueState>; targets: WeakMap<Query, Target>; dirty: Set<Query>; scheduled: boolean; stopFocus?: () => void };
const clients = new WeakMap<QueryClient, State>();

function state(client: QueryClient): State {
  const existing = clients.get(client);
  if (existing) return existing;
  const value: State = { receipts: new Map(), targets: new WeakMap(), dirty: new Set(), scheduled: false };
  clients.set(client, value);
  client.getQueryCache().subscribe(event => {
    if (event.type === "removed" || (event.type === "updated" && event.action.type === "fetch")) value.dirty.delete(event.query);
    // A focus/mount/manual read beginning after a receipt satisfies its target.
    // A receipt arriving after that fetch event still earns a follow-up.
    if (value.dirty.size === 0) { value.stopFocus?.(); value.stopFocus = undefined; }
    if (event.type === "updated" && event.query.state.fetchStatus === "idle" && value.dirty.has(event.query)) schedule(client, value);
    if (event.type === "observerAdded" && value.dirty.has(event.query)) schedule(client, value);
  });
  // The cache owns pending work, including inactive entries. A hidden window
  // does not issue provider reads; focus resumes the coalesced target once.
  return value;
}
function schedule(client: QueryClient, value: State) {
  if (value.dirty.size > 0 && !value.stopFocus) value.stopFocus = focusManager.subscribe(focused => { if (focused) schedule(client, value); });
  if (value.scheduled) return;
  value.scheduled = true;
  queueMicrotask(() => {
    value.scheduled = false;
    if (!focusManager.isFocused()) return;
    for (const query of value.dirty) {
      if (!query.isActive() || query.state.fetchStatus !== "idle") continue;
      value.dirty.delete(query);
      void client.refetchQueries({ queryKey: query.queryKey, exact: true, type: "active" }, { cancelRefetch: false });
    }
    if (value.dirty.size === 0) { value.stopFocus?.(); value.stopFocus = undefined; }
  });
}
/** Never cancel a read: progress during it earns one read after settlement. */
export function revalidateGitLabQueries(client: QueryClient, queries: Query[]) {
  const value = state(client);
  for (const query of queries) {
    value.dirty.add(query);
    void client.invalidateQueries({ queryKey: query.queryKey, exact: true, refetchType: "none" });
  }
  schedule(client, value);
}
function facts(row: MergeRequest): Facts {
  const observation = row.observation;
  if (observation?.state === "retained") return {};
  const known = (field: "head" | "draft" | "ci" | "merge" | "review" | "queue") => !observation || (!observation.unknown_fields.includes(field) && !observation.retained_fields.includes(field));
  const value: Facts = { title: row.title };
  const set = (key: string, item: unknown) => { if (item !== null && item !== undefined) value[key] = JSON.stringify(item); };
  if (known("head")) set("head", row.head_oid);
  if (known("draft")) set("draft", row.is_draft);
  if (known("ci")) set("ci", row.ci);
  if (known("merge")) set("merge", row.detailed_merge_status);
  if (known("review")) set("review", row.review);
  if (known("queue")) set("queue", row.in_merge_queue);
  if (observation?.detail_fields?.includes("base")) set("base", row.base_ref);
  if (observation?.detail_fields?.includes("comments")) set("comments", row.comment_count);
  if (observation?.detail_fields?.includes("threads")) set("threads", [row.unresolved_threads, row.unresolved_threads_floor]);
  if (observation?.detail_fields?.includes("reviewers")) set("reviewers", [...row.reviewers].sort());
  return value;
}
function baseline(client: QueryClient, query: Query): Facts {
  const detail = client.getQueryData<GitLabDetail>(["gitlab-detail", query.queryKey[1], query.queryKey[2]]);
  const value: Facts = detail ? { title: detail.core.title, head: JSON.stringify(detail.core.head_oid), draft: JSON.stringify(detail.core.is_draft), merge: JSON.stringify(detail.core.detailed_merge_status), base: JSON.stringify(detail.core.base_ref) } : {};
  // Capabilities are independently cached. Current detail cannot attest that
  // the permissions were read for the same head.
  if (query.queryKey[0] === "gitlab-actions") {
    const capabilities = query.state.data as GitLabCapabilities | undefined;
    value.head = JSON.stringify(capabilities?.head_oid ?? null);
  }
  return value;
}

/** Called only with a reducer-accepted receipt. Shared acceptance deduplicates
 * independent hook instances; list revisions are never compared globally. */
export function acceptGitLabReceipt(client: QueryClient, viewer: string | null | undefined, receipt: SourcePollUpdate) {
  if (viewer == null) return;
  const value = state(client);
  const scope = JSON.stringify([receipt.source.host, receipt.list, viewer]);
  let reducer = value.receipts.get(scope);
  if (!reducer) { reducer = new GitLabQueueState(receipt.source.host, receipt.list, viewer); value.receipts.set(scope, reducer); }
  if (!reducer.accept(receipt)) return;
  const rows = new Map((receipt.mrs ?? []).map(row => [prKey(row), row]));
  const queries = client.getQueryCache().findAll({ predicate: query => {
    if (!["gitlab-detail", "gitlab-actions"].includes(String(query.queryKey[0])) || query.queryKey[2] !== viewer) return false;
    try { const id = JSON.parse(String(query.queryKey[1])); return id[0] === "gitlab" && id[1] === receipt.source.host; } catch { return false; }
  } });
  const membership = JSON.stringify([receipt.list, receipt.session]);
  const changed: Query[] = [];
  for (const query of queries) {
    let target = value.targets.get(query);
    if (!target) { target = { facts: baseline(client, query), absent: new Set() }; value.targets.set(query, target); }
    const row = rows.get(String(query.queryKey[1]));
    if (!row) {
      if (receipt.coverage === "complete" && receipt.owner === viewer && !target.absent.has(membership)) { target.absent.add(membership); changed.push(query); }
      continue;
    }
    if (row.observation?.state === "retained") continue;
    const next = facts(row);
    const differs = target.absent.has(membership) || query.state.data === undefined || Object.entries(next).some(([key, fact]) => target.facts[key] !== fact);
    target.facts = { ...target.facts, ...next }; target.absent.delete(membership);
    if (differs) changed.push(query);
  }
  revalidateGitLabQueries(client, changed);
}
