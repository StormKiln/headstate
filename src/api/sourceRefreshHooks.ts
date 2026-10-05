import { retireReviewOwnership } from "./reviewOperations";
import { acceptDetailFacts, retireDetailOwnership } from "./detailRevalidation";
import { useEffect, useSyncExternalStore } from "react";
import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import type { PullRequest } from "../types/pr";
import { SourceRefreshState, type SourceStatus } from "./sourceRefresh";
import { getSourceSnapshot, refreshSource } from "./tauri";
import { listen, type UnlistenFn } from "./transport";
import { safeUnlisten } from "./unlisten";
import { IS_MOBILE_BUILD } from "../lib/target";
import { timeCall } from "./diag";

type List = "authored" | "reviewing";
type Entry = { state: SourceRefreshState; users: number; stop?: () => void };
const sessions = new WeakMap<QueryClient, { current?: string; retired: Set<string> }>();
const owners = new WeakMap<QueryClient, string>();
const entries = new WeakMap<QueryClient, Partial<Record<List, Entry>>>();
const authored = new Set<SourceRefreshState>();
let nextRequest = 0;
// Unique across webviews/phone sessions, not persisted or used as credentials.
const requestPrefix = crypto.randomUUID();
/** Pairing/account retirement, never an ordinary reconnect. Old closures keep a
 * retired state so late command completions cannot repopulate query data. */
export function retireSourceOwnership(qc: QueryClient, preserveSession = false) {
  // Backend UUIDs are scoped to a pairing generation. Old closures/requests
  // retain retired entries; a new pairing may use the same running backend.
  if (!preserveSession) sessions.delete(qc);
  owners.delete(qc);
  retireDetailOwnership(qc, !preserveSession);
  retireReviewOwnership(qc);
  const lists = entries.get(qc);
  entries.delete(qc);
  for (const value of Object.values(lists ?? {})) {
    value.stop?.();
    value.state.retire();
  }
}
function retireCurrentQueue(qc: QueryClient) {
  retireSourceOwnership(qc, true);
  void qc.cancelQueries();
  qc.removeQueries({ predicate: query => query.queryKey[0] !== "connection-state" });
}
function acceptSession(qc: QueryClient, session: string | undefined) {
  if (!session) return true;
  let value = sessions.get(qc);
  if (!value) { value = { retired: new Set() }; sessions.set(qc, value); }
  if (value.retired.has(session)) return false;
  if (value.current && value.current !== session) value.retired.add(value.current);
  value.current = session;
  return true;
}
function adoptOwner(qc: QueryClient, owner: string | undefined) {
  if (!owner) return;
  const before = owners.get(qc);
  if (before && before.toLowerCase() !== owner.toLowerCase()) {
    retireCurrentQueue(qc);
  }
  owners.set(qc, owner);
}
function entry(qc: QueryClient, list: List): Entry {
  let lists = entries.get(qc);
  if (!lists) { lists = {}; entries.set(qc, lists); }
  return lists[list] ??= { state: new SourceRefreshState((rows, session) => acceptDetailFacts(qc, rows, session)), users: 0 };
}
function observe(qc: QueryClient, list: List, value: Entry) {
  if (value.users++ > 0) return;
  const { state } = value;
  if (list === "authored") authored.add(state);
  let cancelled = false;
  const unlisteners: UnlistenFn[] = [];
  const register = (pending: Promise<UnlistenFn>) => {
    pending.then((fn) => { if (cancelled) safeUnlisten(fn); else unlisteners.push(fn); }, () => {});
  };
  let rows = state.snapshot().prs;
  const unsubscribe = state.subscribe(() => {
    const next = state.snapshot().prs;
    if (next !== undefined && next !== rows && state.snapshot().fetchedAt === undefined) {
      rows = next;
      qc.setQueryData([list === "authored" ? "prs" : "reviewing"], next);
    }
  });
  register(listen<SourceStatus>("source-poll-status", ({ payload }) => {
    if (!cancelled && payload.source.provider === "github" && payload.source.host === "github.com" && payload.list === list) {
      const previousOwner = owners.get(qc);
      if (payload.owner && previousOwner && payload.owner.toLowerCase() !== previousOwner.toLowerCase() && !state.allowsOwnership(payload)) return;
      if (!acceptSession(qc, payload.session)) return;
      const previousSession = state.snapshot().session;
      if (previousSession && payload.session && previousSession !== payload.session && !payload.owner) retireCurrentQueue(qc);
      adoptOwner(qc, payload.owner ?? undefined);
      const current = entry(qc, list).state;
      current.accept(payload);
      if (payload.owner && current.snapshot().prs === undefined) void readRetained(qc, list).catch(() => {});
    }
  }));
  // Legacy opening events may be ownerless disk arrays from an old desktop.
  // Modern source receipts and correlated live commands supply trustworthy rows.
  if (!IS_MOBILE_BUILD) register(listen<PullRequest[]>(list === "authored" ? "prs-updated" : "reviewing-updated", ({ payload }) => { if (!cancelled) state.legacyRows(payload); }));
  if (list === "authored") register(listen<string>("poll-error", ({ payload }) => { if (!cancelled) state.legacyError(payload); }));
  value.stop = () => {
    cancelled = true;
    for (const unlisten of unlisteners) safeUnlisten(unlisten);
    unsubscribe();
    authored.delete(state);
  };
}

