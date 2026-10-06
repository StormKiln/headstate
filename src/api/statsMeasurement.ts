import { captureReference } from "./measurementCapture";
import { useCallback, useEffect, useRef, useSyncExternalStore } from "react";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { measurementsEnabled, publishClientMeasurements } from "./clientMeasurements";

import type { StatsBoard } from "../types/pr";
import type { ClientMeasurement, MeasurementOutcome } from "../types/measurement";
type Observation = Extract<ClientMeasurement, {kind:"stats_view"}>;
function enabled(qc: QueryClient) {
  return measurementsEnabled(qc);
}
function value(qc: QueryClient, observation: "readback"|"mounted", outcome:MeasurementOutcome, board?:StatsBoard, elapsed?:number):Observation {
  return {kind:"stats_view",observation,outcome,scope:captureReference(qc, board?.measurementScope),
    rows:outcome === "rejected" ? undefined : board?.rows.length, elapsed_ms:elapsed===undefined?undefined:Math.max(0,Math.round(elapsed))};
}
export function observeStats(qc:QueryClient, observation:"readback"|"mounted",outcome:MeasurementOutcome,board?:StatsBoard,elapsed?:number) {
  if(enabled(qc))void publishClientMeasurements([value(qc,observation,outcome,board,elapsed)]).catch(()=>{});
}
/** Subscribe only to preferences already owned by the app; no preference query. */
export function useStatsMeasurement(board:StatsBoard|undefined,outcome:MeasurementOutcome) {
  const qc=useQueryClient();
  const subscribe=useCallback((notify:()=>void)=>qc.getQueryCache().subscribe(notify),[qc]);
  const context=useSyncExternalStore(subscribe,()=>JSON.stringify([enabled(qc), qc.getQueryData(["measurement-capture"])]));
  const on=JSON.parse(context)[0] as boolean;
  const last=useRef<string|undefined>(undefined);
  const encoded=on&&board?JSON.stringify(value(qc,"mounted",outcome,board)):undefined;
  useEffect(()=>{
    if(!encoded){last.current=undefined;return;}
    if(last.current===encoded)return;last.current=encoded;
    void publishClientMeasurements([JSON.parse(encoded) as Observation]).catch(()=>{});
  },[encoded]);
}
