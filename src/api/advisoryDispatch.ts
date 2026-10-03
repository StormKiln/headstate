import type { QueryClient } from "@tanstack/react-query";

type Lane = "detail" | "pusher" | "stack";
interface Pending {
  key: string; lane: Lane; signal: AbortSignal;
  run: () => Promise<void>;
  cancel: (error: DOMException) => void;
  detach: () => void;
}
interface Queue { waiting: Pending[]; running: boolean; scheduled: boolean; next: "pusher" | "stack" }
const queues = new WeakMap<QueryClient, Queue>();
const PENDING_CAPACITY = 64;
const canceled = () => new DOMException("Cancelled", "AbortError");
const deferred = () => new DOMException("Advisory demand deferred while pending work is full", "QuotaExceededError");

/** Only advisory metadata passes here. Query identity coalescing belongs to
 * TanStack; this queue caps waiting work at 64 plus one dispatched command.
 * Native admission remains the meter for actual HTTP fanout/retries. */
export function advisoryDispatch<T>(qc: QueryClient, key: string, lane: Lane, signal: AbortSignal, work: () => Promise<T>): Promise<T> {
  if (signal.aborted) return Promise.reject(canceled());
  let state = queues.get(qc);
  if (!state) { state = { waiting: [], running: false, scheduled: false, next: "pusher" }; queues.set(qc, state); }
  const queue = state;
  return new Promise((resolve, reject) => {
    if (queue.waiting.length >= PENDING_CAPACITY && lane === "detail") {
      // Selected ancestry may replace the newest waiting strip demand, never
      // work already dispatched or an older selected consumer's opportunity.
      for (let i = queue.waiting.length - 1; i >= 0; i--) {
        if (queue.waiting[i].lane === "detail") continue;
        const [victim] = queue.waiting.splice(i, 1);
        victim.cancel(deferred());
        break;
      }
    }
    if (queue.waiting.length >= PENDING_CAPACITY) { reject(deferred()); return; }
    const abort = () => {
      const index = queue.waiting.indexOf(entry);
      if (index < 0) return;
      queue.waiting.splice(index, 1);
      entry.cancel(canceled());
    };
    const entry: Pending = { key, lane, signal,
      run: async () => { try { resolve(await work()); } catch (error) { reject(error); } },
      detach: () => signal.removeEventListener("abort", abort),
      cancel: error => { entry.detach(); reject(error); },
    };
    queue.waiting.push(entry);
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted) abort();
    if (!queue.running && !queue.scheduled) {
      queue.scheduled = true;
      queueMicrotask(() => { queue.scheduled = false; void drain(queue); });
    }
  });
}
export function prioritizeAdvisoryDetail(qc: QueryClient, keys: string[]) {
  const wanted = new Set(keys);
  for (const entry of queues.get(qc)?.waiting ?? []) if (wanted.has(entry.key)) entry.lane = "detail";
}
async function drain(queue: Queue) {
  if (queue.running || !queue.waiting.length) return;
  queue.running = true;
  let turn = queue.next;
  queue.next = queue.next === "pusher" ? "stack" : "pusher";
  try {
    while (queue.waiting.length) {
      let index = queue.waiting.findIndex(entry => entry.lane === "detail");
      if (index < 0) index = queue.waiting.findIndex(entry => entry.lane === turn);
      const [entry] = queue.waiting.splice(index < 0 ? 0 : index, 1);
      entry.detach();
      if (entry.signal.aborted) { entry.cancel(canceled()); continue; }
      if (entry.lane !== "detail") turn = entry.lane === "pusher" ? "stack" : "pusher";
      await entry.run();
    }
  } finally { queue.running = false; }
}
