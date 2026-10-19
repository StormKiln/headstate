import assert from 'node:assert/strict';
import {assertCompatibleEnvironment,assertCompatibleWorkload,normalizeWorkload,normalizeEnvironment} from './workload.mjs';
import {validateResourceResult,resourceCoverage} from './resources.mjs';

export function resourceBatchPlan(stage){
 assert.ok(['calibrate','validate','overhead'].includes(stage));
 if(stage==='overhead')return Array.from({length:5},(_,pair)=>(pair%2?['enabled','disabled']:['disabled','enabled']).map(arm=>({name:`pair-${pair+1}-${arm}`,pair:pair+1,arm,warm:false}))).flat();
 return Array.from({length:5},(_,index)=>[false,true].map(warm=>({name:`${stage}-${index+1}-${warm?'warm':'cold'}`,sample:index+1,warm}))).flat();
}
export function assertResourceComparable(run,reference,{observed=true}={}){
 assert.equal(run.pass,true,'resource run must pass');assert.equal(typeof run.warm,'boolean','warm state required');assert.equal(run.mode,'mixed','resource budgets require real mixed adapter');
 assert.equal(run.calibrationEligible,true,'exploratory evidence cannot calibrate');
 assert.equal(run.resourceIdentity?.phasePlanStatus,'fixed');
 const processes=run.restart?.actualProcesses;assert.equal(processes?.length,2,'actual two-process restart required');
 assert.ok(processes.every(p=>Number.isSafeInteger(p.pid)&&p.pid>0&&typeof p.nonce==='string'&&p.nonce.length));assert.notEqual(processes[0].pid,processes[1].pid);assert.notEqual(processes[0].nonce,processes[1].nonce);
 for(const key of ['sourceResultSha256','resumedResultSha256','checkpointSha256'])assert.match(run.restart[key],/^[a-f0-9]{64}$/,'restart artifact identity required');
 const segments=run.resourceIdentity.segmentPhasePlan;assert.deepEqual(Object.keys(segments??{}).sort(),['initial','resumed']);assert.deepEqual(Object.keys(segments.initial).sort(),['active_ingestion','churn_restart','idle','unchanged_refresh']);assert.deepEqual(Object.keys(segments.resumed),['churn_restart']);
 for(const durations of Object.values(segments))for(const duration of Object.values(durations))assert.ok(Number.isFinite(duration)&&duration>0);
 for(const phase of ['idle','unchanged_refresh','active_ingestion','churn_restart'])assert.equal(run.resourceIdentity.phasePlan[phase],(segments.initial[phase]??0)+(segments.resumed[phase]??0));
 if(observed){assert.equal(run.resources?.evidence?.schema,2,'single-process mixed evidence is ineligible');assert.deepEqual(run.resources.evidence.segments.map(s=>s.process),processes);}
 assert.equal(run.engine,'chromium','authoritative memory calibration is Chromium');assert.equal(run.environment.platform,'darwin');assert.equal(run.environment.architecture,'arm64');
 assert.equal(run.workload.populations.reviewing,150,'fixed150 workload required');
 assertCompatibleEnvironment(run.environment,reference.environment);assertCompatibleWorkload(run.workload,reference.workload);
 assert.deepEqual(run.resourceIdentity,reference.resourceIdentity,'manifest, phase plan and toolchain must match');
 assert.ok(run.resourceIdentity?.manifestSha256,'generated corpus manifest identity required');
 assert.ok(run.resourceIdentity?.toolchain,'toolchain identity required');
 assert.deepEqual(Object.keys(run.resourceIdentity?.phasePlan??{}).sort(),['active_ingestion','churn_restart','idle','unchanged_refresh']);
 for(const value of Object.values(run.resourceIdentity.phasePlan))assert.ok(Number.isFinite(value)&&value>0,'fixed phase durations required');
 if(observed){validateResourceResult(run);for(const segment of run.resources.summary.segments)for(const [phase,requestedMs] of Object.entries(segments[segment.kind])){const actual=segment.summary.phases[phase].durationMs;assert.ok(actual>=requestedMs&&actual<=requestedMs+2500,'local phase duration incompatible with fixed plan');}for(const [phase,requestedMs] of Object.entries(run.resourceIdentity.phasePlan)){const actualMs=run.resources.summary.phases[phase].durationMs;assert.ok(actualMs>=requestedMs&&actualMs<=requestedMs+(phase==='churn_restart'?5000:2500),'phase duration incompatible with fixed plan');}}
}
// Exact native callback allowlist. Zero means no committed callback in validated,
// lossless scoped transaction evidence; it is never a claim about unscoped writes.
export const mutationBudgetTables=Object.freeze(['queue_scan', 'snapshot', 'snapshot_sources', 'github_pr_facts', 'github_pr_fact_owner', 'settings', 'claude_session', 'claude_subagent', 'claude_session_usage', 'claude_session_model', 'claude_session_pr', 'claude_usage_ledger', 'claude_index_ledger', 'claude_index_state', 'claude_fts_identity', 'claude_transcript_fts_data', 'claude_transcript_fts_idx', 'claude_transcript_fts_content', 'claude_transcript_fts_docsize', 'claude_transcript_fts_config', 'claude_link_baseline', 'claude_link_checkpoint', 'claude_link_bytes', 'claude_link_hash', 'claude_link_continuation', 'claude_population_generation', 'claude_population_state', 'claude_corpus_epoch', 'claude_corpus_directory', 'claude_corpus_inventory']);
function writeMetrics(phase,values){
 for(const table of Object.keys(values.mutations))assert.ok(mutationBudgetTables.includes(table),'unclassified or unknown mutation table is ineligible for frozen budgets');
 return [[`${phase}.transactions`,values.transactions],...mutationBudgetTables.flatMap(table=>['insert','update','delete'].map((action,i)=>[`${phase}.mutations.${table}.${action}`,values.mutations[table]?.[i]??0]))];
}
const metricVector=run=>{
 const entries=Object.entries(run.resources.summary.phases).flatMap(([phase,values])=>[
  ...writeMetrics(phase,values),
  ...(values.segmentMemoryDrift??[]).map(drift=>[`${phase}.${drift.segment}.nativeRssGrowth`,Math.max(0,drift.nativeRssWindowMedianDelta)]),
  ...['nativeRssPeak','dbPeak','writerWaitMaxNs','transactionHoldMaxNs'].map(key=>[`${phase}.${key}`,values[key]]),
  // validateResourceResult has already rejected unavailable/lost WAL samples.
  // Confirmed file absence contributes no retained bytes to this sampled upper
  // bound; raw/display summaries keep null and their explicit absence counts.
  [`${phase}.walRetainedPeak`,values.walRetainedPeak===null?0:values.walRetainedPeak],
  ...Object.entries(values.rendererJsHeapInclusive).map(([role,heap])=>[`${phase}.rendererJsHeapInclusive.${role}`,heap.bytes]),
  ...Object.entries(values.readBytes).map(([lane,bytes])=>[`${phase}.readBytes.${lane}`,bytes]),
 ]);
 return {...Object.fromEntries(entries),commandLatencyMaxMs:run.parentObservation?.commandLatencyMaxMs};
};
export function deriveResourceLimits(runs){
 assert.equal(runs.length,10,'five cold and five warm calibration runs required');
 for(const warm of [false,true])assert.equal(runs.filter(r=>r.warm===warm).length,5);
 const reference=runs[0];for(const run of runs)assertResourceComparable(run,reference);
 const limits={cold:{},warm:{}};for(const run of runs)for(const [key,value] of Object.entries(metricVector(run))){const arm=limits[run.warm?'warm':'cold'];assert.ok(Number.isFinite(value)&&value>=0,`${key} measured calibration value required`);const count=key.endsWith('.transactions')||key.includes('.mutations.');const limit=count?(value===0?0:Math.ceil(value*2+1)):Math.ceil(value*2+(key.endsWith('Ns')?1_000_000:key.endsWith('Ms')?500:1024*1024));arm[key]=Math.max(arm[key]??0,limit);}
 return {schema:2,stage:'frozen',workload:reference.workload,environment:reference.environment,resourceIdentity:reference.resourceIdentity,coverage:resourceCoverage,sourceRevisions:[...new Set(runs.map(r=>r.environment.sourceRevision))],sampleDesign:'5 cold + 5 warm; maximum with engineering headroom, no p95 claim',headroom:'2 * observed maximum + 1 MiB for sizes, + 1 ms for transaction timing, + 500 ms command latency; scoped transaction/callback counts: 2 * observed maximum + 1 occurrence, but observed-zero remains zero; unknown tables reject',mutationBudgetTables,limits};
}
export function validateFrozenResourcePolicy(policy){
 assert.equal(policy?.schema,2);assert.equal(policy.stage,'frozen');
 assert.ok(Array.isArray(policy.sourceRevisions)&&policy.sourceRevisions.length>0&&policy.sourceRevisions.every(s=>typeof s==='string'&&s.length>0),'source revision provenance required');
 for(const key of ['sampleDesign','headroom'])assert.ok(typeof policy[key]==='string'&&policy[key].length>0,`${key} required`);
 assert.deepEqual(policy.coverage,resourceCoverage);assert.deepEqual(policy.mutationBudgetTables,mutationBudgetTables);
 const workload=normalizeWorkload(policy.workload),environment=normalizeEnvironment(policy.environment);
 assert.equal(workload.scenario,'mixed');assert.equal(workload.populations.reviewing,150);
 assert.equal(environment.platform,'darwin');assert.equal(environment.architecture,'arm64');assert.equal(environment.browser.engine,'chromium');
 const identity=policy.resourceIdentity;assert.equal(identity?.phasePlanStatus,'fixed');
 for(const key of ['manifestSha256','toolchain'])assert.ok(typeof identity[key]==='string'&&identity[key].length>0);
 const segments=identity.segmentPhasePlan;assert.deepEqual(Object.keys(segments??{}).sort(),['initial','resumed']);
 assert.deepEqual(Object.keys(segments.initial??{}).sort(),['active_ingestion','churn_restart','idle','unchanged_refresh']);assert.deepEqual(Object.keys(segments.resumed??{}),['churn_restart']);
 for(const n of Object.values(segments).flatMap(Object.values))assert.ok(Number.isSafeInteger(n)&&n>0);
 const phases=['idle','unchanged_refresh','active_ingestion','churn_restart'];assert.deepEqual(Object.keys(identity.phasePlan??{}).sort(),[...phases].sort());
 for(const phase of phases)assert.equal(identity.phasePlan[phase],(segments.initial[phase]??0)+(segments.resumed[phase]??0));
 assert.deepEqual(Object.keys(policy.limits??{}).sort(),['cold','warm']);
 const required=new Set(['commandLatencyMaxMs']);
 for(const phase of phases){
  for(const key of ['transactions','nativeRssPeak','dbPeak','writerWaitMaxNs','transactionHoldMaxNs','rendererJsHeapInclusive.desktop','rendererJsHeapInclusive.paired',...['metadata','usage','index','links'].map(lane=>`readBytes.${lane}`),...(phase==='churn_restart'?['initial','resumed']:['initial']).map(segment=>`${segment}.nativeRssGrowth`)])required.add(`${phase}.${key}`);
  for(const table of mutationBudgetTables)for(const action of ['insert','update','delete'])required.add(`${phase}.mutations.${table}.${action}`);
  required.add(`${phase}.walRetainedPeak`);
 }
 for(const limits of Object.values(policy.limits)){
  assert.ok(limits&&typeof limits==='object'&&!Array.isArray(limits),'numeric frozen limits required');
  for(const key of required)assert.ok(Object.hasOwn(limits,key),`${key} missing from frozen limits`);
  for(const [key,value] of Object.entries(limits)){assert.ok(required.has(key),`${key} unknown frozen metric`);assert.ok(Number.isSafeInteger(value)&&value>=0,`${key} must be a finite nonnegative integer`);}
 }
 return policy;
}
export function validateResourceLimits(run,policy){
 validateFrozenResourcePolicy(policy);
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

export function resourceBatchInvocation(item,out){
 return {args:['scripts/enterprise-run.mjs',out,'chromium','mixed'],env:{ENTERPRISE_MIXED_WARM:item.warm?'1':'0',ENTERPRISE_RESOURCE_DISABLED:item.arm==='disabled'?'1':'0'}};
}
