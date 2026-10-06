import { expect,it,vi } from "vitest";
import { QueryClient } from "@tanstack/react-query";
vi.mock("../lib/target",()=>({IS_MOBILE_BUILD:true}));
const local=vi.hoisted(()=>vi.fn(()=>Promise.resolve()));
const host=vi.hoisted(()=>vi.fn(()=>Promise.reject(new Error("unpaired"))));
vi.mock("./phoneMeasurements",()=>({PHONE_MEASUREMENT_PREFS:["phone-measurement-prefs"],recordPhoneMeasurements:local}));
vi.mock("./tauri",()=>({recordClientMeasurements:host}));
import { measurementsEnabled,publishClientMeasurements } from "./clientMeasurements";
it("ignores desktop opt-in and strips foreign correlation while preserving phone counts",async()=>{
 const qc=new QueryClient();qc.setQueryData(["ui-prefs"],{diagnostic_logging:true});expect(measurementsEnabled(qc)).toBe(false);
 qc.setQueryData(["phone-measurement-prefs"],{enabled:true});expect(measurementsEnabled(qc)).toBe(true);
 await publishClientMeasurements([{kind:"stats_view",scope:{epoch:"foreign",capture:1,id:2},outcome:"accepted",rows:4},{kind:"transcript_view",operation:{epoch:"foreign",capture:1,id:3},phase:"page",rows:3,capability:"measured"}]);
 expect(host).not.toHaveBeenCalled();expect(local).toHaveBeenCalledExactlyOnceWith([{kind:"stats_view",scope:undefined,outcome:"accepted",rows:4},{kind:"transcript_view",operation:undefined,phase:"page",rows:3,capability:"measured"}]);
});
