import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { listen } from "./transport";
import { safeUnlisten } from "./unlisten";
import type { PrIdentity } from "../types/identity";
import { prKey } from "../lib/prIdentity";

/** Desktop writes invalidate affected reads on every connected webview. */
export function useGitLabInvalidation() {
  const client = useQueryClient();
  useEffect(() => {
    let active = true;
    let stop: (() => void) | undefined;
    void listen<PrIdentity>("gitlab-data-changed", ({ payload }) => {
      if (!active || payload.source?.provider !== "gitlab") return;
      for (const queryKey of [["gitlab-detail", prKey(payload)], ["gitlab-actions", prKey(payload)], ["stats", "gitlab", payload.source.host]]) {
        const filters = { queryKey, predicate: (query: { queryKey: readonly unknown[] }) => query.queryKey[0] !== "stats" || query.queryKey[3] !== "tree" };
        void client.cancelQueries(filters).then(() => client.invalidateQueries(filters));
      }
    }).then(unlisten => { if (active) stop = unlisten; else safeUnlisten(unlisten); }, () => {});
    return () => { active = false; if (stop) safeUnlisten(stop); };
  }, [client]);
}
