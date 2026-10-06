import { useCallback, useEffect, useRef, useState } from "react";
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
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const [refresh, setRefresh] = useState(0);

  useEffect(() => {
    if (!enabled) return;
    let live = true;
    const epoch = ++generation.current;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function poll() {
      try {
        const next = await api.diskStatus();
        if (!live || generation.current !== epoch) return;
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
      if (!live || generation.current !== epoch) return;
      const newest = new Map<string, DiskObservation>();
      for (const observation of history) {
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
      generation.current++;
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
    status, settings, error, busy,
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
