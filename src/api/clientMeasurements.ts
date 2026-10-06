import { useCallback, useContext, useSyncExternalStore } from "react";
import { QueryClientContext, type QueryClient } from "@tanstack/react-query";
import { IS_MOBILE_BUILD } from "../lib/target";
import type { ClientMeasurement } from "../types/measurement";
import { recordClientMeasurements } from "./tauri";
import { PHONE_MEASUREMENT_PREFS, recordPhoneMeasurements } from "./phoneMeasurements";
export function measurementsEnabled(qc:QueryClient|undefined) {
  return (IS_MOBILE_BUILD ? qc?.getQueryData<{enabled:boolean}>(PHONE_MEASUREMENT_PREFS)?.enabled : qc?.getQueryData<{diagnostic_logging:boolean}>(["ui-prefs"])?.diagnostic_logging) === true;
}
export function publishClientMeasurements(batch:ClientMeasurement[]) {
  // Host references never become phone-local handles. Count observations survive unlinked.
  return IS_MOBILE_BUILD ? recordPhoneMeasurements(batch.map(value=>{
    switch(value.kind){
      case "mounted_review":return {...value,receipt:undefined,scope:undefined};
      case "stats_view":return {...value,scope:undefined};
      case "transcript_view":return {...value,operation:undefined};
    }
  })) : recordClientMeasurements(batch);
}
export function useMeasurementsEnabled() {
  const qc=useContext(QueryClientContext);
  const subscribe=useCallback((notify:()=>void)=>qc?.getQueryCache().subscribe(notify)??(()=>{}),[qc]);
  return useSyncExternalStore(subscribe,()=>measurementsEnabled(qc),()=>false);
}
