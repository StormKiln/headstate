import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import type { StatsReviewers } from "../types/pr";
const call=vi.hoisted(()=>vi.fn());
vi.mock("./transport",async original=>({...await original<Record<string,unknown>>(),call}));
import { useStatsReviewers } from "./hooks";
afterEach(()=>{cleanup();call.mockReset();});
const owner={viewer:"alice",generation:1};
const receipt={owner,fetchedAt:"2026-09-01T12:00:00Z",reused:false,retained:false,qualification:null};
const spend={points:2,requests:2,unmetered:0,remaining:4900,resetAt:null};
const accepted:StatsReviewers={rows:[{login:"alice",reviews:11},{login:"bob",reviews:22}],unmeasured:[],refusedFields:0,spend,receipt};
it("retains only current-roster intersections through pending/failure and clears on owner or backend reset",async()=>{
 let reject: ((error: Error)=>void)|undefined;
 let attempts=0;
 call.mockImplementation((name:string)=>{
  if(name!=="stats_reviewers")return Promise.resolve();
  attempts++;return attempts===1?Promise.resolve(accepted):new Promise((_,fail)=>{reject=fail;});
 });
 const client=new QueryClient({defaultOptions:{queries:{retry:false}}});
 const {result,rerender}=renderHook(({logins,current})=>useStatsReviewers({kind:"org",value:"acme",subject:undefined},30,logins,true,current),{initialProps:{logins:["alice","bob"],current:owner},wrapper:({children}:{children:ReactNode})=><QueryClientProvider client={client}>{children}</QueryClientProvider>});
 await waitFor(()=>expect(result.current.data?.rows).toHaveLength(2));
 rerender({logins:["bob","charlie"],current:owner});
 await waitFor(()=>expect(attempts).toBe(2));
 expect(result.current.data?.rows).toEqual([{login:"bob",reviews:22}]);
 expect(result.current.data?.unmeasured).toEqual(["charlie"]);
 expect(result.current.data?.receipt).toMatchObject({fetchedAt:receipt.fetchedAt,retained:true,owner});
 await act(async()=>reject?.(new Error("provider offline")));
 await waitFor(()=>expect(result.current.isError).toBe(true));
 expect(result.current.data?.rows).toEqual([{login:"bob",reviews:22}]);
 expect(result.current.data?.receipt?.qualification).toMatch(/failed/i);
 rerender({logins:["bob","charlie"],current:{viewer:"alice",generation:3}});
 expect(result.current.data).toBeUndefined();
 rerender({logins:["bob","charlie"],current:owner});
 expect(result.current.data?.rows).toEqual([{login:"bob",reviews:22}]);
 act(()=>client.clear());
 rerender({logins:["bob","charlie"],current:owner});
 expect(result.current.data).toBeUndefined();
 rerender({logins:["bob","charlie"],current:{viewer:"alice",generation:3}});
 expect(result.current.data).toBeUndefined();
 client.clear();
});

it("does not keep accepted same-roster rows after the pairing QueryClient is cleared",async()=>{
 let attempts=0;
 call.mockImplementation((name:string)=>{
  if(name!=="stats_reviewers")return Promise.resolve();
  return ++attempts===1?Promise.resolve(accepted):new Promise(()=>{});
 });
 const client=new QueryClient({defaultOptions:{queries:{retry:false}}});
 const {result,rerender}=renderHook(()=>useStatsReviewers({kind:"org",value:"acme",subject:undefined},30,["alice","bob"],true,owner),{wrapper:({children}:{children:ReactNode})=><QueryClientProvider client={client}>{children}</QueryClientProvider>});
 await waitFor(()=>expect(result.current.data?.rows).toHaveLength(2));
 act(()=>client.clear());rerender();
 expect(result.current.data).toBeUndefined();
 await waitFor(()=>expect(attempts).toBe(2));
 client.clear();
});
