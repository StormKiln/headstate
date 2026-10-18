import assert from 'node:assert/strict';
import {assertCompatibleEnvironment,assertCompatibleWorkload} from './workload.mjs';
import {validateResourceResult,resourceCoverage} from './resources.mjs';

export function resourceBatchPlan(stage){
 assert.ok(['calibrate','validate','overhead'].includes(stage));
 if(stage==='overhead')return Array.from({length:5},(_,pair)=>(pair%2?['enabled','disabled']:['disabled','enabled']).map(arm=>({name:`pair-${pair+1}-${arm}`,pair:pair+1,arm,warm:false}))).flat();
 return Array.from({length:5},(_,index)=>[false,true].map(warm=>({name:`${stage}-${index+1}-${warm?'warm':'cold'}`,sample:index+1,warm}))).flat();
}
export function assertResourceComparable(run,reference,{observed=true}={}){
 assert.equal(run.pass,true,'resource run must pass');assert.equal(typeof run.warm,'boolean','warm state required');assert.equal(run.mode,'mixed','resource budgets require real mixed adapter');
 assert.equal(run.engine,'chromium','authoritative memory calibration is Chromium');assert.equal(run.environment.platform,'darwin');assert.equal(run.environment.architecture,'arm64');
 assert.equal(run.workload.populations.reviewing,150,'fixed150 workload required');
 assertCompatibleEnvironment(run.environment,reference.environment);assertCompatibleWorkload(run.workload,reference.workload);
 assert.deepEqual(run.resourceIdentity,reference.resourceIdentity,'manifest, phase plan and toolchain must match');
 assert.ok(run.resourceIdentity?.manifestSha256,'generated corpus manifest identity required');
 assert.ok(run.resourceIdentity?.toolchain,'toolchain identity required');
 assert.deepEqual(Object.keys(run.resourceIdentity?.phasePlan??{}).sort(),['active_ingestion','churn_restart','idle','unchanged_refresh']);
 for(const value of Object.values(run.resourceIdentity.phasePlan))assert.ok(Number.isFinite(value)&&value>0,'fixed phase durations required');
 if(observed){validateResourceResult(run);for(const [phase,requestedMs] of Object.entries(run.resourceIdentity.phasePlan)){const actualMs=run.resources.summary.phases[phase].durationMs;assert.ok(actualMs>=requestedMs&&actualMs<=requestedMs+2500,'phase duration incompatible with fixed plan');}}
}
const metricVector=run=>{
 const entries=Object.entries(run.resources.summary.phases).flatMap(([phase,values])=>[
  ...['nativeRssPeak','dbPeak','writerWaitMaxNs','transactionHoldMaxNs'].map(key=>[`${phase}.${key}`,values[key]]),
  ...(values.walRetainedPeak===null?[]:[[`${phase}.walRetainedPeak`,values.walRetainedPeak]]),
  ...Object.entries(values.rendererJsHeapInclusive).map(([role,heap])=>[`${phase}.rendererJsHeapInclusive.${role}`,heap.bytes]),
  ...Object.entries(values.readBytes).map(([lane,bytes])=>[`${phase}.readBytes.${lane}`,bytes]),
 ]);
 return {...Object.fromEntries(entries),commandLatencyMaxMs:run.parentObservation?.commandLatencyMaxMs};
};
export function deriveResourceLimits(runs){
 assert.equal(runs.length,10,'five cold and five warm calibration runs required');
 for(const warm of [false,true])assert.equal(runs.filter(r=>r.warm===warm).length,5);
 const reference=runs[0];for(const run of runs)assertResourceComparable(run,reference);
 const limits={cold:{},warm:{}};for(const run of runs)for(const [key,value] of Object.entries(metricVector(run))){const arm=limits[run.warm?'warm':'cold'];assert.ok(Number.isFinite(value)&&value>=0,`${key} measured calibration value required`);arm[key]=Math.max(arm[key]??0,Math.ceil(value*2+(key.endsWith('Ns')?1_000_000:key.endsWith('Ms')?500:1024*1024)));}
 return {schema:1,stage:'frozen',workload:reference.workload,environment:reference.environment,resourceIdentity:reference.resourceIdentity,coverage:resourceCoverage,sourceRevisions:[...new Set(runs.map(r=>r.environment.sourceRevision))],sampleDesign:'5 cold + 5 warm; maximum with engineering headroom, no p95 claim',headroom:'2 * observed maximum + 1 MiB for sizes, + 1 ms for transaction timing, + 500 ms command latency',limits};
}
export function validateResourceLimits(run,policy){
 assert.equal(policy.schema,1);assert.equal(policy.stage,'frozen');assert.deepEqual(policy.coverage,resourceCoverage);
 assertResourceComparable(run,policy);
 assert.deepEqual(Object.keys(policy.limits).sort(),['cold','warm']);const limits=policy.limits[run.warm?'warm':'cold'];const vector=metricVector(run);for(const [key,limit] of Object.entries(limits)){assert.ok(Number.isFinite(limit)&&limit>=0);assert.ok(Number.isFinite(vector[key]),`${key} required`);assert.ok(vector[key]<=limit,`${key} exceeds calibrated limit`);}
 for(const key of Object.keys(vector))assert.ok(Object.hasOwn(limits,key),`${key} absent from frozen calibration`);
}
export function compareObserverPair(disabled,enabled){
 assertResourceComparable(disabled,enabled,{observed:false});assertResourceComparable(enabled,enabled);
 assert.equal(disabled.warm,enabled.warm,'paired warm state must match');assert.equal(disabled.parentObservation?.lost,false,'independent sampler loss');assert.equal(enabled.parentObservation?.lost,false,'independent sampler loss');
 assert.equal(disabled.resources.enabled,false,'A resource observation must be runtime-disabled');
 assert.equal(disabled.nativeBinarySha256,enabled.nativeBinarySha256,'A/B must use identical compiled binary');
 assert.ok(enabled.nativeBinarySha256,'binary identity required');
 assert.ok(disabled.mixedOutcome&&enabled.mixedOutcome,'independent product/provider/corpus outcome witness required');
 assert.deepEqual(disabled.mixedOutcome,enabled.mixedOutcome,'observer must not alter outcomes');
 const deltas={};for(const key of ['wallMs','nativeRssPeak','artifactBytes','commandLatencyMaxMs']){assert.ok(Number.isFinite(disabled.parentObservation?.[key])&&Number.isFinite(enabled.parentObservation?.[key]),`${key} independent parent observation required`);deltas[key]=enabled.parentObservation[key]-disabled.parentObservation[key];}
 return {deltas,meaning:'enabled minus disabled; inclusive measurements, no application-memory subtraction'};
}
