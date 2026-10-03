import type { QueryClient } from "@tanstack/react-query";

type Lane = "detail" | "pusher" | "stack";
interface Pending { key: string; lane: Lane; signal: AbortSignal; run: () => Promise<void>; cancel: () => void }
interface Queue { waiting: Pending[]; running: boolean; next: "pusher" | "stack" }
const queues = new WeakMap<QueryClient, Queue>();

/** Only advisory metadata passes here. Native admission remains the meter for
 * actual HTTP fanout/retries. One-row dispatch prevents an eight-row batch
 * from spending the entire cycle before another consumer can ask. */
export function advisoryDispatch<T>(qc: QueryClient, key: string, lane: Lane, signal: AbortSignal, work: () => Promise<T>): Promise<T> {
  let state = queues.get(qc);
  if (!state) { state = { waiting: [], running: false, next: "pusher" }; queues.set(qc, state); }
  const queue = state;
  return new Promise((resolve, reject) => {
    queue.waiting.push({ key, lane, signal,
      run: async () => { try { resolve(await work()); } catch (error) { reject(error); } },
      cancel: () => reject(new DOMException("Cancelled", "AbortError")),
    });
    queueMicrotask(() => { void drain(queue); });
  });
}
export function prioritizeAdvisoryDetail(qc: QueryClient, keys: string[]) {
  const wanted = new Set(keys);
  for (const entry of queues.get(qc)?.waiting ?? []) if (wanted.has(entry.key)) entry.lane = "detail";
}
async function drain(queue: Queue) {
  if (queue.running) return;
  queue.running = true;
  let turn = queue.next;
  queue.next = queue.next === "pusher" ? "stack" : "pusher";
  try {
    while (queue.waiting.length) {
      queue.waiting = queue.waiting.filter(entry => { if (!entry.signal.aborted) return true; entry.cancel(); return false; });
      if (!queue.waiting.length) break;
      let index = queue.waiting.findIndex(entry => entry.lane === "detail");
      if (index < 0) index = queue.waiting.findIndex(entry => entry.lane === turn);
      const [entry] = queue.waiting.splice(index < 0 ? 0 : index, 1);
      if (entry.lane !== "detail") turn = entry.lane === "pusher" ? "stack" : "pusher";
      await entry.run();
    }
  } finally { queue.running = false; }
}
