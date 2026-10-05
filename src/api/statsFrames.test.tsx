import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { StatsBackfillFrame } from "../types/pr";
const handlers=vi.hoisted(()=>[] as ((event:{payload:StatsBackfillFrame})=>void)[]);
vi.mock("./transport",async original=>({...await original<Record<string,unknown>>(),listen:vi.fn(async (_:string,fn:(event:{payload:StatsBackfillFrame})=>void)=>{handlers.push(fn);return ()=>{};})}));
import { useStatsBackfill } from "./hooks";
const clients:QueryClient[]=[];
function wrapper(){const client=new QueryClient();clients.push(client);return ({children}:{children:ReactNode})=><QueryClientProvider client={client}>{children}</QueryClientProvider>;}
afterEach(()=>{cleanup();for(const client of clients)client.clear();clients.length=0;handlers.length=0;});
it("matches scope and captured owner generation after a late old-account frame",async()=>{
 const old={viewer:"alice",generation:1}; const current={viewer:"alice",generation:3};
 const {result,rerender}=renderHook(({owner})=>useStatsBackfill("scope",owner),{initialProps:{owner:old},wrapper:wrapper()});
 await waitFor(()=>expect(handlers).toHaveLength(1));
 const frame:StatsBackfillFrame={owner:old,scopeKey:"scope",daysCovered:2,daysTotal:30,collected:17,total:20,phase:{kind:"working"},nextTickAtMs:null};
 act(()=>handlers[0]({payload:frame})); expect(result.current?.collected).toBe(17);
 rerender({owner:current}); await waitFor(()=>expect(handlers).toHaveLength(2)); expect(result.current).toBeNull();
 act(()=>handlers[1]({payload:frame})); expect(result.current).toBeNull();
 act(()=>handlers[1]({payload:{...frame,owner:current,collected:33}})); expect(result.current?.collected).toBe(33);
});
it("does not accept an old unqualified progress frame as a present owner",async()=>{
 const owner={viewer:"alice",generation:1};
 const {result}=renderHook(()=>useStatsBackfill("scope",owner),{wrapper:wrapper()});
 await waitFor(()=>expect(handlers).toHaveLength(1));
 const frame={scopeKey:"scope",daysCovered:2,daysTotal:30,collected:17,total:20,phase:{kind:"working" as const},nextTickAtMs:null};
 act(()=>handlers[0]({payload:frame}));expect(result.current).toBeNull();
});
it("unrelated newer frames do not suppress matching ordered progress, and retired streams never display",async()=>{
 const owner={viewer:"alice",generation:1};
 const {result,rerender}=renderHook(({stream})=>useStatsBackfill("scope",owner,stream),{initialProps:{stream:"a"},wrapper:wrapper()});
 await waitFor(()=>expect(handlers).toHaveLength(1));
 const frame:StatsBackfillFrame={owner,scopeKey:"scope",daysCovered:2,daysTotal:30,collected:17,total:20,phase:{kind:"working"},nextTickAtMs:null,observation:{from:"2026-09-01",to:"2026-09-30",stream:"a",sequence:1,cacheChange:1}};
 act(()=>handlers[0]({payload:{...frame,scopeKey:"other",observation:{...frame.observation!,sequence:99}}}));
 act(()=>handlers[0]({payload:frame}));expect(result.current?.collected).toBe(17);
 rerender({stream:"b"});expect(result.current).toBeNull();
});
