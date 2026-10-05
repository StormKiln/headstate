import { useEffect, useReducer, useSyncExternalStore } from "react";
import type { QueryClient } from "@tanstack/react-query";
import { retireAdvisoryDispatch } from "./advisoryDispatch";

export interface Evidence<T> { value: T; expiresAt: number; observedAt: number }
export interface DisplayEvidence<T> { value: T; freshness: "fresh" | "retained"; observedAt: number }
// Capacity, not elapsed browsing time, bounds retained display memory.
export const advisoryGcTime = Infinity;
const isAdvisory = (key: readonly unknown[]) => key[0] === "ready-stack" || key[0] === "ready-pushers" || key[0] === "review-gates";

// Only session control lives outside query state. Query reset/removal owns all
// evidence, including last successes. A login round trip is a new generation.
const sessions = new WeakMap<QueryClient, ReturnType<typeof createSession>>();
function createSession(qc: QueryClient) {
  let generation = 0;
  let owner = qc.getQueryData(["viewer"]);
  const listeners = new Set<() => void>();
  qc.getQueryCache().subscribe(event => {
    if (event.query.queryKey[0] === "viewer" && (event.type === "removed" || event.type === "updated")) {
      const next = event.type === "removed" ? undefined : event.query.state.data;
      const reset = event.type === "updated" && event.action.type === "setState";
      if (next !== owner || reset || event.type === "removed") {
        owner = next;
        generation++;
        retireAdvisoryDispatch(qc);
        qc.removeQueries({ predicate: query => isAdvisory(query.queryKey) });
        for (const listener of listeners) listener();
      }
    }
    if (event.type === "added" && isAdvisory(event.query.queryKey)) {
      // The window bounds active observers. Evict oldest inactive identities
      // incrementally, never clear an entire useful population on one new row.
      const peers = qc.getQueryCache().findAll({ queryKey: [event.query.queryKey[0]] });
      const victims = peers.filter(query => query !== event.query && query.getObserversCount() === 0)
        .sort((a, b) => a.state.dataUpdatedAt - b.state.dataUpdatedAt);
      for (let i = 512; i < peers.length; i++) {
        const victim = victims.shift() ?? peers.filter(query => query !== event.query).sort((a, b) => a.state.dataUpdatedAt - b.state.dataUpdatedAt)[0];
        if (victim) qc.getQueryCache().remove(victim);
      }
    }
  });
  return { current: () => generation, subscribe: (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; } };
}
export function useAdvisorySession(qc: QueryClient) {
  let session = sessions.get(qc);
  if (!session) { session = createSession(qc); sessions.set(qc, session); }
  const generation = useSyncExternalStore(session.subscribe, session.current);
  return { generation, current: session.current };
}
export function receipt<T>(value: T, validFor: number | undefined, maximum: number, started: number): Evidence<T> | undefined {
  if (typeof validFor !== "number" || !Number.isFinite(validFor) || validFor < 0 || validFor > maximum) return undefined;
  return { value, expiresAt: started + validFor, observedAt: Date.now() - (performance.now() - started) - (maximum - validFor) };
}
export function display<T>(value: Evidence<T> | undefined, confirmed = true): DisplayEvidence<T> | undefined {
  return value && { value: value.value, observedAt: value.observedAt, freshness: confirmed && value.expiresAt > performance.now() ? "fresh" : "retained" };
}
export function assertCurrent(signal: AbortSignal, generation: number, current: () => number) {
  if (signal.aborted || generation !== current()) throw new DOMException("Cancelled", "AbortError");
}
/** Expiry updates display and action eligibility, without issuing requests. */
export function useEvidenceExpiry(expiries: () => number[], enabled: boolean) {
  const [revision, expire] = useReducer(value => value + 1, 0);
  useEffect(() => {
    if (!enabled) return;
    const now = performance.now();
    const next = Math.min(...expiries().filter(at => at >= now));
    if (!Number.isFinite(next)) return;
    const timer = setTimeout(expire, Math.max(0, next - now) + 1);
    return () => clearTimeout(timer);
  }, [expiries, enabled, revision]);
}

/** Native history is display-only, including a success retained after failure. */
export function retainedReceipt<T>(known: { value: T; age_ms: number } | undefined, started: number): Evidence<T> | undefined {
  if (!known || !Number.isFinite(known.age_ms) || known.age_ms < 0) return undefined;
  return { value: known.value, expiresAt: started, observedAt: Date.now() - (performance.now() - started) - known.age_ms };
}
