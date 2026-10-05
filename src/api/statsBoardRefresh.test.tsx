import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { StatsBackfillFrame, StatsBoard, StatsBoardReadback } from '@/types/pr';
const seam=vi.hoisted(()=>({cached:vi.fn(),callbacks:new Map<string,Set<(e:{payload:unknown})=>void>>() }));
vi.mock('./tauri',()=>({statsBoardCached:seam.cached}));
vi.mock('./transport',()=>({listen:vi.fn(async(name:string,fn:(e:{payload:unknown})=>void)=>{const set=seam.callbacks.get(name)??new Set();set.add(fn);seam.callbacks.set(name,set);return()=>set.delete(fn);})}));
import { readStatsBoard, retireStatsOwnership, useStatsBoardRefresh } from './statsBoardRefresh';
const key=['stats-board','org:synthetic','merged',7];
const question={scopeKind:'org',scopeValue:'synthetic',measure:'merged' as const,days:7};
const owner={viewer:'synthetic',generation:1};
const window={from:'2026-09-01',to:'2026-09-07'};
const clients:QueryClient[]=[];
function board(count=1,extra:Partial<StatsBoard>={}):StatsBoard{return {owner,viewer:owner.viewer,scopeKey:'merged|*|org:synthetic',window,stream:'stream-a',rows:[],repoCounts:[{repo:`synthetic/${count}`,merged:count}],total:10,retrieved:count,complete:false,truncatedSlices:[],refusedFields:0,slices:1,rounds:1,spend:{points:0,requests:0,unmetered:0,remaining:null,resetAt:null},slowest:[],largest:[],accumulated:count,accumulating:true,daysCovered:0,daysTotal:7,backfill:{state:'failed',reason:'registration unavailable'},...extra};}
function reply(count=2,extra:Partial<StatsBoard>={}):StatsBoardReadback {const b=board(count,extra);return{owner:b.owner!,viewer:b.viewer,scopeKey:b.scopeKey,window:b.window!,stream:b.stream!,measurement:Object.fromEntries(Object.entries(b).filter(([key])=>!["owner","viewer","scopeKey","window","stream","backfill"].includes(key))) as StatsBoardReadback["measurement"]};}
function setup(initial=board()) {const qc=new QueryClient({defaultOptions:{queries:{retry:false}}});clients.push(qc);qc.setQueryData(key,initial);const hook=renderHook(()=>useStatsBoardRefresh(qc,key,question,true));return{qc,...hook};}
async function flush(){await act(async()=>{await Promise.resolve();await Promise.resolve();});}
async function emit(sequence:number,cacheChange=sequence,extra:Partial<StatsBackfillFrame>={}){await act(async()=>{for(const fn of seam.callbacks.get('stats-backfill-progress')??[])fn({payload:{owner,scopeKey:board().scopeKey,daysCovered:0,daysTotal:7,collected:2,total:10,phase:{kind:'working'},nextTickAtMs:null,observation:{...window,stream:'stream-a',sequence,cacheChange},...extra}});});}
function deferred<T>(){let resolve!:(v:T)=>void;let reject!:(e:unknown)=>void;const promise=new Promise<T>((a,b)=>{resolve=a;reject=b;});return{promise,resolve,reject};}
beforeEach(()=>{seam.cached.mockResolvedValue(reply());});
afterEach(()=>{cleanup();for(const qc of clients)qc.clear();clients.length=0;seam.cached.mockReset();seam.callbacks.clear();vi.useRealTimers();});
it('coalesces observers and event bursts, ignores duplicates, countdowns, other owners and scopes',async()=>{
 const pending=deferred<StatsBoardReadback>();seam.cached.mockReturnValueOnce(pending.promise);
 const a=setup();const b=renderHook(()=>useStatsBoardRefresh(a.qc,key,question,true));await flush();expect(seam.cached).toHaveBeenCalledTimes(1);
 await emit(1);await emit(2);await emit(3);a.unmount();
 await act(async()=>pending.resolve(reply(3)));await waitFor(()=>expect(seam.cached).toHaveBeenCalledTimes(2));await flush();
 await emit(3);await emit(2);await emit(4,3);await emit(9,9,{owner:{...owner,generation:2}});await emit(10,10,{scopeKey:'other'});
 expect(seam.cached).toHaveBeenCalledTimes(2);expect(a.qc.getQueryData<StatsBoard>(key)?.backfill).toEqual(board().backfill);b.unmount();
});
it('retirement fences old callbacks, old readbacks and same-viewer generation work',async()=>{
 const pending=deferred<StatsBoardReadback>();seam.cached.mockReturnValueOnce(pending.promise);
 const a=setup();await flush();const old=[...(seam.callbacks.get('stats-backfill-progress')??[])];
 act(()=>retireStatsOwnership(a.qc));await flush();const currentCalls=seam.cached.mock.calls.length;
 await act(async()=>{pending.resolve(reply(99));for(const fn of old)fn({payload:{owner,scopeKey:board().scopeKey,observation:{...window,stream:'stream-a',sequence:99,cacheChange:99}}});});
 expect(a.qc.getQueryData<StatsBoard>(key)?.accumulated).not.toBe(99);expect(seam.cached).toHaveBeenCalledTimes(currentCalls);
});
it('waits for a normal load and rejects an older in-flight readback when normal load starts',async()=>{
 const cached=deferred<StatsBoardReadback>();seam.cached.mockReturnValueOnce(cached.promise);const a=setup();await flush();
 const normal=deferred<StatsBoard>();let load!:Promise<StatsBoard>;
 act(()=>{load=a.qc.fetchQuery({queryKey:key,queryFn:()=>readStatsBoard(a.qc,key,()=>normal.promise)});});
 await act(async()=>cached.resolve(reply(90)));expect(a.qc.getQueryData<StatsBoard>(key)?.accumulated).toBe(1);
 seam.cached.mockResolvedValue(reply(7));await act(async()=>{normal.resolve(board(6));await load;});await waitFor(()=>expect(a.qc.getQueryData<StatsBoard>(key)?.accumulated).toBe(7));expect(seam.cached).toHaveBeenCalledTimes(2);
});
it('retains useful foreground-only and stronger same-window rows, accepts complete or new-window replacement',async()=>{
 seam.cached.mockResolvedValue(reply(0));const a=setup(board(5,{accumulating:false}));await flush();expect(a.qc.getQueryData<StatsBoard>(key)?.retrieved).toBe(5);
 seam.cached.mockResolvedValue(reply(10,{complete:true}));await emit(1);await waitFor(()=>expect(a.qc.getQueryData<StatsBoard>(key)?.complete).toBe(true));
 seam.cached.mockResolvedValue(reply(0,{window:{from:'2026-09-02',to:'2026-09-08'},total:null}));await emit(2);await waitFor(()=>expect(a.qc.getQueryData<StatsBoard>(key)?.accumulated).toBe(0));
});
it('bounds final-event retries and keeps failed progress retryable without duplicate replenishment',async()=>{
 vi.useFakeTimers();seam.cached.mockRejectedValue(new Error('synthetic storage refusal'));const a=setup();await flush();expect(seam.cached).toHaveBeenCalledTimes(1);
 await act(async()=>{await vi.advanceTimersByTimeAsync(5000);});expect(seam.cached).toHaveBeenCalledTimes(2);
 await act(async()=>{await vi.advanceTimersByTimeAsync(30000);});expect(seam.cached).toHaveBeenCalledTimes(3);
 await act(async()=>{await vi.advanceTimersByTimeAsync(60000);});expect(seam.cached).toHaveBeenCalledTimes(3);expect(a.qc.getQueryData<StatsBoard>(key)?.accumulated).toBe(1);
 seam.cached.mockResolvedValue(reply(8));act(()=>a.result.current.retry());await flush();expect(a.qc.getQueryData<StatsBoard>(key)?.accumulated).toBe(8);
});
it('fails closed for unsupported backends until ownership retires, with no normal-load fallback',async()=>{
 seam.cached.mockRejectedValue(new Error('unknown command stats_board_cached'));const a=setup();await flush();await emit(1);act(()=>a.result.current.retry());await flush();expect(seam.cached).toHaveBeenCalledTimes(1);expect(a.result.current.unsupported).toBe(true);
 seam.cached.mockResolvedValue(reply(8));act(()=>retireStatsOwnership(a.qc));await flush();expect(seam.cached).toHaveBeenCalledTimes(2);expect(a.qc.getQueryData<StatsBoard>(key)?.accumulated).toBe(8);
});
it('reconciles same-stream reconnect and UTC rollover without a repeating read poll',async()=>{
 vi.useFakeTimers();vi.setSystemTime(new Date('2026-09-08T23:59:59.000Z'));const a=setup();await flush();expect(seam.cached).toHaveBeenCalledTimes(1);
 await act(async()=>{for(const fn of seam.callbacks.get('connection-state')??[])fn({payload:{state:'connected'}});});await flush();expect(seam.cached).toHaveBeenCalledTimes(2);
 await act(async()=>{for(const fn of seam.callbacks.get('connection-state')??[])fn({payload:{state:'connected'}});});expect(seam.cached).toHaveBeenCalledTimes(2);
 await act(async()=>{await vi.advanceTimersByTimeAsync(1010);});expect(seam.cached).toHaveBeenCalledTimes(3);
 await act(async()=>{await vi.advanceTimersByTimeAsync(60000);});expect(seam.cached).toHaveBeenCalledTimes(3);a.unmount();
});
it('remembers an unsupported backend across different questions until ownership changes',async()=>{
 seam.cached.mockRejectedValue(new Error('unknown command stats_board_cached'));const a=setup();await flush();expect(seam.cached).toHaveBeenCalledTimes(1);
 const otherKey=['stats-board','org:other','merged',7];a.qc.setQueryData(otherKey,board(1,{scopeKey:'merged|*|org:other'}));
 const other=renderHook(()=>useStatsBoardRefresh(a.qc,otherKey,{...question,scopeValue:'other'},true));await flush();
 expect(seam.cached).toHaveBeenCalledTimes(1);expect(other.result.current.unsupported).toBe(true);
});
