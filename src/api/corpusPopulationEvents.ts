import { focusManager, type Query, type QueryClient } from "@tanstack/react-query";
import { listen, type UnlistenFn } from "./transport";
import { safeUnlisten } from "./unlisten";

const clients = new WeakMap<QueryClient, { users: number; dispose: () => void }>();
const previouslyObserved = new WeakSet<QueryClient>();
const summaries = new Set(["claude-usage-profile", "claude-index-coverage", "claude-transcript-search", "claude-session-events", "claude-event-profile"]);

/** One listener per cache. A receipt during a read requires a read AFTER it settles:
 * cancelling/restarting on every receipt can starve a slow read indefinitely. */
export function observeCorpusPopulation(client: QueryClient): () => void {
  let entry = clients.get(client);
  if (!entry) {
    let active = true;
    let scheduled = false;
    let unlisten: UnlistenFn | undefined;
    const dirty = new Set<Query>();
    const waiting = new Set<Query>();
    const schedule = () => {
      if (scheduled || !active) return;
      scheduled = true;
      queueMicrotask(() => {
        scheduled = false;
        if (!active) return;
        for (const query of dirty) {
          if (query.state.fetchStatus !== "idle") continue;
          dirty.delete(query);
          waiting.delete(query);
          // Settlement clears invalidation, so invalidate again before the follow-up.
          query.invalidate();
          if (focusManager.isFocused() && query.isActive()) {
            void client.refetchQueries({ queryKey: query.queryKey, exact: true, type: "active" }, { cancelRefetch: false });
          }
        }
      });
    };
    const invalidate = () => {
      if (!active) return;
      for (const query of client.getQueryCache().getAll()) {
        if (!summaries.has(String(query.queryKey[0]))) continue;
        dirty.add(query);
        if (query.state.fetchStatus !== "idle") waiting.add(query);
        query.invalidate();
      }
      schedule();
    };
    const unsubscribe = client.getQueryCache().subscribe(event => {
      if (event.type === "removed") { dirty.delete(event.query); waiting.delete(event.query); }
      else if (dirty.has(event.query)) {
        // A focus/mount/manual fetch started after invalidation already consumes it.
        if (event.type === "updated" && event.action.type === "fetch" && !waiting.has(event.query)) dirty.delete(event.query);
        if (event.query.state.fetchStatus === "idle") { waiting.delete(event.query); schedule(); }
      }
    });
    entry = { users: 0, dispose: () => { active = false; dirty.clear(); waiting.clear(); unsubscribe(); safeUnlisten(unlisten); } };
    clients.set(client, entry);
    void listen("claude-population-updated", invalidate).then((stop) => {
      if (active) unlisten = stop; else safeUnlisten(stop);
    }, () => {});
    // Null receipts cannot be replayed. A detached cache may have missed commits,
    // including ones hidden by an old fetch settling after its last observer left.
    if (previouslyObserved.has(client)) invalidate();
    previouslyObserved.add(client);
  }
  entry.users++;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    if (--entry.users === 0) { entry.dispose(); clients.delete(client); }
  };
}
