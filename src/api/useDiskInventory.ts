import { useCallback, useEffect, useState } from "react";
import * as api from "./diskInventory";
import { IS_MOBILE_BUILD } from "../lib/target";
import type { DiskObservation, DiskSettings, DiskStatus } from "../types/disk";

const message = (error: unknown) => error instanceof Error ? error.message : String(error);
function retainMeasurement(next: DiskStatus, previous: DiskStatus | null): DiskStatus {
  return { ...next, observations: next.observations.length ? next.observations : previous?.observations ?? [] };
}

export function useDiskInventory(enabled: boolean) {
  const [status, setStatus] = useState<DiskStatus | null>(null);
  const [settings, setSettings] = useState<DiskSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Saved measurements that could not be read and were left out (#1963):
  // what remains is not the whole history, and must not read as if it were.
  const [historyUnreadable, setHistoryUnreadable] = useState(0);
  const [busy, setBusy] = useState(false);
  const [refresh, setRefresh] = useState(0);

  useEffect(() => {
    if (!enabled) return;
    // This run's answers count only while it is the current run. `live`
    // is enough on its own (#1965): the next run's cleanup-before-start
    // ordering means a newer run exists only once this one's cleanup has
    // set it false. A shared generation ref duplicated that check and
    // was read in the cleanup after it might have moved on.
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function poll() {
      try {
        const next = await api.diskStatus();
        if (!live) return;
        setStatus(previous => retainMeasurement(next, previous));
        if (next.running) timer = setTimeout(() => void poll(), 1000);
      } catch (error) {
        if (live) setError(message(error));
      }
    }
    void Promise.all([
      api.diskHistory(),
      IS_MOBILE_BUILD ? Promise.resolve(null) : api.diskSettings(),
    ]).then(([history, config]) => {
      if (!live) return;
      const newest = new Map<string, DiskObservation>();
      setHistoryUnreadable(history.unreadable);
      for (const observation of history.observations) {
        if (!newest.has(observation.volume.id)) newest.set(observation.volume.id, observation);
      }
      setStatus(previous => previous?.observations.length ? previous : {
        run_id: null, running: false, visited: 0, current_path: null,
        observations: [...newest.values()], error: null,
      });
      setSettings(config);
    }).catch(error => { if (live) setError(message(error)); });
    void poll();
    return () => {
      live = false;
      if (timer) clearTimeout(timer);
    };
  }, [enabled, refresh]);

  const perform = useCallback(async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try { await action(); }
    catch (error) { setError(message(error)); }
    finally { setBusy(false); }
  }, []);

  return {
    status, settings, error, busy, historyUnreadable,
    save: (next: DiskSettings) => perform(async () => {
      await api.saveDiskSettings(next);
      setSettings(next);
    }),
    start: (additional = false) => perform(async () => {
      if (additional && settings) {
        const next = { ...settings, additional_locations: true };
        await api.saveDiskSettings(next);
        setSettings(next);
      }
      const next = await api.startDiskScan();
      setStatus(previous => retainMeasurement(next, previous));
      setRefresh(n => n + 1);
    }),
    cancel: () => perform(async () => {
      if (status?.run_id) await api.cancelDiskScan(status.run_id);
    }),
  };
}
