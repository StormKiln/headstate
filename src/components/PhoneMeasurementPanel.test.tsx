import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
const call=vi.hoisted(()=>vi.fn());
vi.mock("../api/transport",()=>({call}));
import { PhoneMeasurementPanel } from "./PhoneMeasurementPanel";
const status={enabled:false,durableRecords:0,oldestWallMs:null,newestWallMs:null,writerState:"disabled",incomplete:false};
beforeEach(()=>{call.mockReset();call.mockImplementation((name:string)=>Promise.resolve(name==="get_phone_measurement_prefs"?{enabled:false}:status));});
function mount(){const client=new QueryClient({defaultOptions:{queries:{retry:false}}});render(<QueryClientProvider client={client}><PhoneMeasurementPanel/></QueryClientProvider>);return client;}
it("uses only local preferences, preserves failed save, and shares retained recent data while off",async()=>{
 const client=mount();const toggle=await screen.findByRole("checkbox",{name:"Record phone measurements"});
 await waitFor(()=>expect((toggle as HTMLInputElement).disabled).toBe(false));expect((toggle as HTMLInputElement).checked).toBe(false);
 call.mockImplementation((name:string)=>name==="set_phone_measurement_prefs"?Promise.reject(new Error("failed local store")):Promise.resolve(status));
 fireEvent.click(toggle);await screen.findByText(/Could not save phone/);expect((toggle as HTMLInputElement).checked).toBe(false);
 expect(client.getQueryData(["measurement-capture"])).toBeUndefined();
 call.mockImplementation((name:string)=>Promise.resolve(name==="export_phone_measurements"?{outcome:"cancelled",report:{records:1,incomplete:true}}:status));
 fireEvent.click(screen.getByRole("button",{name:"Share recent measurement report…"}));await screen.findByText("Sharing canceled.");
 expect(call.mock.calls.every(([name])=>String(name).includes("phone_measurement"))).toBe(true);
});
it("publishes only successful local capture identity without adopting a host identity",async()=>{
 const client=mount();const toggle=await screen.findByRole("checkbox");await waitFor(()=>expect((toggle as HTMLInputElement).disabled).toBe(false));
 call.mockImplementation((name:string)=>Promise.resolve(name==="set_phone_measurement_prefs"?{enabled:true,capture:{epoch:"local",capture:2}}:{...status,enabled:true}));
 await act(async()=>{fireEvent.click(toggle);});await waitFor(()=>expect((toggle as HTMLInputElement).checked).toBe(true));
 expect(client.getQueryData(["measurement-capture"])).toEqual({epoch:"local",capture:2});
});

it("shows unsupported local commands as unavailable, not an empty capture",async()=>{
 call.mockRejectedValue(new Error("unsupported"));mount();await screen.findByText("Phone measurement preferences are unavailable. Update this phone app and try again.");expect(screen.queryByText(/0 retained records/)).toBeNull();
});
