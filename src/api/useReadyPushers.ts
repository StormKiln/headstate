import { useEffect } from "react";
import { useQueries, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { getReadyPushers } from "./tauri";
import { useViewer } from "./hooks";
import { useAdvisoryWindow } from "./useAdvisoryWindow";
import { prKey } from "@/lib/prIdentity";
import { readyPusher, type ReadyPusher } from "@/lib/readyPusher";
import type { PusherAsk, RowPusher, PullRequest } from "@/types/pr";
interface Pending { ask: PusherAsk; signal: AbortSignal; resolve: (value: RowPusher | null) => void; reject: (e: unknown) => void }
const queues = new WeakMap<QueryClient, { waiting: Pending[]; running: boolean }>();
function load(qc: QueryClient, ask: PusherAsk, signal: AbortSignal): Promise<RowPusher | null> {
  let state = queues.get(qc);
  if (!state) { state = { waiting: [], running: false }; queues.set(qc, state); }
  const queue = state;
  return new Promise((resolve, reject) => {
    queue.waiting.push({ ask, signal, resolve, reject });
    queueMicrotask(() => { void drain(); });
    async function drain() {
      if (queue.running) return;
      queue.running = true;
      try {
        while (queue.waiting.length) {
          const batch = queue.waiting.splice(0, 8).filter(entry => {
            if (!entry.signal.aborted) return true;
            entry.reject(new DOMException("Cancelled", "AbortError"));
            return false;
          });
          if (!batch.length) continue;
          try {
            const answers = await getReadyPushers(batch.map(entry => entry.ask));
            for (const entry of batch) entry.resolve(answers?.find(value => value.repo === entry.ask.repo && value.number === entry.ask.number
              && value.head_oid === entry.ask.head_oid && value.base === entry.ask.base && value.head_ref === entry.ask.head_ref && value.head_repo === entry.ask.head_repo) ?? null);
          } catch (error) { for (const entry of batch) entry.reject(error); }
        }
      } finally { queue.running = false; }
    }
  });
}
function askOf(pr: PullRequest): PusherAsk {
  return { repo: pr.repo, number: pr.number, base: pr.base_ref, head_repo: pr.head_repo ?? null, head_ref: pr.head_ref, head_oid: pr.head_oid };
}
const keyOf = (pr: PullRequest) => JSON.stringify([prKey(pr), askOf(pr)]);
const ttl = (value: RowPusher | null | undefined) => value?.last_pusher.state === "known" && value.rules.state === "read" ? 60_000 : 5_000;
export function useReadyPushers(prs: PullRequest[], priority: ReadonlySet<string> = new Set()) {
  const qc = useQueryClient();
  const viewer = useViewer();
  const owner = typeof viewer.data === "string" ? viewer.data : undefined;
  const rows = prs.filter(pr => !pr.source || (pr.source.provider === "github" && pr.source.host === "github.com"));
  const window = useAdvisoryWindow(rows.map(keyOf), new Set(rows.filter(pr => priority.has(prKey(pr))).map(keyOf)), !!owner);
  const selected = rows.filter(pr => window.selected.includes(keyOf(pr)));
  const keys = selected.map(pr => ["ready-pushers", owner, keyOf(pr)]);
  const queries = useQueries({ queries: selected.map((pr, i) => ({
    queryKey: keys[i], queryFn: ({ signal }: { signal: AbortSignal }) => load(qc, askOf(pr), signal),
    staleTime: (query: { state: { data: RowPusher | null | undefined } }) => ttl(query.state.data),
    gcTime: 5 * 60_000, retry: false, refetchOnWindowFocus: false,
  })) });
  const signature = JSON.stringify(keys);
  useEffect(() => {
    for (const key of JSON.parse(signature) as string[][]) {
      const query = qc.getQueryCache().find({ queryKey: key, exact: true });
      if (query?.state.data !== undefined && query.isStaleByTime(ttl(query.state.data as RowPusher | null))) void qc.invalidateQueries({ queryKey: key, exact: true }, { cancelRefetch: false });
    }
  }, [qc, signature, window.tick]);
  const active = new Map(selected.map((pr, i) => [keyOf(pr), queries[i].data]));
  const of = (pr: PullRequest): ReadyPusher => {
    if (viewer.isError && !owner) return { pusher: { state: "unknown" }, rule: "unread" };
    const key = ["ready-pushers", owner, keyOf(pr)];
    const state = qc.getQueryState<RowPusher | null>(key);
    const value = state && Date.now() - state.dataUpdatedAt < ttl(state.data)
      ? active.get(keyOf(pr)) ?? state.data : undefined;
    return readyPusher(pr, value ?? undefined, viewer.isError ? null : owner);
  };
  return { of, isPending: rows.length > 0 && (viewer.isPending || queries.some(query => query.isPending)) };
}
