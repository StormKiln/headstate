import { useCallback, useEffect, useRef, useSyncExternalStore } from "react";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { IS_MOBILE_BUILD } from "../lib/target";
import { recordClientMeasurements } from "./tauri";
import type { StatsBoard } from "../types/pr";
import type { ClientMeasurement, MeasurementOutcome } from "../types/measurement";
type Observation = Extract<ClientMeasurement, {kind:"stats_view"}>;
function enabled(qc: QueryClient) {
  return !IS_MOBILE_BUILD && qc.getQueryData<{diagnostic_logging?:boolean}>(["ui-prefs"])?.diagnostic_logging === true;
}
function value(observation: "readback"|"mounted", outcome:MeasurementOutcome, board?:StatsBoard, elapsed?:number):Observation {
  return {kind:"stats_view",observation,outcome,scope:board?.measurementScope,
    rows:outcome === "rejected" ? undefined : board?.rows.length, elapsed_ms:elapsed===undefined?undefined:Math.max(0,Math.round(elapsed))};
}
export function observeStats(qc:QueryClient, observation:"readback"|"mounted",outcome:MeasurementOutcome,board?:StatsBoard,elapsed?:number) {
  if(enabled(qc))void recordClientMeasurements([value(observation,outcome,board,elapsed)]).catch(()=>{});
}
/** Subscribe only to preferences already owned by the app; no preference query. */
export function useStatsMeasurement(board:StatsBoard|undefined,outcome:MeasurementOutcome) {
  const qc=useQueryClient();
  const subscribe=useCallback((notify:()=>void)=>qc.getQueryCache().subscribe(notify),[qc]);
  const on=useSyncExternalStore(subscribe,()=>enabled(qc));
  const last=useRef<string|undefined>(undefined);
  const encoded=on&&board?JSON.stringify(value("mounted",outcome,board)):undefined;
  useEffect(()=>{
    if(!encoded){last.current=undefined;return;}
    if(last.current===encoded)return;last.current=encoded;
    void recordClientMeasurements([JSON.parse(encoded) as Observation]).catch(()=>{});
  },[encoded]);
}
