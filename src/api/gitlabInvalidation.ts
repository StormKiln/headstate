import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { listen } from "./transport";
import { safeUnlisten } from "./unlisten";
import type { PrIdentity } from "../types/identity";
import { revalidateGitLabQueries } from "./gitlabReceiptRevalidation";
import { useGitLabViewer } from "./authAvailability";
import { prKey } from "../lib/prIdentity";

/** Desktop writes invalidate affected reads on every connected webview. */
export function useGitLabInvalidation() {
  const client = useQueryClient();
  const viewer = useGitLabViewer();
  useEffect(() => {
    let active = true;
    let stop: (() => void) | undefined;
    void listen<PrIdentity>("gitlab-data-changed", ({ payload }) => {
      if (!active || payload.source?.provider !== "gitlab") return;
      for (const queryKey of [["gitlab-detail", prKey(payload)], ["gitlab-actions", prKey(payload)]]) {
        revalidateGitLabQueries(client, client.getQueryCache().findAll({ queryKey, predicate: query => viewer === undefined || query.queryKey[2] === viewer }));
      }
    }).then(unlisten => { if (active) stop = unlisten; else safeUnlisten(unlisten); }, () => {});
    return () => { active = false; if (stop) safeUnlisten(stop); };
  }, [client, viewer]);
}
