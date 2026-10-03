import { useEffect } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { advisoryDispatch } from "./advisoryDispatch";
import { getReadyPushers } from "./tauri";
import { useViewer } from "./hooks";
import { useAdvisoryWindow } from "./useAdvisoryWindow";
import { advisoryGcTime, assertCurrent, display, receipt, retainedReceipt, useAdvisorySession, useEvidenceExpiry, type Evidence } from "./advisoryEvidence";
import { prKey } from "@/lib/prIdentity";
import { readyPusher, type ReadyPusher } from "@/lib/readyPusher";
import type { PusherAsk, RowPusher, PullRequest } from "@/types/pr";
interface Answer { row: RowPusher | null; started: number }
interface Receipt { row: RowPusher | null; pusher?: Evidence<RowPusher["last_pusher"]>; rules?: Evidence<RowPusher["rules"]>; expiresAt: number; staleFor: number }
function askOf(pr: PullRequest): PusherAsk {
  return { repo: pr.repo, number: pr.number, base: pr.base_ref, head_repo: pr.head_repo ?? null, head_ref: pr.head_ref, head_oid: pr.head_oid };
}
const keyOf = (pr: PullRequest) => JSON.stringify([prKey(pr), askOf(pr)]);
export function useReadyPushers(prs: PullRequest[], priority: ReadonlySet<string> = new Set()) {
  const qc = useQueryClient();
  const session = useAdvisorySession(qc);
  const viewer = useViewer();
  const owner = typeof viewer.data === "string" ? viewer.data : undefined;
  const rows = prs.filter(pr => !pr.source || (pr.source.provider === "github" && pr.source.host === "github.com"));
  const preferred = new Set(rows.filter(pr => priority.has(prKey(pr)) || rows.length <= 8).map(keyOf));
  const demand = new Set(rows.filter(pr => preferred.has(keyOf(pr)) || !qc.getQueryData<Receipt>(["ready-pushers", owner, session.generation, keyOf(pr)])?.pusher).map(keyOf));
  const window = useAdvisoryWindow(rows.map(keyOf), preferred, !!owner, demand);
  const byKey = new Map(rows.map(pr => [keyOf(pr), pr]));
  const selected = window.selected.flatMap(key => { const pr = byKey.get(key); return pr ? [pr] : []; });
  const keys = selected.map(pr => ["ready-pushers", owner, session.generation, keyOf(pr)]);
  const queries = useQueries({ queries: selected.map((pr, i) => ({
    queryKey: keys[i], queryFn: async ({ signal }: { signal: AbortSignal }): Promise<Receipt> => {
      let answer: Answer;
      try { answer = await advisoryDispatch(qc, JSON.stringify(keys[i]), "pusher", signal, async () => {
        const started = performance.now();
        const ask = askOf(pr);
        const answers = await getReadyPushers([ask]);
        return { started, row: answers?.find(value => value.repo === ask.repo && value.number === ask.number && value.head_oid === ask.head_oid && value.base === ask.base && value.head_ref === ask.head_ref && value.head_repo === ask.head_repo) ?? null };
      }); }
      catch { answer = { row: null, started: performance.now() }; }
      assertCurrent(signal, session.generation, session.current);
      const previous = qc.getQueryData<Receipt>(keys[i]);
      const row = answer.row;
      const pusher = row?.last_pusher.state === "known" ? receipt(row.last_pusher, row.pusher_valid_for_ms, 60_000, answer.started) : undefined;
      const rules = row?.rules.state === "read" ? receipt(row.rules, row.rules_valid_for_ms, 600_000, answer.started) : undefined;
      const expiresAt = pusher && rules ? Math.min(pusher.expiresAt, rules.expiresAt) : performance.now() + 5_000;
      const currentRow: RowPusher | null = row && { ...row,
        last_pusher: row.last_pusher.state === "known" && !pusher ? { state: "declined", reason: "Current pusher lifetime unavailable" } : row.last_pusher,
        rules: row.rules.state === "read" && !rules ? { state: "declined", reason: "Current policy lifetime unavailable" } : row.rules,
      };
      return { row: currentRow, pusher: pusher ?? previous?.pusher ?? retainedReceipt(row?.last_known_pusher, answer.started), rules: rules ?? previous?.rules ?? retainedReceipt(row?.last_known_rules, answer.started), expiresAt, staleFor: Math.max(0, expiresAt - performance.now()) };
    },
    staleTime: (query: { state: { data: Receipt | undefined } }) => query.state.data?.staleFor ?? 0,
    gcTime: advisoryGcTime, retry: false, refetchOnWindowFocus: false,
  })) });
  const signature = JSON.stringify(keys);
  useEffect(() => {
    for (const key of JSON.parse(signature) as string[][]) {
      const query = qc.getQueryCache().find({ queryKey: key, exact: true });
      if (query?.state.data !== undefined && (query.state.data as Receipt).expiresAt <= performance.now()) void qc.invalidateQueries({ queryKey: key, exact: true }, { cancelRefetch: false });
    }
  }, [qc, signature, window.tick]);
  useEvidenceExpiry(queries.flatMap(query => query.data ? [query.data.pusher?.expiresAt, query.data.rules?.expiresAt].filter((at): at is number => at !== undefined) : []), window.visible);
  const read = (pr: PullRequest) => qc.getQueryData<Receipt>(["ready-pushers", owner, session.generation, keyOf(pr)]);
  const of = (pr: PullRequest): ReadyPusher => {
    if (viewer.isError && !owner) return { pusher: { state: "unknown" }, rule: "unread" };
    const value = read(pr);
    const row = value?.row;
    if (!row) return readyPusher(pr, undefined, owner);
    return readyPusher(pr, { ...row,
      last_pusher: row.last_pusher.state === "known" && (!value.pusher || value.pusher.expiresAt <= performance.now()) ? { state: "declined", reason: "Last-known push needs revalidation" } : row.last_pusher,
      rules: row.rules.state === "read" && (!value.rules || value.rules.expiresAt <= performance.now()) ? { state: "declined", reason: "Last-known rules need revalidation" } : row.rules,
    }, viewer.isError ? null : owner);
  };
  const displayOf = (pr: PullRequest) => {
    const value = read(pr);
    if (!value?.pusher) return undefined;
    const pusher = display(value.pusher, value.row?.last_pusher.state === "known");
    const rules = display(value.rules, value.row?.rules.state === "read");
    const row = { ...askOf(pr), last_pusher: value.pusher.value, rules: value.rules?.value ?? { state: "declined" as const, reason: "Unknown policy" } };
    return { value: readyPusher(pr, row, viewer.isError ? null : owner), observedAt: value.pusher.observedAt,
      freshness: pusher?.freshness === "fresh" && rules?.freshness === "fresh" ? "fresh" as const : "retained" as const };
  };
  return { of, displayOf, isPending: rows.length > 0 && (viewer.isPending || queries.some(query => query.isPending)) };
}
