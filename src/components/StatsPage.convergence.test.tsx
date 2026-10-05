import { StrictMode } from "react";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
const seam = vi.hoisted(() => ({ call: vi.fn(), callbacks: new Map<string, Set<(event: {payload: unknown}) => void>>() }));
vi.mock("../api/transport", async original => ({ ...await original<Record<string, unknown>>(), call: seam.call,
  listen: vi.fn(async (name: string, callback: (event: {payload: unknown}) => void) => {
    const callbacks = seam.callbacks.get(name) ?? new Set(); seam.callbacks.set(name, callbacks); callbacks.add(callback);
    return () => { callbacks.delete(callback); };
  }),
}));
vi.mock("../store/filters", () => ({ useActiveFilters: () => ({statsScopeKind:"org",statsScopeValue:"synthetic-lab"}), useFilters: () => ({setFilter:vi.fn(),setPanel:vi.fn()}) }));
import { StatsPage } from "./StatsPage";
const owner={viewer:"synthetic-viewer",generation:1};
const scopeKey="merged|*|org:synthetic-lab";
const window={from:"2026-09-05",to:"2026-10-04"};
const spend={points:0,requests:0,unmetered:0,remaining:null,resetAt:null};
function measurement(count=50,complete=false,total:number|null=250) { return {rows:[{login:owner.viewer,prs:count,additions:count,deletions:0,changedFiles:count,reviewsReceived:0,cycleTimeHours:[]}],total,retrieved:50,complete,truncatedSlices:[],refusedFields:0,slices:1,rounds:1,spend,slowest:[],largest:[],repoCounts:[{repo:`synthetic-lab/${complete?'finished':'initial'}`,merged:count}],accumulated:count,accumulating:true,daysCovered:complete?30:0,daysTotal:30}; }
function mount(initial=measurement(), strict=false) {
 const qc=new QueryClient({defaultOptions:{queries:{retry:false}}});
 seam.call.mockImplementation((name:string)=>{
  if(name==='stats_board')return Promise.resolve({owner,viewer:owner.viewer,scopeKey,window,stream:'synthetic-stream',...initial,backfill:{state:'registered',owner,lastFrame:null}});
  if(name==='stats_board_cached')return Promise.resolve({owner,viewer:owner.viewer,scopeKey,window,stream:'synthetic-stream',measurement:measurement(250,true)});
  if(name==='stats_tree')return Promise.resolve({viewer:owner.viewer,orgs:[],repos:[]});
  return new Promise(()=>{});
 });
 const tree=<QueryClientProvider client={qc}><StatsPage/></QueryClientProvider>;
 render(strict?<StrictMode>{tree}</StrictMode>:tree);
 return qc;
}
afterEach(()=>{cleanup();seam.call.mockReset();seam.callbacks.clear();});
it("automatically replaces the mounted partial table from matching stored progress without another provider board load",async()=>{
 const qc=mount();await screen.findByText('synthetic-lab/initial');
 await waitFor(()=>expect(seam.callbacks.get('stats-backfill-progress')?.size).toBeGreaterThan(0));
 await act(async()=>{for(const callback of seam.callbacks.get('stats-backfill-progress')??[])callback({payload:{owner,scopeKey,daysCovered:30,daysTotal:30,collected:250,total:250,phase:{kind:'converged'},nextTickAtMs:null,observation:{...window,stream:'synthetic-stream',sequence:1,cacheChange:1}}});});
 await screen.findByText('synthetic-lab/finished');
 expect(seam.call.mock.calls.filter(c=>c[0]==='stats_board')).toHaveLength(1);
 expect(seam.call.mock.calls.filter(c=>c[0]==='stats_board_cached')).toHaveLength(1);
 expect(qc.getQueryData<{complete:boolean}>(['stats-board','org:synthetic-lab','merged',30])?.complete).toBe(true);
 qc.clear();
});
it("renders the accumulated measured population without an impossible partial denominator",async()=>{
 const qc=mount({...measurement(50,false,0),retrieved:0});
 await screen.findByText('synthetic-lab/initial');
 expect(screen.queryByText(/share of the .* of 0 merged/i)).toBeNull();
 expect(screen.getByText('share of the 50 merged that could be measured')).toBeTruthy();
 qc.clear();
});

it("keeps the initial normal board through development StrictMode effect replay",async()=>{
 const qc=mount(measurement(),true);await screen.findByText('synthetic-lab/initial');
 expect(seam.call.mock.calls.filter(c=>c[0]==='stats_board')).toHaveLength(1);qc.clear();
});
it("retains the mounted table on readback failure and its local Retry recovers without a provider load",async()=>{
 const qc=mount();await screen.findByText('synthetic-lab/initial');
 const original=seam.call.getMockImplementation()!;
 seam.call.mockImplementation((name:string,...args:unknown[])=>name==='stats_board_cached'?Promise.reject(new Error('synthetic storage busy')):original(name,...args));
 await act(async()=>{for(const callback of seam.callbacks.get('stats-backfill-progress')??[])callback({payload:{owner,scopeKey,daysCovered:30,daysTotal:30,collected:250,total:250,phase:{kind:'converged'},nextTickAtMs:null,observation:{...window,stream:'synthetic-stream',sequence:1,cacheChange:1}}});});
 await screen.findByText(/Stored Stats refresh failed/);expect(screen.getByText('synthetic-lab/initial')).toBeTruthy();
 seam.call.mockImplementation(original);
 await act(async()=>screen.getByRole('button',{name:'Retry stored Stats'}).click());
 await screen.findByText('synthetic-lab/finished');
 expect(seam.call.mock.calls.filter(c=>c[0]==='stats_board')).toHaveLength(1);qc.clear();
});
it("does not overwrite a newer partial local measurement with the older progress frame that triggered it",async()=>{
 const qc=mount();await screen.findByText('synthetic-lab/initial');
 const original=seam.call.getMockImplementation()!;
 seam.call.mockImplementation((name:string,...args:unknown[])=>name==='stats_board_cached'?Promise.resolve({owner,viewer:owner.viewer,scopeKey,window,stream:'synthetic-stream',measurement:measurement(150,false)}):original(name,...args));
 await act(async()=>{for(const callback of seam.callbacks.get('stats-backfill-progress')??[])callback({payload:{owner,scopeKey,daysCovered:6,daysTotal:30,collected:50,total:250,phase:{kind:'working'},nextTickAtMs:null,observation:{...window,stream:'synthetic-stream',sequence:1,cacheChange:1}}});});
 await waitFor(()=>expect(qc.getQueryData<{accumulated:number}>(['stats-board','org:synthetic-lab','merged',30])?.accumulated).toBe(150));
 expect(screen.queryByText(/\b50 of 250 pull requests collected/)).toBeNull();
 expect(screen.getByText(/150 of 250 pull requests collected/)).toBeTruthy();qc.clear();
});
