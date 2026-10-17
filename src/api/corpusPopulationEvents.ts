import { focusManager, type QueryClient } from "@tanstack/react-query";
import { listen, type UnlistenFn } from "./transport";
import { safeUnlisten } from "./unlisten";

const clients = new WeakMap<QueryClient, { users: number; dispose: () => void }>();

/** One receipt listener per query cache, even with multiple visible consumers. */
export function observeCorpusPopulation(client: QueryClient): () => void {
  let entry = clients.get(client);
  if (!entry) {
    let active = true;
    let unlisten: UnlistenFn | undefined;
    entry = { users: 0, dispose: () => { active = false; safeUnlisten(unlisten); } };
    clients.set(client, entry);
    void listen("claude-population-updated", () => {
      if (!active) return;
      for (const key of ["claude-usage-profile", "claude-index-coverage"]) {
        void client.invalidateQueries({ queryKey: [key], refetchType: focusManager.isFocused() ? "active" : "none" });
      }
    }).then((stop) => { if (active) unlisten = stop; else safeUnlisten(stop); }, () => {});
  }
  entry.users++;
  return () => {
    if (--entry.users === 0) { entry.dispose(); clients.delete(client); }
  };
}
