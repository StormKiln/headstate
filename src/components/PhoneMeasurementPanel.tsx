import { useEffect, useState } from "react";
import { phoneMeasurementStatus, sharePhoneMeasurements, usePhoneMeasurementPrefs } from "../api/phoneMeasurements";
import type { MeasurementStatus } from "../types/measurement";
export function PhoneMeasurementPanel() {
  const prefs=usePhoneMeasurementPrefs();
  const [status,setStatus]=useState<MeasurementStatus>();
  const [error,setError]=useState<string>();
  const [result,setResult]=useState<string>();
  const [busy,setBusy]=useState(false);
  const [attempt,setAttempt]=useState(0);
  useEffect(()=>{let live=true;void phoneMeasurementStatus().then(value=>{if(live){setStatus(value);setError(undefined);}},()=>{if(live)setError("Phone measurements are unavailable. Update this phone app and try again.");});return()=>{live=false;};},[prefs.data?.enabled,attempt]);
  const toggle=async(enabled:boolean)=>{setBusy(true);setError(undefined);try{await prefs.set({enabled});setAttempt(n=>n+1);}catch{setError("Could not save phone measurement preferences. Try again.");}finally{setBusy(false);}};
  const share=async()=>{setBusy(true);setError(undefined);setResult(undefined);try{const value=await sharePhoneMeasurements();setResult(value.outcome==="cancelled"?"Sharing canceled.":`${value.outcome==="shared"?"Shared":"Opened sharing for"} ${value.report.records} recent measurement records.${value.report.incomplete?" Coverage is incomplete; earlier records or other gaps are identified in the report.":""}`);setAttempt(n=>n+1);}catch{setError("Could not share phone measurements. Finish any open share sheet, then try again.");}finally{setBusy(false);}};
  return <section aria-label="This phone measurements" className="space-y-2 p-3 text-xs">
    <h3 className="font-medium">This phone</h3>
    <p>Record anonymous counts and timings on this phone. Transcript text and repository names are excluded. This is separate from the paired computer's diagnostic log.</p>
    {prefs.isError?<p role="alert">Phone measurement preferences are unavailable. Update this phone app and try again.</p>:<label className="flex items-center gap-2"><input type="checkbox" checked={prefs.data?.enabled??false} disabled={busy||!prefs.data} onChange={e=>void toggle(e.target.checked)}/>Record phone measurements</label>}
    {status?<><p>Capture {status.enabled?"on":"off"}. {status.durableRecords} retained records.</p><p>{status.oldestWallMs!==null&&status.newestWallMs!==null?`Retained interval: ${new Date(status.oldestWallMs).toLocaleString()} – ${new Date(status.newestWallMs).toLocaleString()}`:"No durable measurement interval yet."}</p>{status.writerState==="unavailable"?<p role="alert">Phone measurement storage is unavailable. Restart the app and try again.</p>:null}{status.incomplete?<p>Coverage has gaps: {status.dropped} dropped, {status.invalid} invalid, {status.rotatedOut} rotated out. Other gaps are included in the report.</p>:null}</>:null}
    <p>Sharing includes recent whole records up to 8 MiB. Older omitted records are identified. Nothing is sent until you choose a share destination.</p>
    <button type="button" disabled={busy||!status||status.writerState==="unavailable"} onClick={()=>void share()}>{busy?"Preparing phone measurements…":"Share recent measurement report…"}</button>
    {error?<p role="alert">{error}</p>:null}{result?<p role="status">{result}</p>:null}
    {error||prefs.isError?<button type="button" onClick={()=>{void prefs.refetch();setAttempt(n=>n+1);}}>Retry phone measurement status</button>:null}
  </section>;
}
