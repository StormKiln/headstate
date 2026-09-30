import { useQueries } from "@tanstack/react-query";
import { getReadyStacks } from "./tauri";
import { prIdentity, prKey } from "@/lib/prIdentity";
import type { PrIdentity } from "@/types/identity";
import type { PrStack, PullRequest } from "@/types/pr";

interface Pending {
  pr: PrIdentity;
  signal: AbortSignal;
  resolve: (stack: PrStack) => void;
  reject: (error: unknown) => void;
}

// One metadata-only batch in flight across all strip instances. Per-row Query
// keys share answers across sorting, filtering and remounts. A cancelled row
// leaves the waiting batch without issuing work for an invisible panel.
const waiting: Pending[] = [];
let running = false;
async function drain() {
  if (running) return;
  running = true;
  try {
    while (waiting.length > 0) {
      const batch = waiting.splice(0, 8).filter((entry) => {
        if (!entry.signal.aborted) return true;
        entry.resolve({ kind: "unknown" });
        return false;
      });
      if (batch.length === 0) continue;
      try {
        const answers = await getReadyStacks(batch.map((entry) => entry.pr));
        const byKey = new Map((answers ?? []).map((answer) => [prKey(answer), answer.stack]));
        for (const entry of batch) entry.resolve(byKey.get(prKey(entry.pr)) ?? { kind: "unknown" });
      } catch (error) {
        for (const entry of batch) entry.reject(error);
      }
    }
  } finally {
    running = false;
  }
}

function load(pr: PrIdentity, signal: AbortSignal): Promise<PrStack> {
  return new Promise((resolve, reject) => {
    waiting.push({ pr, signal, resolve, reject });
    queueMicrotask(() => { void drain(); });
  });
}

export function useReadyStacks(prs: PullRequest[]) {
  const rows = prs.filter((pr) => !pr.source || (pr.source.provider === "github" && pr.source.host === "github.com"));
  const queries = useQueries({ queries: rows.map((pr) => ({
    queryKey: ["ready-stack", prKey(pr), pr.head_oid, pr.base_ref],
    queryFn: ({ signal }: { signal: AbortSignal }) => load(prIdentity(pr), signal),
    staleTime: 60_000,
    gcTime: 5 * 60_000,
    retry: false,
    refetchOnWindowFocus: false,
  })) });
  const byKey = new Map(rows.map((pr, i) => [prKey(pr), queries[i].data]));
  return { of: (pr: PullRequest) => byKey.get(prKey(pr)) };
}
