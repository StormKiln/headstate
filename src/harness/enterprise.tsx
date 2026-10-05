import { Profiler, useLayoutEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { usePullRequests,useReviewing } from '../api/hooks';
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
const probe={inventory:()=>renderedInventory,querySummary:()=>client.getQueryCache().getAll().map(query=>{
 const value=query.state.data as Record<string,unknown>|undefined;
 return {kind:query.queryKey[0],syntheticNumber:['ready-pushers','ready-stack'].includes(String(query.queryKey[0]))?JSON.parse(JSON.parse(String(query.queryKey[3]))[0])[3]:undefined,status:query.state.status,fetchStatus:query.state.fetchStatus,
  measuredPusher:!!value?.pusher,measuredRules:!!value?.rules,measuredStack:!!value?.lastKnown,
  ...(query.queryKey[0]==='stats-board'?{days:query.queryKey[3],total:value?.total,retrieved:value?.retrieved,complete:value?.complete,accumulated:value?.accumulated,backfill:value?.backfill}:{}),
 };
}),measurement,commits,longTasks,telemetry,started:performance.now(),firstUsefulQueue:null as number|null,fullQueue:null as number|null,refreshQueues:()=>Promise.allSettled(['authored','reviewing'].map(list=>refreshWithState(client,list as 'authored'|'reviewing'))),refreshDetail:()=>client.invalidateQueries({queryKey:["pr-detail"]})};
declare global {interface Window {__enterprise:typeof probe}}
window.__enterprise=probe;
const client=new QueryClient({defaultOptions:{queries:{retry:false,refetchOnWindowFocus:false}}});
useFilters.getState().setStatsScope('org','synthetic-lab',undefined);
export function Workload(){
 const authored=usePullRequests();const reviewing=useReviewing();const source=useSourceRefresh('reviewing');
 useLayoutEffect(()=>{
  // Observe exactly what the production hooks render, including retained startup data.
  renderedInventory={authored:(authored.data??[]).map(row=>`${row.repo}/${row.number}`).sort(),reviewing:(reviewing.data??[]).map(row=>`${row.repo}/${row.number}`).sort()};
  if(reviewing.data?.some(row=>row.number>=51&&row.repo.startsWith('synthetic-lab/'))&&probe.firstUsefulQueue===null)probe.firstUsefulQueue=performance.now()-probe.started;
  if(authored.data?.length===50&&reviewing.data?.length===150&&probe.fullQueue===null)probe.fullQueue=performance.now()-probe.started;
 },[authored.data,reviewing.data]);
 const [selected,select]=useState<PullRequest|null>(null);const [view,setView]=useState('reviewing');
 return <main className="min-h-screen bg-[#0d1117] p-5 text-[#c9d1d9]">
  <nav className="mb-4 flex gap-4"><button onClick={()=>setView('reviewing')}>To Review</button><button onClick={()=>setView('authored')}>My PRs</button><button onClick={()=>setView('stats')}>Statistics</button></nav>
  <output data-testid="counts">Authored {authored.data?.length??'unknown'} / Reviewing {reviewing.data?.length??'unknown'}</output>
  {view==='stats'?<StatsPage/>:selected?<PrDetailView repo={selected.repo} number={selected.number} onBack={()=>select(null)} localTools={false}/>:view==='reviewing'?<ReadyStrip prs={reviewing.data??[]} onOpen={select} localTools={false} availability={{status:reviewing.data===undefined?(reviewing.isError?"failed":"pending"):"available",coverage:source.coverage??null}}/>:<PrList prs={authored.data??[]} onOpen={select}/>}
 </main>;
}
createRoot(document.getElementById('root')!).render(<QueryClientProvider client={client}><Profiler id="root" onRender={(_id,phase,actual,base,started,at)=>boundedPush(commits,{phase,actual,base,started,at})}><Workload/></Profiler></QueryClientProvider>);
