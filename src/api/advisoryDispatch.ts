import type { QueryClient } from "@tanstack/react-query";
import type { AdvisoryProgress } from "@/types/pr";

interface DispatchOptions<T> {
  preferred: () => boolean;
  rank?: () => number;
  boosted?: () => boolean;
  current: () => boolean;
  progress: (result: T) => AdvisoryProgress | undefined;
  acknowledge?: (progress: AdvisoryProgress | undefined) => void;
}

type Lane = "detail" | "pusher" | "stack";
interface Pending {
  key: string; lane: Lane; signal: AbortSignal; generation: number;
  preferred: () => boolean;
  rank: () => number;
  boosted: () => boolean;
  run: () => Promise<{ rotate: boolean; deferred: boolean }>;
  cancel: (error: DOMException) => void;
  detach: () => void;
}
interface Queue { waiting: Pending[]; running: boolean; scheduled: boolean; cursor: number; generation: number; heldUntil: number; wake?: () => void }
const queues = new WeakMap<QueryClient, Queue>();
const PENDING_CAPACITY = 64;
/** How long strip dispatch holds after the native allowance declined a row.
 *
 * A declined row answers in about a millisecond, so draining on would fire
 * every waiting row into the same spent cycle: a measured desktop log shows
 * 761 local refusals packed into 71 seconds, up to 15 in one second, none of
 * which reached GitHub. Five seconds matches the declined row's own retry. */
export const ADVISORY_DEFER_MS = 5_000;
const canceled = () => new DOMException("Cancelled", "AbortError");
const deferred = () => new DOMException("Advisory demand deferred while pending work is full", "QuotaExceededError");

/** Only advisory metadata passes here. Query identity coalescing belongs to
 * TanStack; this queue caps waiting work at 64 plus one dispatched command.
 * Native admission remains the meter for actual HTTP fanout/retries. */
export function advisoryDispatch<T>(qc: QueryClient, key: string, lane: Lane, signal: AbortSignal, work: () => Promise<T>, options?: DispatchOptions<T>): Promise<T> {
  if (signal.aborted) return Promise.reject(canceled());
  let state = queues.get(qc);
  if (!state) { state = { waiting: [], running: false, scheduled: false, cursor: 0, generation: 0, heldUntil: 0 }; queues.set(qc, state); }
  const queue = state;
  const generation = queue.generation;
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
    const entry: Pending = { key, lane, signal, generation, preferred: options?.preferred ?? (() => true),
      rank: options?.rank ?? (() => -1), boosted: options?.boosted ?? (() => false),
      run: async () => {
        try {
          const result = await work();
          const current = generation === queue.generation && (!options || options.current());
          const progress = options?.progress(result);
          if (current) options?.acknowledge?.(progress);
          resolve(result);
          // Older peers retain best-effort dispatch rotation. Only typed
          // replies can establish current-call provider admission.
          return { rotate: current && (progress ? progress.admitted : true),
            deferred: progress?.admitted === false && progress.outcome === "deferred" };
        } catch (error) {
          const current = generation === queue.generation && (!options || options.current());
          // No row receipt means no admission proof. Keep only the legacy
          // best-effort failure rotation so one transport failure cannot pin
          // the same tail forever; never synthesize an admitted:true reply.
          if (current) options?.acknowledge?.(undefined);
          reject(error); return { rotate: current, deferred: false };
        }
      },
      detach: () => signal.removeEventListener("abort", abort),
      cancel: error => { entry.detach(); reject(error); },
    };
    queue.waiting.push(entry);
    // Selected detail does not wait out a strip hold.
    if (lane === "detail") queue.wake?.();
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
/** Retired native work may finish, but cannot move a replacement owner's ring. */
export function retireAdvisoryDispatch(qc: QueryClient) {
  const queue = queues.get(qc);
  if (!queue) return;
  queue.generation++;
  queue.cursor = 0;
  for (const entry of queue.waiting.splice(0)) entry.cancel(canceled());
  // A retired owner's declined allowance says nothing about the replacement's.
  queue.heldUntil = 0;
  queue.wake?.();
  // Keep the one running native slot occupied until its original reply.
  // Replacement-owner demand waits behind it, within the same 64+1 cap.
}
// The ring reserves actual opportunities for both visible refresh and owed
// offscreen work in both metadata families. Array enqueue order is immaterial.
function cell(entry: Pending) {
  return entry.preferred() ? (entry.lane === "pusher" ? 0 : 2) : (entry.lane === "stack" ? 1 : 3);
}
/** Sleep until the strip hold ends, or sooner when selected detail arrives
 * or the owner is retired. The hold itself is a deadline, so detail served
 * in the middle of it does not end it for the strip. */
function sleep(queue: Queue, ms: number) {
  return new Promise<void>(resolve => {
    const timer = setTimeout(done, ms);
    function done() { clearTimeout(timer); queue.wake = undefined; resolve(); }
    queue.wake = done;
  });
}
async function drain(queue: Queue) {
  if (queue.running || !queue.waiting.length) return;
  queue.running = true;
  try {
    while (queue.waiting.length) {
      let index = queue.waiting.findIndex(entry => entry.lane === "detail");
      const held = queue.heldUntil - Date.now();
      if (index < 0 && held > 0) { await sleep(queue, held); continue; }
      if (index < 0) {
        for (let offset = 0; offset < 4 && index < 0; offset++) {
          const wanted = (queue.cursor + offset) % 4;
          const candidates = queue.waiting.filter(entry => cell(entry) === wanted);
          candidates.sort((a, b) => Number(b.boosted()) - Number(a.boosted()) || a.rank() - b.rank());
          if (candidates.length) index = queue.waiting.indexOf(candidates[0]);
        }
      }
      const [entry] = queue.waiting.splice(index, 1);
      entry.detach();
      if (entry.signal.aborted) { entry.cancel(canceled()); continue; }
      const servedCell = cell(entry);
      const { rotate, deferred } = await entry.run();
      if (rotate && entry.generation === queue.generation && entry.lane !== "detail") queue.cursor = (servedCell + 1) % 4;
      if (deferred && entry.generation === queue.generation) queue.heldUntil = Date.now() + ADVISORY_DEFER_MS;
    }
  } finally { queue.running = false; }
}
