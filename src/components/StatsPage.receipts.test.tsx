import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
const call = vi.hoisted(() => vi.fn());
vi.mock("../api/transport", async original => ({ ...await original<Record<string, unknown>>(), call, listen: vi.fn(async () => () => {}) }));
vi.mock("../store/filters", () => ({ useActiveFilters: () => ({statsScopeKind:"org",statsScopeValue:"acme"}), useFilters: () => ({setFilter:vi.fn(),setPanel:vi.fn()}) }));
import { StatsPage } from "./StatsPage";
afterEach(() => { cleanup(); call.mockReset(); });
const spend={points:0,requests:0,unmetered:0,remaining:null,resetAt:null};
const tree=(logins:string[])=>({viewer:"alice",orgs:[{login:"acme",members:logins.map(login=>({login})),membersTotal:logins.length,repos:[],reposTotal:0}],repos:[]});
function mount() {
 const client=new QueryClient({defaultOptions:{queries:{retry:false}}});
 render(<QueryClientProvider client={client}><StatsPage /></QueryClientProvider>);
 return client;
}
it("mounted StatsPage requests changed roster membership while equivalent order stays warm",async()=>{
 call.mockImplementation((command:string,args:{logins:string[]})=>{
  if(command==="stats_tree")return Promise.resolve(tree(["alice","bob"]));
  if(command==="stats_reviewers")return Promise.resolve({rows:args.logins.map(login=>({login,reviews:7})),unmeasured:[],refusedFields:0,spend});
  return new Promise(()=>{});
 });
 const client=mount();
 await waitFor(()=>expect(call.mock.calls.filter(c=>c[0]==="stats_reviewers")).toHaveLength(1));
 await act(async()=>{client.setQueryData(["stats-tree"],tree(["alice","bob","charlie"]));});
 await waitFor(()=>expect(call.mock.calls.filter(c=>c[0]==="stats_reviewers")).toHaveLength(2));
 expect(call.mock.calls.filter(c=>c[0]==="stats_reviewers")[1][1].logins).toEqual(["alice","bob","charlie"]);
 await act(async()=>{client.setQueryData(["stats-tree"],tree(["charlie","bob","alice"]));});
 expect(call.mock.calls.filter(c=>c[0]==="stats_reviewers")).toHaveLength(2);
 await act(async()=>{client.setQueryData(["stats-tree"],tree(["charlie","alice"]));});
 await waitFor(()=>expect(call.mock.calls.filter(c=>c[0]==="stats_reviewers")).toHaveLength(3));
 client.clear();
});
it("mounted StatsPage Retry reissues a fully unmeasured series through the actual hook and transport",async()=>{
 let attempts=0;
 call.mockImplementation((command:string)=>{
  if(command==="stats_tree")return Promise.resolve(tree([]));
  if(command==="stats_series"){
   attempts++;
   return Promise.resolve(attempts===1?{points:[],failedDays:Array.from({length:30},(_,i)=>`2026-09-${String(i+1).padStart(2,"0")}`),refusedFields:0,spend}:{points:[{date:"2026-09-01",merged:17,opened:18}],failedDays:[],refusedFields:0,spend});
  }
  return new Promise(()=>{});
 });
 const client=mount();
 fireEvent.click(await screen.findByRole("button",{name:/try again/i}));
 await waitFor(()=>expect(attempts).toBe(2));
 await waitFor(()=>expect(screen.queryByText(/None of the 30 days/)).toBeNull());
 expect(call.mock.calls.filter(c=>c[0]==="stats_series")[1][1]).toEqual(call.mock.calls.filter(c=>c[0]==="stats_series")[0][1]);
 client.clear();
});

it("the real reviewer hook clears empty rosters and rejects late old-roster replies", async () => {
 const { useStatsReviewers } = await import("../api/hooks");
 let finishOld: ((value: unknown) => void) | undefined;
 call.mockImplementation((command: string, args: { logins: string[] }) => {
  if (command !== "stats_reviewers") return Promise.resolve();
  if (args.logins.includes("alice")) return new Promise(resolve => { finishOld = resolve; });
  return Promise.resolve({ rows: args.logins.map(login => ({ login, reviews: 22 })), unmeasured: [], refusedFields: 0, spend });
 });
 function Consumer({ logins, scope = "acme" }: { logins: string[]; scope?: string }) {
  const query = useStatsReviewers({ kind: "org", value: scope, subject: undefined }, 30, logins, true);
  return <output>{query.data?.rows.map(row => row.login).join(",") ?? "no matching receipt"}</output>;
 }
 const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
 const wrap = (logins: string[], scope?: string) => <QueryClientProvider client={client}><Consumer logins={logins} scope={scope} /></QueryClientProvider>;
 const view = render(wrap([]));
 expect(call).not.toHaveBeenCalled();
 view.rerender(wrap(["alice"]));
 await waitFor(() => expect(call.mock.calls.filter(c => c[0] === "stats_reviewers")).toHaveLength(1));
 view.rerender(wrap(["bob"]));
 await waitFor(() => expect(call.mock.calls.filter(c => c[0] === "stats_reviewers")).toHaveLength(2));
 await screen.findByText("bob");
 await act(async () => { finishOld?.({ rows: [{ login: "alice", reviews: 11 }], unmeasured: [], refusedFields: 0, spend }); });
 expect(screen.getByRole("status").textContent).toBe("bob");
 view.rerender(wrap([]));
 expect(screen.getByRole("status").textContent).toBe("no matching receipt");
 expect(call.mock.calls.filter(c => c[0] === "stats_reviewers")).toHaveLength(2);
 view.rerender(wrap(["bob"], "different-org"));
 await waitFor(() => expect(call.mock.calls.filter(c => c[0] === "stats_reviewers")).toHaveLength(3));
 await screen.findByText("bob");
 client.clear();
});
