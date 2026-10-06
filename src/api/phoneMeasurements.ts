import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { call } from "./transport";
import { acceptCaptureWrite, beginCaptureWrite } from "./measurementCapture";
import type { ClientMeasurement, MeasurementCapture, MeasurementExportReceipt, MeasurementStatus } from "../types/measurement";
const successes = new WeakMap<QueryClient, number>();
export const PHONE_MEASUREMENT_PREFS = ["phone-measurement-prefs"] as const;
export interface PhoneMeasurementPrefs { enabled: boolean }
export const phoneMeasurementStatus = () => call<MeasurementStatus>("phone_measurement_status");
export const sharePhoneMeasurements = () => call<{outcome:"shared"|"cancelled"|"presented";report:MeasurementExportReceipt}>("export_phone_measurements");
export const recordPhoneMeasurements = (batch:ClientMeasurement[]) => call<void>("record_phone_measurements",{batch});
export function usePhoneMeasurementPrefs() {
  const qc=useQueryClient();
  const query=useQuery({queryKey:PHONE_MEASUREMENT_PREFS,queryFn:()=>call<PhoneMeasurementPrefs>("get_phone_measurement_prefs"),staleTime:Infinity,retry:false});
  return {...query,set:async(prefs:PhoneMeasurementPrefs)=>{
    const serial=beginCaptureWrite(qc);
    const result=await call<{enabled:boolean;capture:MeasurementCapture|null}>("set_phone_measurement_prefs",{prefs});
    acceptCaptureWrite(qc,serial,result.capture);
    if(serial > (successes.get(qc) ?? 0)){successes.set(qc,serial);qc.setQueryData(PHONE_MEASUREMENT_PREFS,{enabled:result.enabled});}
  }};
}
/** One local preference read at companion startup, including while unpaired. */
export function PhoneMeasurementStartup(){usePhoneMeasurementPrefs();return null;}
