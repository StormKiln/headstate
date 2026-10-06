import { useEffect } from "react";
import type { QueryClient } from "@tanstack/react-query";
import { measurementsEnabled, publishClientMeasurements, useMeasurementsEnabled } from "./clientMeasurements";
import type { ClientMeasurement } from "../types/measurement";
export type TranscriptObservation = Omit<Extract<ClientMeasurement,{kind:"transcript_view"}>,"kind"|"operation"|"capability">;
export function observeTranscript(qc:QueryClient|undefined, observation:TranscriptObservation) {
  if(measurementsEnabled(qc))void publishClientMeasurements([{kind:"transcript_view",...observation,capability:"measured"}]).catch(()=>{});
}
/** Two browser animation callbacks after the accepted window commit, never compositor paint. */
export function useTranscriptWindowMeasurement(rows:number,resident:number,revision:unknown) {
  const enabled=useMeasurementsEnabled();
  useEffect(()=>{
    if(!enabled)return;
    const base={kind:"transcript_view" as const,rows,resident_rows:resident,capability:"measured" as const};
    if(document.visibilityState==="hidden") {void publishClientMeasurements([{...base,phase:"hidden"}]).catch(()=>{});return;}
    void publishClientMeasurements([{...base,phase:"render"}]).catch(()=>{});
    if(typeof requestAnimationFrame!=="function")return;
    const start=performance.now();let second:number|undefined;
    let canceled=false;
    const first=requestAnimationFrame(()=>{
      if(canceled)return;
      second=requestAnimationFrame(()=>{
        if(!canceled&&document.visibilityState!=="hidden")void publishClientMeasurements([{...base,phase:"raf_proxy",elapsed_ms:Math.max(0,Math.round(performance.now()-start))}]).catch(()=>{});
      });
    });
    const cancel=()=>{canceled=true;cancelAnimationFrame(first);if(second!==undefined)cancelAnimationFrame(second);};
    // Visibility can change without changing the mounted rows or their identity.
    // Censor that interval permanently; only a new visible commit starts a sample.
    const visibility=()=>{if(document.visibilityState==="hidden")cancel();};
    document.addEventListener("visibilitychange",visibility);
    return()=>{cancel();document.removeEventListener("visibilitychange",visibility);};
  },[enabled,rows,resident,revision]);
}
