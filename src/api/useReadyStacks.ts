import { useEffect } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { advisoryDispatch, prioritizeAdvisoryDetail } from "./advisoryDispatch";
import { getReadyStacks } from "./tauri";
import { useViewer } from "./hooks";
import { useAdvisoryWindow } from "./useAdvisoryWindow";
import { advisoryGcTime, assertCurrent, display, receipt, retainedReceipt, useAdvisorySession, useEvidenceExpiry, type Evidence } from "./advisoryEvidence";
import { prIdentity, prKey } from "@/lib/prIdentity";
import type { PrStack, PullRequest } from "@/types/pr";

interface Receipt { stack: PrStack; expiresAt: number; staleFor: number; lastKnown?: Evidence<PrStack> }
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
  const demand = new Set(rows.filter(pr => preferred.has(keyOf(pr)) || !qc.getQueryData<Receipt>(["ready-stack", owner, session.generation, keyOf(pr)])?.lastKnown).map(keyOf));
  const window = useAdvisoryWindow(rows.map(keyOf), preferred, enabled && !!owner, demand);
  const byKey = new Map(rows.map(pr => [keyOf(pr), pr]));
  const selected = window.selected.flatMap(key => { const pr = byKey.get(key); return pr ? [pr] : []; });
  const keys = selected.map(pr => ["ready-stack", owner, session.generation, keyOf(pr)]);
  useQueries({ queries: selected.map((pr, i) => ({
    queryKey: keys[i],
    queryFn: async ({ signal }: { signal: AbortSignal }) => {
      let answer: Receipt;
      try { answer = await advisoryDispatch(qc, JSON.stringify(keys[i]), consumer === "detail" ? "detail" : "stack", signal, async () => {
        const started = performance.now();
        const answers = await getReadyStacks([{ ...prIdentity(pr), head_oid: pr.head_oid, base_ref: pr.base_ref }]);
        const value = answers?.find(value => prKey(value) === prKey(pr) && value.head_oid === pr.head_oid && value.base_ref === pr.base_ref);
        const known = value?.stack.kind !== "unknown" && value ? receipt(value.stack, value.valid_for_ms, 60_000, started) : undefined;
        return known ? { stack: known.value, expiresAt: known.expiresAt, staleFor: Math.max(0, known.expiresAt - performance.now()), lastKnown: known }
          : { stack: { kind: "unknown" }, expiresAt: performance.now() + 5_000, staleFor: 5_000, lastKnown: retainedReceipt(value?.last_known_stack, started) };
      }); }
      catch { answer = { stack: { kind: "unknown" }, expiresAt: performance.now() + 5_000, staleFor: 5_000 }; }
      assertCurrent(signal, session.generation, session.current);
      return { ...answer, lastKnown: answer.lastKnown ?? qc.getQueryData<Receipt>(keys[i])?.lastKnown };
    },
    staleTime: (query: { state: { data: Receipt | undefined } }) => query.state.data?.staleFor ?? 0,
    gcTime: advisoryGcTime,
    retry: false,
    refetchOnWindowFocus: false,
  })) });
  const signature = JSON.stringify(keys);
  useEffect(() => {
    const now = performance.now();
    if (consumer === "detail") prioritizeAdvisoryDetail(qc, (JSON.parse(signature) as unknown[][]).map(key => JSON.stringify(key)));
    for (const key of JSON.parse(signature) as string[][]) {
      const receipt = qc.getQueryData<Receipt>(key);
      if (receipt && receipt.expiresAt <= now) void qc.invalidateQueries({ queryKey: key, exact: true }, { cancelRefetch: false });
    }
  }, [qc, signature, window.tick, consumer]);
  // Display qualification outlives the bounded network observer window.
  useEvidenceExpiry(() => rows.flatMap(pr => {
    const value = qc.getQueryData<Receipt>(["ready-stack", owner, session.generation, keyOf(pr)]);
    return value ? [value.expiresAt] : [];
  }), window.visible && enabled);
  return { of: (pr: StackSubject): PrStack | undefined => {
    const receipt = qc.getQueryData<Receipt>(["ready-stack", owner, session.generation, keyOf(pr)]);
    return receipt && receipt.expiresAt > performance.now() ? receipt.stack : undefined;
  }, displayOf: (pr: StackSubject) => {
    const value = qc.getQueryData<Receipt>(["ready-stack", owner, session.generation, keyOf(pr)]);
    return display(value?.lastKnown, value?.stack.kind !== "unknown");
  } };
}
