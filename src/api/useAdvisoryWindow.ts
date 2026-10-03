import { useCallback, useEffect, useState, useSyncExternalStore } from "react";

import { useQueryClient, type QueryClient } from "@tanstack/react-query";

const clocks = new WeakMap<QueryClient, ReturnType<typeof clock>>();
function clock() {
  let tick = 0;
  let timer: ReturnType<typeof setInterval> | undefined;
  const listeners = new Set<() => void>();
  return {
    read: () => tick,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      timer ??= setInterval(() => { tick++; for (const notify of listeners) notify(); }, 30_000);
      return () => { listeners.delete(listener); if (!listeners.size) { clearInterval(timer); timer = undefined; } };
    },
  };
}
interface WindowState { canonical: string; tick: number; selected: string[]; served: Map<string, number> }
function select(canonical: string, tick: number, priority: ReadonlySet<string>, previous: Map<string, number>, demand: ReadonlySet<string>): WindowState {
  const identities: string[] = JSON.parse(canonical);
  const all = identities.filter(key => demand.has(key));
  const live = new Set(identities);
  const served = new Map([...previous].filter(([key]) => live.has(key)));
  // Visible/selected demand has six slots, with two oldest-first slots that
  // cannot be monopolized by an unreadable visible prefix.
  all.sort((a, b) => (served.get(a) ?? -1) - (served.get(b) ?? -1) || a.localeCompare(b));
  const preferred = all.filter(key => priority.has(key)).slice(0, 6);
  const fair = all.filter(key => !priority.has(key)).slice(0, 2);
  const lead = tick % 2 === 1 && fair.length ? [fair[0], ...preferred] : [...preferred.slice(0, 1), ...fair, ...preferred.slice(1)];
  const selected = [...new Set([...lead, ...all])].slice(0, 8);
  for (const key of selected) served.set(key, tick);
  return { canonical, tick, selected, served };
}
/** Removing observers cancels queued JS demand, not already dispatched native
 * HTTP. The native batch remains governed by its original deadline. */
export function useAdvisoryWindow(keys: string[], priority: ReadonlySet<string>, enabled: boolean, demand: ReadonlySet<string> = new Set(keys)) {
  const [visible, setVisible] = useState(() => document.visibilityState !== "hidden");
  const qc = useQueryClient();
  let shared = clocks.get(qc);
  if (!shared) { shared = clock(); clocks.set(qc, shared); }
  const subscribe = useCallback((listener: () => void) => enabled && visible ? shared.subscribe(listener) : () => {}, [shared, enabled, visible]);
  const tick = useSyncExternalStore(subscribe, shared.read);
  const canonical = JSON.stringify([...new Set(keys)].sort());
  const [window, setWindow] = useState(() => select(canonical, tick, priority, new Map(), demand));
  // Adjust during render, before child observers commit. Reorders keep the same
  // canonical set. Intersection changes affect the next finite window, so a
  // scroll cannot initiate an unbounded succession of new batches.
  if (window.tick !== tick) setWindow(select(canonical, tick, priority, window.served, demand));
  // A selected row changing identity gets a replacement, but unrelated set
  // churn must not spend another window. New offscreen rows wait for the tick.
  else if (window.canonical !== canonical) {
    const before: string[] = JSON.parse(window.canonical);
    const added = JSON.parse(canonical) as string[];
    const replacements = added.filter(key => !before.includes(key));
    const live = new Set(added);
    const selected = before.length === 0 ? select(canonical, tick, priority, window.served, demand).selected : window.selected.map(key => live.has(key) ? key : replacements.shift()).filter((key): key is string => !!key);
    setWindow({ ...window, canonical, selected });
  }
  useEffect(() => {
    const change = () => setVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", change);
    return () => document.removeEventListener("visibilitychange", change);
  }, []);
  return { selected: enabled && visible ? window.selected : [], tick, visible };
}
