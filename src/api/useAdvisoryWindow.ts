import { useEffect, useState } from "react";

interface WindowState { canonical: string; tick: number; selected: string[]; served: Map<string, number> }
function select(canonical: string, tick: number, priority: ReadonlySet<string>, previous: Map<string, number>): WindowState {
  const all: string[] = JSON.parse(canonical);
  const live = new Set(all);
  const served = new Map([...previous].filter(([key]) => live.has(key)));
  // Oldest demand first; viewport priority breaks ties. A failed visible prefix
  // cannot starve later rows. Row demands are not HTTP attempts (native owns it).
  all.sort((a, b) => (served.get(a) ?? -1) - (served.get(b) ?? -1)
    || Number(priority.has(b)) - Number(priority.has(a)) || a.localeCompare(b));
  const selected = all.slice(0, 8);
  for (const key of selected) served.set(key, tick);
  return { canonical, tick, selected, served };
}
/** Removing observers cancels queued JS demand, not already dispatched native
 * HTTP. The native batch remains governed by its original deadline. */
export function useAdvisoryWindow(keys: string[], priority: ReadonlySet<string>, enabled: boolean) {
  const [visible, setVisible] = useState(() => document.visibilityState !== "hidden");
  const [tick, setTick] = useState(0);
  const canonical = JSON.stringify([...new Set(keys)].sort());
  const [window, setWindow] = useState(() => select(canonical, tick, priority, new Map()));
  // Adjust during render, before child observers commit. Reorders keep the same
  // canonical set. Intersection changes affect the next finite window, so a
  // scroll cannot initiate an unbounded succession of new batches.
  if (window.canonical !== canonical || window.tick !== tick) setWindow(select(canonical, tick, priority, window.served));
  useEffect(() => {
    const change = () => setVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", change);
    return () => document.removeEventListener("visibilitychange", change);
  }, []);
  useEffect(() => {
    if (!enabled || !visible) return;
    const timer = setInterval(() => setTick(value => value + 1), 30_000);
    return () => clearInterval(timer);
  }, [enabled, visible]);
  return { selected: enabled && visible ? window.selected : [], tick, visible };
}
