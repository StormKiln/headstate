import { useEffect, useReducer } from "react";
import { useQueries, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { getReadyStacks, type StackAsk } from "./tauri";
import { useViewer } from "./hooks";
import { useAdvisoryWindow } from "./useAdvisoryWindow";
import { prIdentity, prKey } from "@/lib/prIdentity";
import type { PrStack, PullRequest } from "@/types/pr";

interface Receipt { stack: PrStack; expiresAt: number; staleFor: number }
interface Pending {
  ask: StackAsk;
  signal: AbortSignal;
  resolve: (receipt: Receipt) => void;
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
        // Anchor to dispatch, never arrival: a cached native receipt may already
        // be old, and IPC/batching delay must not renew its original lifetime.
        const started = performance.now();
        const answers = await getReadyStacks(batch.map(entry => entry.ask));
        for (const entry of batch) {
          const answer = answers?.find(answer => prKey(answer) === prKey(entry.ask)
            && answer.head_oid === entry.ask.head_oid && answer.base_ref === entry.ask.base_ref);
          const ttl = answer?.valid_for_ms;
          const valid = typeof ttl === "number" && Number.isFinite(ttl) && ttl > 0 && ttl <= 60_000;
          entry.resolve(valid && answer ? { stack: answer.stack, expiresAt: started + ttl, staleFor: Math.max(0, started + ttl - performance.now()) }
            : { stack: { kind: "unknown" }, expiresAt: performance.now() + 5_000, staleFor: 5_000 });
        }
      } catch (error) { for (const entry of batch) entry.reject(error); }
    }
  } finally { queue.running = false; }
}
function load(qc: QueryClient, ask: StackAsk, signal: AbortSignal): Promise<Receipt> {
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
    staleTime: (query: { state: { data: Receipt | undefined } }) => query.state.data?.staleFor ?? 0,
    gcTime: 5 * 60_000,
    retry: false,
    refetchOnWindowFocus: false,
  })) });
  const signature = JSON.stringify(keys);
  useEffect(() => {
    for (const key of JSON.parse(signature) as string[][]) {
      const query = qc.getQueryCache().find({ queryKey: key, exact: true });
      const receipt = query?.state.data as Receipt | undefined;
      if (receipt && receipt.expiresAt <= performance.now()) void qc.invalidateQueries({ queryKey: key, exact: true }, { cancelRefetch: false });
    }
  }, [qc, signature, window.tick]);
  // Expiry changes action eligibility even if no other query causes a render.
  // At most the bounded advisory window owns timers; expiry itself makes no RPC.
  const [, expire] = useReducer(value => value + 1, 0);
  const expiries = JSON.stringify(queries.map(query => query.data?.expiresAt).filter(value => value !== undefined));
  useEffect(() => {
    if (!window.visible || !enabled) return;
    const timers = (JSON.parse(expiries) as number[]).filter(at => at >= performance.now())
      .map(at => setTimeout(expire, Math.max(0, at - performance.now()) + 1));
    return () => { for (const timer of timers) clearTimeout(timer); };
  }, [expiries, enabled, window.visible]);
  return { of: (pr: StackSubject): PrStack | undefined => {
    const receipt = qc.getQueryData<Receipt>(["ready-stack", owner, keyOf(pr)]);
    return receipt && receipt.expiresAt > performance.now() ? receipt.stack : undefined;
  } };
}
