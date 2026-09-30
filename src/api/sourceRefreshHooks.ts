import { useEffect, useSyncExternalStore } from "react";
import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import type { PullRequest } from "../types/pr";
import { SourceRefreshState, type SourceStatus } from "./sourceRefresh";
import { getCached, refreshSource } from "./tauri";
import { listen, type UnlistenFn } from "./transport";
import { safeUnlisten } from "./unlisten";
import { IS_MOBILE_BUILD } from "../lib/target";
import { timeCall } from "./diag";

type List = "authored" | "reviewing";
type Entry = { state: SourceRefreshState; users: number; stop?: () => void };
const entries = new WeakMap<QueryClient, Partial<Record<List, Entry>>>();
const authored = new Set<SourceRefreshState>();
let nextRequest = 0;
// Unique across webviews/phone sessions, not persisted or used as credentials.
const requestPrefix = crypto.randomUUID();
function entry(qc: QueryClient, list: List): Entry {
  let lists = entries.get(qc);
  if (!lists) { lists = {}; entries.set(qc, lists); }
  return lists[list] ??= { state: new SourceRefreshState(), users: 0 };
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
    if (next !== undefined && next !== rows) {
      rows = next;
      qc.setQueryData([list === "authored" ? "prs" : "reviewing"], next);
    }
  });
  register(listen<SourceStatus>("source-poll-status", ({ payload }) => {
    if (payload.source.provider === "github" && payload.source.host === "github.com" && payload.list === list) state.accept(payload);
  }));
  register(listen<PullRequest[]>(list === "authored" ? "prs-updated" : "reviewing-updated", ({ payload }) => state.legacyRows(payload)));
  if (list === "authored") register(listen<string>("poll-error", ({ payload }) => state.legacyError(payload)));
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
  const { state } = entry(qc, list);
  const request = state.start(`${requestPrefix}:${++nextRequest}`);
  let reply;
  try {
    reply = await timeCall(list === "authored" ? "prs" : "reviewing", () => refreshSource(list, request.id));
  } catch (error) {
    state.reject(request, error);
    throw error;
  }
  const rows = state.resolve(request, reply);
  if (rows === undefined) {
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

export async function readAuthored(qc: QueryClient): Promise<PullRequest[]> {
  const { state } = entry(qc, "authored");
  const cached = await getCached();
  return state.snapshot().prs ?? (cached.length > 0 ? cached : await refreshWithState(qc, "authored"));
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