/// Queries may be disabled when their view is hidden; observation stays active.
export function useSourceRefresh(list: List) {
  const qc = useQueryClient();
  const value = entry(qc, list);
  useEffect(() => {
    observe(qc, list, value);
    return () => { if (--value.users === 0) value.stop?.(); };
  }, [qc, list, value]);
  return useSyncExternalStore(value.state.subscribe, value.state.snapshot);
}

export function clearAuthoredError() {
  for (const state of authored) state.dismiss();
}

export async function refreshWithState(qc: QueryClient, list: List): Promise<PullRequest[]> {
  const value = entry(qc, list);
  const startedSession = sessions.get(qc)?.current;
  let state = value.state;
  let request = state.start(`${requestPrefix}:${++nextRequest}`);
  let reply;
  try {
    reply = await timeCall(list === "authored" ? "prs" : "reviewing", () => refreshSource(list, request.id));
  } catch (error) {
    state.reject(request, error);
    throw error;
  }
  if (entry(qc, list) !== value) throw new Error("The paired desktop or account changed.");
  if (!Array.isArray(reply)) {
    const currentSession = sessions.get(qc)?.current;
    if ((currentSession !== startedSession && reply.update.session !== currentSession) || !acceptSession(qc, reply.update.session)) throw new Error("The desktop session changed during this request.");
    const previousOwner = owners.get(qc);
    if (reply.update.owner && previousOwner && reply.update.owner.toLowerCase() !== previousOwner.toLowerCase() && !state.allowsOwnership(reply.update)) throw new Error("A newer account receipt is already available.");
    adoptOwner(qc, reply.update.owner ?? undefined);
    const current = entry(qc, list).state;
    if (current !== state) { state = current; request = state.start(request.id); }
  }
  const rows = state.resolve(request, reply);
  if (rows === undefined || state.snapshot().fetchedAt !== undefined) {
    // No in-process receipt is absence, not a measured empty list. Keep the
    // cached query (and its stale marker) while surfacing the provider failure.
    throw new Error(state.snapshot().error ?? "No refreshed snapshot is available yet");
  }
  qc.setQueryData([list === "authored" ? "prs" : "reviewing"], rows);
  return rows;
}

export function patchSourceRows(qc: QueryClient, list: List, patch: (rows: PullRequest[]) => PullRequest[]) {
  const { state } = entry(qc, list);
  state.patchRows(patch);
  return state.snapshot().prs;
}

export async function readRetained(qc: QueryClient, list: List) {
  const value = entry(qc, list);
  const startedSession = sessions.get(qc)?.current;
  const receipt = await getSourceSnapshot({ provider: "github", host: "github.com" }, list);
  if (entry(qc, list) !== value) throw new Error("The paired desktop or account changed.");
  const currentSession = sessions.get(qc)?.current;
  if ((currentSession !== startedSession && receipt.session !== currentSession) || !acceptSession(qc, receipt.session ?? undefined)) throw new Error("The desktop session changed during this request.");
  if (receipt.ownership?.state === "different_account") retireCurrentQueue(qc);
  if (receipt.ownership && "owner" in receipt.ownership) adoptOwner(qc, receipt.ownership.owner);
  const current = entry(qc, list);
  current.state.seed(receipt);
  const snapshot = current.state.snapshot();
  if (snapshot.prs === undefined) throw new Error(snapshot.error ?? "No saved snapshot is available.");
  return { prs: snapshot.prs, stale_secs: snapshot.staleSecs ?? null };
}
export async function readAuthored(qc: QueryClient): Promise<PullRequest[]> {
  const value = entry(qc, "authored");
  try { return (await readRetained(qc, "authored")).prs; }
  catch (error) {
    if (entry(qc, "authored") !== value) throw error;
    return await refreshWithState(qc, "authored");
  }
}


/// The phone's source choice is independent of the desktop poll preference.
/// Fetch upstream on selection, cadence and resume even when SQLite is warm.
export function usePhoneGitHubRefresh(enabled: boolean) {
  const qc = useQueryClient();
  useQuery({
    queryKey: ["phone-github-authored-refresh"],
    queryFn: () => refreshWithState(qc, "authored"),
    enabled: IS_MOBILE_BUILD && enabled,
    staleTime: 0,
    refetchInterval: 60_000,
    refetchOnWindowFocus: "always",
    retry: false,
  });
}
