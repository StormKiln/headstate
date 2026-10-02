import { useEffect } from "react";
import { useQueries, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { getReadyStacks, type StackAsk } from "./tauri";
import { useViewer } from "./hooks";
import { useAdvisoryWindow } from "./useAdvisoryWindow";
import { prIdentity, prKey } from "@/lib/prIdentity";
import type { PrStack, PullRequest } from "@/types/pr";

interface Pending {
  ask: StackAsk;
  signal: AbortSignal;
  resolve: (stack: PrStack) => void;
  reject: (error: unknown) => void;
}
interface Queue { waiting: Pending[]; running: boolean }
const queues = new WeakMap<QueryClient, Queue>();
async function drain(queue: Queue) {
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
        const answers = await getReadyStacks(batch.map(entry => entry.ask));
        for (const entry of batch) {
          const answer = answers?.find(answer => prKey(answer) === prKey(entry.ask)
            && answer.head_oid === entry.ask.head_oid && answer.base_ref === entry.ask.base_ref);
          entry.resolve(answer?.stack ?? { kind: "unknown" });
        }
      } catch (error) { for (const entry of batch) entry.reject(error); }
    }
  } finally { queue.running = false; }
}
function load(qc: QueryClient, ask: StackAsk, signal: AbortSignal): Promise<PrStack> {
  let queue = queues.get(qc);
  if (!queue) { queue = { waiting: [], running: false }; queues.set(qc, queue); }
  const owner = queue;
  return new Promise((resolve, reject) => {
    owner.waiting.push({ ask, signal, resolve, reject });
    queueMicrotask(() => { void drain(owner); });
  });
}
type StackSubject = Pick<PullRequest, "source" | "repo" | "number" | "head_oid" | "base_ref">;
const keyOf = (pr: StackSubject) => JSON.stringify([prKey(pr), pr.head_oid, pr.base_ref]);
/** Also the selected-detail consumer: pass only the displayed full row. */
export function useReadyStacks(prs: StackSubject[], priority: ReadonlySet<string> = new Set(), enabled = true) {
  const qc = useQueryClient();
  const viewer = useViewer();
  const owner = typeof viewer.data === "string" ? viewer.data : undefined;
  const rows = prs.filter(pr => !pr.source || (pr.source.provider === "github" && pr.source.host === "github.com"));
  const window = useAdvisoryWindow(rows.map(keyOf), new Set(rows.filter(pr => priority.has(prKey(pr))).map(keyOf)), enabled && !!owner);
  const selected = rows.filter(pr => window.selected.includes(keyOf(pr)));
  const keys = selected.map(pr => ["ready-stack", owner, keyOf(pr)]);
  const queries = useQueries({ queries: selected.map((pr, i) => ({
    queryKey: keys[i],
    queryFn: ({ signal }: { signal: AbortSignal }) => load(qc, { ...prIdentity(pr), head_oid: pr.head_oid, base_ref: pr.base_ref }, signal),
    staleTime: (query: { state: { data: PrStack | undefined } }) => query.state.data?.kind === "unknown" ? 5_000 : 60_000,
    gcTime: 5 * 60_000,
    retry: false,
    refetchOnWindowFocus: false,
  })) });
  const signature = JSON.stringify(keys);
  useEffect(() => {
    for (const key of JSON.parse(signature) as string[][]) {
      const query = qc.getQueryCache().find({ queryKey: key, exact: true });
      const ttl = (query?.state.data as PrStack | undefined)?.kind === "unknown" ? 5_000 : 60_000;
      if (query?.state.data !== undefined && query.isStaleByTime(ttl)) void qc.invalidateQueries({ queryKey: key, exact: true }, { cancelRefetch: false });
    }
  }, [qc, signature, window.tick]);
  const active = new Map(selected.map((pr, i) => [keyOf(pr), queries[i].data]));
  return { of: (pr: StackSubject): PrStack | undefined => {
    const key = ["ready-stack", owner, keyOf(pr)];
    const state = qc.getQueryState<PrStack>(key);
    const ttl = state?.data?.kind === "unknown" ? 5_000 : 60_000;
    if (!state || Date.now() - state.dataUpdatedAt >= ttl) return undefined;
    return active.get(keyOf(pr)) ?? state.data;
  } };
}
