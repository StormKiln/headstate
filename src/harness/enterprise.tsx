import { Profiler, useLayoutEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { usePullRequests,useReviewing } from '../api/hooks';
import { useReadyPushers } from '../api/useReadyPushers';
import { useReadyStacks } from '../api/useReadyStacks';
import { prKey } from '../lib/prIdentity';
import { useSourceRefresh, refreshWithState } from '../api/sourceRefreshHooks';
import { PrList } from '../components/PrList';
import { ReadyStrip } from '../components/ReadyStrip';
import { PrDetailView } from '../components/PrDetailView';
import { StatsPage } from '../components/StatsPage';
import { useFilters } from '../store/filters';
import type { PullRequest } from '../types/pr';
import { telemetry, boundedPush, measurement } from './enterpriseTransport';
import '../index.css';
const commits:{phase:string;actual:number;base:number;started:number;at:number}[]=[];
const longTasks:{at:number;duration:number}[]|null=PerformanceObserver.supportedEntryTypes.includes('longtask')?[]:null;
if(longTasks)new PerformanceObserver(list=>{for(const e of list.getEntries())boundedPush(longTasks,{at:e.startTime,duration:e.duration});}).observe({type:'longtask',buffered:true});
let renderedInventory:{authored:string[];reviewing:string[]}={authored:[],reviewing:[]};
const advisoryPublications:{at:number;wallTime:number;kind:string;identity:unknown;number:number;owner:unknown;generation:unknown;freshPusher:boolean;freshRules:boolean;freshStack:boolean}[]=[];
const probe={advisoryPublications,inventory:()=>renderedInventory,querySummary:()=>client.getQueryCache().getAll().map(query=>{
 const value=query.state.data as Record<string,unknown>|undefined;
 const schedule=query.meta?.advisorySchedule as {claims?:Map<string,boolean>;lastAdmittedAt?:number;hasContinuation?:boolean;resumeBoostSpent?:boolean}|undefined;
 const freshness=(field:string,usable:boolean)=>{const evidence=value?.[field] as {expiresAt:number;observedAt:number}|undefined;return evidence?{fresh:usable&&evidence.expiresAt>performance.now(),expiresAt:evidence.expiresAt,observedAt:evidence.observedAt}:null;};
 return {kind:query.queryKey[0],syntheticNumber:['ready-pushers','ready-stack'].includes(String(query.queryKey[0]))?JSON.parse(JSON.parse(String(query.queryKey[3]))[0])[3]:undefined,status:query.state.status,fetchStatus:query.state.fetchStatus,observers:query.getObserversCount(),schedule:schedule?{lastAdmittedAt:schedule.lastAdmittedAt,hasContinuation:schedule.hasContinuation,resumeBoostSpent:schedule.resumeBoostSpent,preferred:[...(schedule.claims?.values()??[])].some(Boolean)}:undefined,
  evidence:{pusher:freshness('pusher',(value?.row as {last_pusher?:{state:string}}|undefined)?.last_pusher?.state==='known'),rules:freshness('rules',(value?.row as {rules?:{state:string}}|undefined)?.rules?.state==='read'),stack:freshness('lastKnown',(value?.stack as {kind:string}|undefined)?.kind==='none')},
  measuredPusher:!!value?.pusher,measuredRules:!!value?.rules,measuredStack:!!value?.lastKnown,
  ...(query.queryKey[0]==='stats-board'?{owner:value?.owner,viewer:value?.viewer,scopeKey:value?.scopeKey,days:query.queryKey[3],daysCovered:value?.daysCovered,daysTotal:value?.daysTotal,total:value?.total,retrieved:value?.retrieved,complete:value?.complete,accumulated:value?.accumulated,rows:value?.rows,repoCounts:value?.repoCounts,window:value?.window,stream:value?.stream,backfill:value?.backfill}:{}),
 };
}),measurement,commits,longTasks,telemetry,started:performance.now(),firstUsefulQueue:null as number|null,fullQueue:null as number|null,refreshQueues:()=>Promise.allSettled(['authored','reviewing'].map(list=>refreshWithState(client,list as 'authored'|'reviewing'))),refreshDetail:()=>client.invalidateQueries({queryKey:["pr-detail"]})};
declare global {interface Window {__enterprise:typeof probe}}
window.__enterprise=probe;
const client=new QueryClient({defaultOptions:{queries:{retry:false,refetchOnWindowFocus:false}}});
// Read-only post-publication witnesses. Presence of retained last-known data is
// not evidence that this generation ever published usable unexpired authority.
client.getQueryCache().subscribe(event=>{
 if(event.type!=='updated'||event.action.type!=='success')return;
 const query=event.query,kind=String(query.queryKey[0]);
 if(!['ready-pushers','ready-stack'].includes(kind))return;
 const value=query.state.data as {row?:{last_pusher:{state:string};rules:{state:string}};stack?:{kind:string};pusher?:{expiresAt:number};rules?:{expiresAt:number};lastKnown?:{expiresAt:number}}|undefined;
 const at=performance.now(),identity=JSON.parse(String(query.queryKey[3]));
 boundedPush(advisoryPublications,{at,wallTime:Date.now(),kind,identity,number:JSON.parse(identity[0])[3],owner:query.queryKey[1],generation:query.queryKey[2],
  freshPusher:value?.row?.last_pusher.state==='known'&&(value.pusher?.expiresAt??0)>at,
  freshRules:value?.row?.rules.state==='read'&&(value.rules?.expiresAt??0)>at,
  freshStack:value?.stack?.kind==='none'&&(value.lastKnown?.expiresAt??0)>at});
});
useFilters.getState().setStatsScope('org','synthetic-lab',undefined);
export function Workload(){
 const authored=usePullRequests();const reviewing=useReviewing();const source=useSourceRefresh('reviewing');
 useLayoutEffect(()=>{
  // Observe exactly what the production hooks render, including retained startup data.
  renderedInventory={authored:(authored.data??[]).map(row=>`${row.repo}/${row.number}`).sort(),reviewing:(reviewing.data??[]).map(row=>`${row.repo}/${row.number}`).sort()};
  if(reviewing.data?.some(row=>row.number>=51&&row.repo.startsWith('synthetic-lab/'))&&probe.firstUsefulQueue===null)probe.firstUsefulQueue=performance.now()-probe.started;
  if(authored.data?.length===50&&reviewing.data?.length===150&&probe.fullQueue===null)probe.fullQueue=performance.now()-probe.started;
 },[authored.data,reviewing.data]);
 const completionProbe=new URL(location.href).searchParams.get('scenario')==='ready-completion';
 const first=new URL(location.href).searchParams.get('role')==='paired'?76:51;
 const readyRows=completionProbe?(reviewing.data??[]).filter(row=>row.number>=first&&row.number<first+16):(reviewing.data??[]);
 const [selected,select]=useState<PullRequest|null>(null);const [view,setView]=useState(completionProbe?'idle':'reviewing');
 return <main className="min-h-screen bg-[#0d1117] p-5 text-[#c9d1d9]">
  <nav className="mb-4 flex gap-4"><button onClick={()=>setView('reviewing')}>To Review</button><button onClick={()=>setView('authored')}>My PRs</button><button onClick={()=>setView('stats')}>Statistics</button></nav>
  <output data-testid="counts">Authored {authored.data?.length??'unknown'} / Reviewing {reviewing.data?.length??'unknown'}</output>
  {view==='idle'?null:view==='stats'?<StatsPage/>:selected?<PrDetailView repo={selected.repo} number={selected.number} onBack={()=>select(null)} localTools={false}/>:view==='reviewing'?<ReadyStrip prs={readyRows} onOpen={select} localTools={false} availability={{status:reviewing.data===undefined?(reviewing.isError?"failed":"pending"):"available",coverage:source.coverage??null}}/>:<PrList prs={authored.data??[]} onOpen={select}/>}
 </main>;
}
function ReadyProgressRows({rows}:{rows:PullRequest[]}) {
 const priority=new Set(rows.slice(0,6).map(prKey));
 const pushers=useReadyPushers(rows,priority);
 const stacks=useReadyStacks(rows,priority);
 return <section aria-label="Mounted Ready hook results">{rows.map(row=><div key={prKey(row)} data-progress-number={row.number} data-progress-visible={priority.has(prKey(row))}>
  Synthetic review {row.number}: {JSON.stringify({pusher:pushers.of(row),pusherDisplay:pushers.displayOf(row),stack:stacks.of(row),stackDisplay:stacks.displayOf(row)})}
 </div>)}</section>;
}
function ReadyProgress() {
 const reviewing=useReviewing();
 const start=new URL(location.href).searchParams.get("role")==="paired"?76:51;
 const rows=(reviewing.data??[]).filter(row=>row.number>=start&&row.number<start+16).sort((a,b)=>a.number-b.number);
 const [mounted,mount]=useState(false);
 return <main><output data-testid="ready-progress-loaded">{rows.length}</output><button disabled={rows.length!==16} onClick={()=>mount(!mounted)}>{mounted?"Unmount Ready hooks":"Mount Ready hooks"}</button>{mounted&&<ReadyProgressRows rows={rows}/>}</main>;
}
createRoot(document.getElementById('root')!).render(<QueryClientProvider client={client}><Profiler id="root" onRender={(_id,phase,actual,base,started,at)=>boundedPush(commits,{phase,actual,base,started,at})}>{new URL(location.href).searchParams.get('scenario')==='ready-progress'?<ReadyProgress/>:<Workload/>}</Profiler></QueryClientProvider>);
