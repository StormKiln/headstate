import { useEffect } from "react";
import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import { statsDemand, type StatsDemandRequest } from "./tauri";
import { useAdvisorySession } from "./advisoryEvidence";

type Acquire = Extract<StatsDemandRequest, {op:"acquire"}>;
interface Observer { refs:number; stop:()=>void }
const observers = new WeakMap<QueryClient,Map<string,Observer>>();

function observe(request:Acquire):()=>void {
  let stopped=false, epoch=0, sequence=0, handle:string|undefined;
  let timer:ReturnType<typeof setTimeout>|undefined;
  let permanent=false;
  function release(value:string) { void statsDemand({op:"release",handle:value,sequence:++sequence}).catch(()=>{}); }
  function pause() {
    epoch++; clearTimeout(timer);timer=undefined;
    if(handle)release(handle);handle=undefined;
  }
  async function tick() {
    if(stopped || document.visibilityState==="hidden" || permanent)return;
    const generation=epoch;
    try {
      const receipt=await statsDemand(handle ? {op:"renew",handle,sequence:++sequence}:request);
      if(stopped || generation!==epoch) {if(receipt?.handle)release(receipt.handle);return;}
      if(!receipt?.handle || !receipt.owner || !Number.isSafeInteger(receipt.owner.generation) || receipt.owner.generation<=0)throw new Error("missing stats demand receipt");
      handle=receipt.handle;
    } catch(error) {
      if(stopped || generation!==epoch)return;
      handle=undefined;
      permanent=/not a Headstate command|unknown command|command.*not found/i.test(String(error));
    }
    if(!stopped && generation===epoch && !permanent)timer=setTimeout(()=>void tick(),60_000);
  }
  function visibility() {pause();if(document.visibilityState!=="hidden")void tick();}
  document.addEventListener("visibilitychange",visibility);
  void tick();
  return ()=>{stopped=true;pause();document.removeEventListener("visibilitychange",visibility);};
}

/** One local lease per equal visible QueryClient observation, independent of
 * board freshness/provider reload. Native monotonic expiry owns correctness. */
export function useStatsDemand(scopeKind:string|undefined,scopeValue:string|undefined,measure:"merged"|"opened",days:number,enabled:boolean) {
  const client=useQueryClient();
  const {generation}=useAdvisorySession(client);
  useEffect(()=>{
    if(!enabled || !scopeKind || measure!=="merged")return;
    let registry=observers.get(client);if(!registry){registry=new Map();observers.set(client,registry);}
    const key=JSON.stringify([generation,scopeKind,scopeValue,measure,days]);
    let observer=registry.get(key);
    if(!observer){observer={refs:0,stop:observe({op:"acquire",scopeKind,scopeValue,measure,days})};registry.set(key,observer);}
    observer.refs++;
    return ()=>{if(--observer.refs===0){observer.stop();registry.delete(key);}};
  },[client,generation,scopeKind,scopeValue,measure,days,enabled]);
}
