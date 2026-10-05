import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { StatsBackfillFrame } from "../types/pr";
const handlers=vi.hoisted(()=>[] as ((event:{payload:StatsBackfillFrame})=>void)[]);
vi.mock("./transport",async original=>({...await original<Record<string,unknown>>(),listen:vi.fn(async (_:string,fn:(event:{payload:StatsBackfillFrame})=>void)=>{handlers.push(fn);return ()=>{};})}));
import { useStatsBackfill } from "./hooks";
afterEach(()=>{cleanup();handlers.length=0;});
it("matches scope and captured owner generation after a late old-account frame",async()=>{
 const old={viewer:"alice",generation:1}; const current={viewer:"alice",generation:3};
 const {result,rerender}=renderHook(({owner})=>useStatsBackfill("scope",owner),{initialProps:{owner:old}});
 await waitFor(()=>expect(handlers).toHaveLength(1));
 const frame:StatsBackfillFrame={owner:old,scopeKey:"scope",daysCovered:2,daysTotal:30,collected:17,total:20,phase:{kind:"working"},nextTickAtMs:null};
 act(()=>handlers[0]({payload:frame})); expect(result.current?.collected).toBe(17);
 rerender({owner:current}); await waitFor(()=>expect(handlers).toHaveLength(2)); expect(result.current).toBeNull();
 act(()=>handlers[1]({payload:frame})); expect(result.current).toBeNull();
 act(()=>handlers[1]({payload:{...frame,owner:current,collected:33}})); expect(result.current?.collected).toBe(33);
});
