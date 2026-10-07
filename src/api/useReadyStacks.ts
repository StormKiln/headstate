import { useLayoutEffect } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { advisoryDispatch, prioritizeAdvisoryDetail } from "./advisoryDispatch";
import { getReadyStacks } from "./tauri";
import { useViewer } from "./hooks";
import { useAdvisoryWindow } from "./useAdvisoryWindow";
import { acknowledgeSchedule, isPreferred, isSelected, readSchedule, scheduleMeta, useScheduleClaims } from "./advisorySchedule";
import { advisoryGcTime, assertCurrent, display, receipt, retainedReceipt, useAdvisorySession, useEvidenceExpiry, type Evidence } from "./advisoryEvidence";
import { prIdentity, prKey } from "@/lib/prIdentity";
import type { PrStack, PullRequest } from "@/types/pr";

interface Receipt { selected?: boolean; stack: PrStack; expiresAt: number; staleFor: number; lastKnown?: Evidence<PrStack> }
type StackSubject = Pick<PullRequest, "source" | "repo" | "number" | "head_oid" | "base_ref">;
const keyOf = (pr: StackSubject) => JSON.stringify([prKey(pr), pr.head_oid, pr.base_ref]);
/** Also the selected-detail consumer: pass only the displayed full row. */
export function useReadyStacks(prs: StackSubject[], priority: ReadonlySet<string> = new Set(), enabled = true, consumer: "strip" | "detail" = "strip") {
  const qc = useQueryClient();
  const session = useAdvisorySession(qc);
  const viewer = useViewer();
  const owner = typeof viewer.data === "string" ? viewer.data : undefined;
  const rows = prs.filter(pr => !pr.source || (pr.source.provider === "github" && pr.source.host === "github.com"));
  const preferred = new Set(rows.filter(pr => priority.has(prKey(pr)) || rows.length <= 8).map(keyOf));
  const queryKey = (key: string) => ["ready-stack", owner, session.generation, key];
  const schedule = (key: string) => readSchedule(qc, queryKey(key));
  const demand = new Set(rows.filter(pr => preferred.has(keyOf(pr)) || !qc.getQueryData<Receipt>(queryKey(keyOf(pr)))?.lastKnown || schedule(keyOf(pr))?.hasContinuation).map(keyOf));
  const window = useAdvisoryWindow(rows.map(keyOf), preferred, enabled && !!owner, demand, schedule);
  const byKey = new Map(rows.map(pr => [keyOf(pr), pr]));
  const selected = window.selected.flatMap(key => { const pr = byKey.get(key); return pr ? [pr] : []; });
  const keys = selected.map(pr => ["ready-stack", owner, session.generation, keyOf(pr)]);
  const metas = keys.map(key => scheduleMeta(qc, key));
  useScheduleClaims(selected.map((pr, i) => ({ meta: metas[i], preferred: window.preferred.has(keyOf(pr)), selected: consumer === "detail" })));
  const queries = useQueries({ queries: selected.map((pr, i) => ({
    queryKey: keys[i], meta: metas[i],
    queryFn: async ({ signal }: { signal: AbortSignal }) => {
      let answer: Receipt;
      let completed = false;
      let selectedAttempt = false;
      try { answer = (await advisoryDispatch(qc, JSON.stringify(keys[i]), consumer === "detail" ? "detail" : "stack", signal, async () => {
        const started = performance.now();
        const selected = selectedAttempt = isSelected(metas[i]);
        const answers = await getReadyStacks([{ ...prIdentity(pr), head_oid: pr.head_oid, base_ref: pr.base_ref }], selected);
        const matching = answers?.find(value => prKey(value) === prKey(pr));
        const value = matching?.head_oid === pr.head_oid && matching.base_ref === pr.base_ref ? matching : undefined;
        const known = value?.stack.kind !== "unknown" && value ? receipt(value.stack, value.valid_for_ms, 60_000, started) : undefined;
        const result: Receipt = known ? { stack: known.value, expiresAt: known.expiresAt, staleFor: Math.max(0, known.expiresAt - performance.now()), lastKnown: known }
          : { stack: { kind: "unknown" }, expiresAt: performance.now() + 5_000, staleFor: 5_000, lastKnown: retainedReceipt(value?.last_known_stack, started) };
        return { receipt: { ...result, selected }, progress: matching?.advisory_progress };
      }, {
        preferred: () => isPreferred(metas[i], window.preferred.has(keyOf(pr))),
        rank: () => metas[i].advisorySchedule.lastAdmittedAt ?? -1,
        boosted: () => window.boosted.has(keyOf(pr)) && !metas[i].advisorySchedule.resumeBoostSpent,
        current: () => session.generation === session.current(),
        progress: value => { completed = value.receipt.stack.kind !== "unknown" && value.receipt.expiresAt > performance.now() && value.progress?.outcome === "offered"; return value.progress; },
        acknowledge: progress => acknowledgeSchedule(metas[i], progress, window.boosted.has(keyOf(pr)), completed),
      })).receipt; }
      catch { answer = { selected: selectedAttempt, stack: { kind: "unknown" }, expiresAt: performance.now() + 5_000, staleFor: 5_000 }; }
      assertCurrent(signal, session.generation, session.current);
      return { ...answer, lastKnown: answer.lastKnown ?? qc.getQueryData<Receipt>(keys[i])?.lastKnown };
    },
    staleTime: (query: { state: { data: Receipt | undefined } }) => query.state.data?.staleFor ?? 0,
    gcTime: advisoryGcTime,
    retry: false,
    refetchOnWindowFocus: false,
  })) });
  const signature = JSON.stringify(keys);
  useLayoutEffect(() => {
    const now = performance.now();
    if (consumer === "detail") prioritizeAdvisoryDetail(qc, (JSON.parse(signature) as unknown[][]).map(key => JSON.stringify(key)));
    for (const key of JSON.parse(signature) as string[][]) {
      const receipt = qc.getQueryData<Receipt>(key);
      const missingSelected = consumer === "detail" && (!receipt || receipt.stack.kind === "unknown");
      if (missingSelected || (receipt && (receipt.expiresAt <= now || readSchedule(qc, key)?.hasContinuation))) {
        // Joining a dispatched strip read cannot change its native admission.
        // Await that shared result, then allow only one selected follow-up.
        void qc.invalidateQueries({ queryKey: key, exact: true }, { cancelRefetch: false }).then(() => {
          const current = qc.getQueryData<Receipt>(key);
          const meta = scheduleMeta(qc, key);
          if (consumer === "detail" && isSelected(meta) && current && !current.selected && current.stack.kind === "unknown") {
            void qc.invalidateQueries({ queryKey: key, exact: true }, { cancelRefetch: false });
          }
        });
      }
    }
  }, [qc, signature, window.tick, consumer]);
  // Display qualification outlives the bounded network observer window.
  useEvidenceExpiry(() => rows.flatMap(pr => {
    const value = qc.getQueryData<Receipt>(["ready-stack", owner, session.generation, keyOf(pr)]);
    return value ? [value.expiresAt] : [];
  }), window.visible && enabled);
  return {
    isFetching: queries.some(query => query.isFetching),
    // Only the observed window; never invalidate the whole account's stacks.
    refetch: () => Promise.all(queries.map(query => query.refetch({ cancelRefetch: false }))),
    of: (pr: StackSubject): PrStack | undefined => {
    const receipt = qc.getQueryData<Receipt>(["ready-stack", owner, session.generation, keyOf(pr)]);
    return receipt && receipt.expiresAt > performance.now() ? receipt.stack : undefined;
  }, displayOf: (pr: StackSubject) => {
    const value = qc.getQueryData<Receipt>(["ready-stack", owner, session.generation, keyOf(pr)]);
    return display(value?.lastKnown, value?.stack.kind !== "unknown");
  } };
}
