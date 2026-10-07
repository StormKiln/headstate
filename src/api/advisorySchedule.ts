import { useId, useLayoutEffect } from "react";
import type { QueryClient } from "@tanstack/react-query";
import type { AdvisoryProgress } from "@/types/pr";

/** Query-owned scheduling state. Mutating it never changes evidence timestamps,
 * dataUpdatedAt, or freshness; query removal/owner retirement removes it. */
export interface AdvisorySchedule {
  lastAdmittedAt?: number;
  hasContinuation?: boolean;
  resumeBoostSpent?: boolean;
  ineligible?: boolean;
}
interface State extends AdvisorySchedule { claims: Map<string, boolean>; selectedClaims: Set<string> }
interface Meta extends Record<string, unknown> { advisorySchedule: State }
export function scheduleMeta(qc: QueryClient, key: readonly unknown[]): Meta {
  const meta = qc.getQueryCache().get(qc.defaultQueryOptions({ queryKey: key }).queryHash)?.meta;
  return meta?.advisorySchedule ? meta as Meta : { ...meta, advisorySchedule: { claims: new Map(), selectedClaims: new Set() } };
}
export function readSchedule(qc: QueryClient, key: readonly unknown[]): AdvisorySchedule | undefined {
  return qc.getQueryCache().get(qc.defaultQueryOptions({ queryKey: key }).queryHash)?.meta?.advisorySchedule as AdvisorySchedule | undefined;
}
export function acknowledgeSchedule(meta: Meta, progress: AdvisoryProgress | undefined, boosted: boolean, complete: boolean) {
  const state = meta.advisorySchedule;
  // Untyped peers retain best-effort dispatch rotation, without an admission
  // guarantee. A typed declined/cache-only reply never settles admission debt.
  if (!progress || progress.admitted) state.lastAdmittedAt = performance.now();
  if (!progress) return;
  state.ineligible = progress.outcome === "ineligible";
  state.hasContinuation = progress.outcome === "partial";
  if (progress.admitted && boosted) state.resumeBoostSpent = true;
  // A failure or expired continuation does not replenish an expedited turn.
  if (complete) state.resumeBoostSpent = false;
}
export function isSelected(meta: Meta) { return meta.advisorySchedule.selectedClaims.size > 0; }
export function isPreferred(meta: Meta, fallback: boolean) {
  return meta.advisorySchedule.claims.size ? [...meta.advisorySchedule.claims.values()].some(Boolean) : fallback;
}
/** A visible observer wins over an offscreen observer of the same query. Claims
 * live only for mounted finite windows and are removed before replacement. */
export function useScheduleClaims(entries: { meta: Meta; preferred: boolean; selected?: boolean }[]) {
  const consumer = useId();
  useLayoutEffect(() => {
    for (const entry of entries) {
      entry.meta.advisorySchedule.claims.set(consumer, entry.preferred);
      if (entry.selected) entry.meta.advisorySchedule.selectedClaims.add(consumer);
    }
    return () => { for (const entry of entries) { entry.meta.advisorySchedule.claims.delete(consumer); entry.meta.advisorySchedule.selectedClaims.delete(consumer); } };
  });
}
